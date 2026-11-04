//! Optional group-domain commit provenance and owner-local possible-change hints.
//!
//! A selected native composition initializes this schema explicitly alongside
//! its group schemas. Ordinary commits never create it. Every participating
//! group transaction advances the same stamp atomically with its durable facts,
//! allowing bounded multi-job scans to reject mixed revisions. It is not a
//! receipt, an event log, a protocol clock or a cross-domain transaction claim.

use crate::{db_error, ChangeWitness, RedbError};
use prns_groups_store::StoreIncarnation;
use redb::{
    Database, MultimapTableHandle, ReadableDatabase, ReadableTable, ReadableTableMetadata,
    TableDefinition, TableHandle, WriteTransaction,
};

const TABLE: TableDefinition<u8, &[u8]> = TableDefinition::new("prns_group_change_stamp_v1");
const MAGIC: &[u8; 5] = b"PGDC\x01";
const BYTES: usize = 5 + 24 + 8;

/// One physical group-domain incarnation and its monotonic committed revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub incarnation: StoreIncarnation,
    pub revision: u64,
}
impl Stamp {
    fn encode(self) -> [u8; BYTES] {
        let mut result = [0; BYTES];
        result[..5].copy_from_slice(MAGIC);
        result[5..29].copy_from_slice(self.incarnation.as_bytes());
        result[29..].copy_from_slice(&self.revision.to_be_bytes());
        result
    }
    fn decode(bytes: &[u8]) -> Result<Self, RedbError> {
        if bytes.len() != BYTES {
            return Err(RedbError::Corrupt);
        }
        if bytes[..4] != MAGIC[..4] {
            return Err(RedbError::Corrupt);
        }
        if bytes[4] != MAGIC[4] {
            return Err(RedbError::Configuration);
        }
        let incarnation =
            StoreIncarnation::new(bytes[5..29].try_into().map_err(|_| RedbError::Corrupt)?)
                .map_err(|_| RedbError::Corrupt)?;
        let revision = u64::from_be_bytes(bytes[29..].try_into().map_err(|_| RedbError::Corrupt)?);
        Ok(Self {
            incarnation,
            revision,
        })
    }
}

