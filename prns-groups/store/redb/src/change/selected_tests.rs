#![allow(clippy::unwrap_used)]
use super::*;
use crate::{RedbLimits, RedbMaintenanceRequest, RedbStoreOwner, StoreNamespace};
use prns_groups_store::{
    AtomicBackend, CommitOutcome, MaintenanceBackend, Mutation, OperationTicket, Revision,
};

const DATA: TableDefinition<u8, u64> = TableDefinition::new("other_domain");
const NAMESPACE: StoreNamespace = StoreNamespace([3; 16]);
const IMPORTED: StoreNamespace = StoreNamespace([4; 16]);

fn incarnation(value: u8) -> StoreIncarnation {
    StoreIncarnation::new([value; 24]).unwrap()
}
fn setup() -> (tempfile::TempDir, Database) {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::create(directory.path().join("selected.redb")).unwrap();
    (directory, database)
}
fn initialize(database: &Database) {
    let tx = database.begin_write().unwrap();
    initialize_in(&tx, incarnation(1)).unwrap();
    tx.commit().unwrap();
}
fn limits() -> RedbLimits {
    RedbLimits {
        max_changes: 4,
        max_key_bytes: 32,
        max_value_bytes: 128,
        max_staged_bytes: 512,
        max_live_objects: 8,
        max_live_bytes: 1024,
        max_receipts: 8,
    }
}
fn put() -> Mutation {
    Mutation::Put {
        key: b"record".to_vec(),
        value: b"contents".to_vec(),
    }
}
fn ticket(sequence: u64) -> OperationTicket {
    OperationTicket::new(incarnation(2), sequence).unwrap()
}
fn revision(database: &Database) -> u64 {
    read(database).unwrap().unwrap().revision
}

#[test]
fn selected_commit_preserves_atomic_stamp_and_unrelated_tables() {
    let (directory, database) = setup();
    initialize(&database);
    let tx = database.begin_write().unwrap();
    for index in 0..1_000 {
        let name = format!("unrelated_{index}");
        tx.open_table(TableDefinition::<u8, u64>::new(&name))
            .unwrap()
            .insert(0, index)
            .unwrap();
    }
    tx.commit().unwrap();
    let witness = ChangeWitness::default();
    let tx = database.begin_write().unwrap();
    tx.open_table(DATA).unwrap().insert(0, 42).unwrap();
    commit_selected(tx, Some(&witness), incarnation(1)).unwrap();
    assert!(witness.take_possible_change());
    assert_eq!(revision(&database), 1);
    drop(database);
    let database = Database::open(directory.path().join("selected.redb")).unwrap();
    assert_eq!(revision(&database), 1);
    let read = database.begin_read().unwrap();
    assert_eq!(
        read.open_table(DATA)
            .unwrap()
            .get(0)
            .unwrap()
            .unwrap()
            .value(),
        42
    );
    assert_eq!(read.list_tables().unwrap().count(), 1_002);
}

#[test]
fn invalid_selected_stamp_aborts_facts_and_never_creates_optional_schema() {
    for case in 0..8 {
        let (_directory, database) = setup();
        let tx = database.begin_write().unwrap();
        match case {
            0 => {} // Missing stamp must not become an empty persisted table.
            1 => {
                drop(tx.open_table(TABLE).unwrap());
            }
            2 => {
                tx.open_multimap_table(redb::MultimapTableDefinition::<u8, u64>::new(TABLE.name()))
                    .unwrap()
                    .insert(0, 0)
                    .unwrap();
            }
            3 => {
                tx.open_table(TABLE)
                    .unwrap()
                    .insert(0, b"invalid".as_slice())
                    .unwrap();
            }
            4..=7 => {
                let mut stamp = Stamp {
                    incarnation: incarnation(if case == 4 { 2 } else { 1 }),
                    revision: if case == 5 { u64::MAX } else { 0 },
                }
                .encode();
                if case == 6 {
                    stamp[4] += 1;
                }
                let mut table = tx.open_table(TABLE).unwrap();
                table.insert(0, stamp.as_slice()).unwrap();
                if case == 7 {
                    table.insert(1, stamp.as_slice()).unwrap();
                }
            }
            _ => unreachable!(),
        }
        tx.commit().unwrap();
        let before = database.begin_read().unwrap();
        let tables = before.list_tables().unwrap().count();
        let witness = ChangeWitness::default();
        let tx = database.begin_write().unwrap();
        tx.open_table(DATA).unwrap().insert(0, 1).unwrap();
        assert!(commit_selected(tx, Some(&witness), incarnation(1)).is_err());
        assert!(!witness.take_possible_change());
        let after = database.begin_read().unwrap();
        assert!(matches!(
            after.open_table(DATA),
            Err(redb::TableError::TableDoesNotExist(_))
        ));
        assert_eq!(after.list_tables().unwrap().count(), tables);
        if case == 0 {
            assert_eq!(read(&database).unwrap(), None);
        }
    }
}