fn present(tx: &WriteTransaction) -> Result<bool, RedbError> {
    for table in tx.list_multimap_tables().map_err(db_error)? {
        if table.name() == TABLE.name() {
            return Err(RedbError::Corrupt);
        }
    }
    for table in tx.list_tables().map_err(db_error)? {
        if table.name() == TABLE.name() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Explicit fresh-schema initialization within the caller's startup transaction.
/// Existing state, including an empty table, is preserved and rejected.
pub fn initialize_in(
    tx: &WriteTransaction,
    incarnation: StoreIncarnation,
) -> Result<Stamp, RedbError> {
    if present(tx)? {
        return Err(RedbError::AlreadyInitialized);
    }
    let stamp = Stamp {
        incarnation,
        revision: 0,
    };
    tx.open_table(TABLE)
        .map_err(db_error)?
        .insert(0, stamp.encode().as_slice())
        .map_err(db_error)?;
    Ok(stamp)
}

/// Strict observation. None means the optional facility was not selected, never
/// permission to initialize it or report a complete dependency scan.
pub fn read(database: &Database) -> Result<Option<Stamp>, RedbError> {
    let tx = database.begin_read().map_err(db_error)?;
    read_in(&tx)
}

/// Inspect the selected stamp within the caller's existing read snapshot.
pub fn read_in(tx: &redb::ReadTransaction) -> Result<Option<Stamp>, RedbError> {
    let table = match tx.open_table(TABLE) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(error) => return Err(db_error(error)),
    };
    if table.len().map_err(db_error)? != 1 {
        return Err(RedbError::Corrupt);
    }
    let value = table.get(0).map_err(db_error)?.ok_or(RedbError::Corrupt)?;
    Ok(Some(Stamp::decode(value.value())?))
}

/// Advance only an already-selected stamp in the same transaction as the
/// changed domain facts. Failure prevents committing those facts. An aborted
/// transaction advances nothing. Exhaustion is an explicit failure, never wrap.
pub fn advance_in(tx: &WriteTransaction) -> Result<Option<Stamp>, RedbError> {
    if !present(tx)? {
        return Ok(None);
    }
    advance_existing(tx, None).map(Some)
}

// Only commit_selected may call this without checking presence first: it owns
// the transaction and aborts on error, including a missing table. redb's write
// open creates absent tables, so this must not expose a fallible borrowed-write
// API that could leave an implicitly created empty stamp for a caller to commit.
fn advance_existing(
    tx: &WriteTransaction,
    expected: Option<StoreIncarnation>,
) -> Result<Stamp, RedbError> {
    let mut table = tx.open_table(TABLE).map_err(db_error)?;
    if table.len().map_err(db_error)? != 1 {
        return Err(RedbError::Corrupt);
    }
    let mut stamp = {
        let value = table.get(0).map_err(db_error)?.ok_or(RedbError::Corrupt)?;
        Stamp::decode(value.value())?
    };
    if expected.is_some_and(|incarnation| stamp.incarnation != incarnation) {
        return Err(RedbError::ForeignIncarnation);
    }
    stamp.revision = stamp
        .revision
        .checked_add(1)
        .ok_or(RedbError::CounterExhausted)?;
    table
        .insert(0, stamp.encode().as_slice())
        .map_err(db_error)?;
    Ok(stamp)
}

/// Common group commit boundary. The hint precedes the fallible commit, while
/// the durable stamp succeeds or fails atomically with the supplied transaction.
/// The caller owns durability selection and all domain/recovery decisions.
pub fn commit(tx: WriteTransaction, witness: Option<&ChangeWitness>) -> Result<(), RedbError> {
    advance_in(&tx)?;
    finish(tx, witness)
}

/// Commit for an owner that explicitly selected this group-domain incarnation.
/// This checks its exact stamp inside the same write transaction without
/// enumerating unrelated tables. Missing, corrupt, foreign or exhausted state
/// aborts the entire transaction, including any staged domain facts. It never
/// initializes the optional facility; use [`initialize_in`] at schema creation.
pub fn commit_selected(
    tx: WriteTransaction,
    witness: Option<&ChangeWitness>,
    incarnation: StoreIncarnation,
) -> Result<(), RedbError> {
    advance_existing(&tx, Some(incarnation))?;
    finish(tx, witness)
}

pub(crate) fn commit_with_selection(
    tx: WriteTransaction,
    witness: Option<&ChangeWitness>,
    incarnation: Option<StoreIncarnation>,
) -> Result<(), RedbError> {
    match incarnation {
        Some(incarnation) => commit_selected(tx, witness, incarnation),
        None => commit(tx, witness),
    }
}

fn finish(tx: WriteTransaction, witness: Option<&ChangeWitness>) -> Result<(), RedbError> {
    if let Some(witness) = witness {
        witness.mark_commit_attempt();
    }
    tx.commit().map_err(db_error)
}

#[cfg(test)]
mod selected_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    const DATA: TableDefinition<u8, u64> = TableDefinition::new("example");
    fn setup() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("store.redb")).unwrap();
        (dir, db)
    }
    fn incarnation() -> StoreIncarnation {
        StoreIncarnation::new([7; 24]).unwrap()
    }

    #[test]
    fn absence_stays_absent_but_owner_hint_records_commit_attempts() {
        let (_dir, db) = setup();
        let witness = ChangeWitness::default();
        assert_eq!(read(&db).unwrap(), None);
        let tx = db.begin_write().unwrap();
        tx.open_table(DATA).unwrap().insert(0, 9).unwrap();
        commit(tx, Some(&witness)).unwrap();
        assert_eq!(read(&db).unwrap(), None);
        assert!(witness.clone().take_possible_change());
        assert!(!witness.take_possible_change());
    }

    #[test]
    fn stamp_is_atomic_with_domain_changes_and_stable_across_reads_and_reopen() {
        let (dir, db) = setup();
        let tx = db.begin_write().unwrap();
        initialize_in(&tx, incarnation()).unwrap();
        tx.commit().unwrap();
        let initial = read(&db).unwrap().unwrap();
        {
            let tx = db.begin_write().unwrap();
            advance_in(&tx).unwrap();
            tx.open_table(DATA).unwrap().insert(0, 1).unwrap();
        }
        assert_eq!(read(&db).unwrap(), Some(initial));
        let tx = db.begin_write().unwrap();
        tx.open_table(DATA).unwrap().insert(0, 2).unwrap();
        commit(tx, None).unwrap();
        let next = read(&db).unwrap().unwrap();
        assert_eq!(next.revision, 1);
        assert_eq!(next.incarnation, initial.incarnation);
        assert_eq!(read(&db).unwrap(), Some(next));
        drop(db);
        let db = Database::open(dir.path().join("store.redb")).unwrap();
        assert_eq!(read(&db).unwrap(), Some(next));
        assert_eq!(
            db.begin_read()
                .unwrap()
                .open_table(DATA)
                .unwrap()
                .get(0)
                .unwrap()
                .unwrap()
                .value(),
            2
        );
    }

    #[test]
    fn existing_empty_unsupported_and_exhausted_state_never_resets() {
        let (_dir, db) = setup();
        let tx = db.begin_write().unwrap();
        drop(tx.open_table(TABLE).unwrap());
        tx.commit().unwrap();
        assert!(matches!(read(&db), Err(RedbError::Corrupt)));
        let tx = db.begin_write().unwrap();
        assert!(matches!(
            initialize_in(&tx, incarnation()),
            Err(RedbError::AlreadyInitialized)
        ));
        drop(tx);
        for (version, revision) in [(2, 0), (1, u64::MAX)] {
            let mut encoded = Stamp {
                incarnation: incarnation(),
                revision,
            }
            .encode();
            encoded[4] = version;
            let tx = db.begin_write().unwrap();
            tx.open_table(TABLE)
                .unwrap()
                .insert(0, encoded.as_slice())
                .unwrap();
            tx.commit().unwrap();
            let witness = ChangeWitness::default();
            let tx = db.begin_write().unwrap();
            tx.open_table(DATA).unwrap().insert(0, 1).unwrap();
            assert!(matches!(
                commit(tx, Some(&witness)),
                Err(RedbError::Configuration | RedbError::CounterExhausted)
            ));
            assert!(!witness.take_possible_change());
            assert!(matches!(
                db.begin_read().unwrap().open_table(DATA),
                Err(redb::TableError::TableDoesNotExist(_))
            ));
        }
    }
}