#[test]
fn selected_owner_covers_initialization_writes_retirement_import_and_deletion() {
    let (_directory, database) = setup();
    initialize(&database);
    let mut owner = RedbStoreOwner::new(&database).with_change_stamp(incarnation(1));
    let mut backend = owner
        .initialize(NAMESPACE, incarnation(2), limits())
        .unwrap();
    assert_eq!(revision(&database), 1);
    assert!(matches!(
        backend
            .commit(Revision(0), ticket(1).operation_id(), &[put()])
            .unwrap(),
        CommitOutcome::Committed(_)
    ));
    assert_eq!(revision(&database), 2);
    backend
        .commit(Revision(0), ticket(1).operation_id(), &[put()])
        .unwrap();
    assert_eq!(revision(&database), 2); // An exact retry has no new durable effect.
    backend
        .maintain(RedbMaintenanceRequest::FinalizeThrough(ticket(1)))
        .unwrap();
    assert_eq!(revision(&database), 3);
    let snapshot = backend.export_enrollment().unwrap();
    drop(backend);
    owner
        .import_enrollment(IMPORTED, incarnation(2), limits(), &snapshot)
        .unwrap();
    assert_eq!(revision(&database), 4);
    owner.delete(IMPORTED, incarnation(2), limits()).unwrap();
    assert_eq!(revision(&database), 5);
    owner.delete(NAMESPACE, incarnation(2), limits()).unwrap();
    assert_eq!(revision(&database), 6);
}

#[test]
fn foreign_selected_owner_cannot_mutate_or_delete_existing_namespace() {
    let (_directory, database) = setup();
    initialize(&database);
    let mut owner = RedbStoreOwner::new(&database).with_change_stamp(incarnation(1));
    let mut backend = owner
        .initialize(NAMESPACE, incarnation(2), limits())
        .unwrap();
    backend
        .commit(Revision(0), ticket(1).operation_id(), &[put()])
        .unwrap();
    let snapshot = backend.export_enrollment().unwrap();
    drop(backend);
    drop(owner);
    let witness = ChangeWitness::default();
    let mut foreign = RedbStoreOwner::new(&database)
        .with_change_stamp(incarnation(3))
        .with_change_witness(witness.clone());
    assert!(matches!(
        foreign.initialize(IMPORTED, incarnation(2), limits()),
        Err(RedbError::ForeignIncarnation)
    ));
    assert!(matches!(
        foreign.open(IMPORTED, incarnation(2), limits()),
        Err(RedbError::MissingNamespace)
    ));
    assert!(matches!(
        foreign.import_enrollment(IMPORTED, incarnation(2), limits(), &snapshot),
        Err(RedbError::ForeignIncarnation)
    ));
    let mut backend = foreign.open(NAMESPACE, incarnation(2), limits()).unwrap();
    assert!(matches!(
        backend.commit(Revision(1), ticket(2).operation_id(), &[put()]),
        Err(RedbError::ForeignIncarnation)
    ));
    assert!(matches!(
        backend.maintain(RedbMaintenanceRequest::FinalizeThrough(ticket(1))),
        Err(RedbError::ForeignIncarnation)
    ));
    assert_eq!(backend.usage().unwrap().retired_through, 0);
    assert_eq!(backend.head().unwrap(), Revision(1));
    drop(backend);
    assert!(matches!(
        foreign.delete(NAMESPACE, incarnation(2), limits()),
        Err(RedbError::ForeignIncarnation)
    ));
    drop(foreign);
    let mut owner = RedbStoreOwner::new(&database).with_change_stamp(incarnation(1));
    assert_eq!(
        owner
            .open(NAMESPACE, incarnation(2), limits())
            .unwrap()
            .read(b"record", 128)
            .unwrap(),
        Some(b"contents".to_vec())
    );
    assert_eq!(revision(&database), 2);
    assert!(!witness.take_possible_change());
}
