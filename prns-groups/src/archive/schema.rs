use prns_groups_core::{GroupId, RecordId};
use prns_groups_store_redb::{CommitContext, RedbError};
use prns_host::IdentityHash;
use redb::{
    ReadTransaction, ReadableTable, ReadableTableMetadata, TableDefinition, TableHandle,
    WriteTransaction,
};

pub(super) const HEADER: TableDefinition<u8, &[u8]> =
    TableDefinition::new(super::selection::NAMES[0]);
pub(super) const RECORDS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new(super::selection::NAMES[1]);
pub(super) const POSITIONS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new(super::selection::NAMES[2]);
const MAGIC: &[u8; 5] = b"PGAR\x01";
pub(super) const ROW_MAGIC: &[u8; 4] = b"PGA\x01";
pub(super) const ROW_FIXED: usize = 37;
pub(super) const MAX_ROW_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupArchiveLimits"))]
pub struct Limits {
    pub max_records: u64,
    pub max_bytes: u64,
}
impl Limits {
    pub fn validate(self) -> Result<(), RedbError> {
        if self.max_records == 0
            || self.max_records > 1_000_000
            || self.max_bytes < 1024
            || self.max_bytes > 1 << 40
        {
            return Err(RedbError::Configuration);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub limits: Limits,
    pub records: u64,
    pub bytes: u64,
}
pub(super) fn db(error: impl Into<redb::Error>) -> RedbError {
    RedbError::Database(error.into())
}
pub(super) fn encode(identity: IdentityHash, usage: Usage) -> [u8; 53] {
    let mut b = [0; 53];
    b[..5].copy_from_slice(MAGIC);
    b[5..21].copy_from_slice(identity.as_bytes());
    for (slot, n) in b[21..].as_chunks_mut::<8>().0.iter_mut().zip([
        usage.limits.max_records,
        usage.limits.max_bytes,
        usage.records,
        usage.bytes,
    ]) {
        slot.copy_from_slice(&n.to_be_bytes());
    }
    b
}
pub(super) fn decode(bytes: &[u8], identity: IdentityHash) -> Result<Usage, RedbError> {
    if bytes.len() != 53 || bytes[..4] != MAGIC[..4] {
        return Err(RedbError::Corrupt);
    }
    if bytes[4] != MAGIC[4] || &bytes[5..21] != identity.as_bytes() {
        return Err(RedbError::Configuration);
    }
    let n = |i| -> Result<u64, RedbError> {
        Ok(u64::from_be_bytes(
            bytes[i..i + 8].try_into().map_err(|_| RedbError::Corrupt)?,
        ))
    };
    let usage = Usage {
        limits: Limits {
            max_records: n(21)?,
            max_bytes: n(29)?,
        },
        records: n(37)?,
        bytes: n(45)?,
    };
    usage.limits.validate()?;
    if usage.records > usage.limits.max_records || usage.bytes > usage.limits.max_bytes {
        return Err(RedbError::Corrupt);
    }
    Ok(usage)
}
fn presence(names: impl Iterator<Item = impl TableHandle>) -> usize {
    names
        .filter(|t| [HEADER.name(), RECORDS.name(), POSITIONS.name()].contains(&t.name()))
        .count()
}
/// Select a fresh schema in the caller's startup transaction. Commit with
/// immediate durability before readiness. Existing/partial state is preserved.
pub fn initialize_in(
    tx: &WriteTransaction,
    identity: IdentityHash,
    limits: Limits,
) -> Result<(), RedbError> {
    limits.validate()?;
    if presence(tx.list_tables().map_err(db)?) != 0 {
        return Err(RedbError::AlreadyInitialized);
    }
    for table in tx.list_multimap_tables().map_err(db)? {
        use redb::MultimapTableHandle;
        if [HEADER.name(), RECORDS.name(), POSITIONS.name()].contains(&table.name()) {
            return Err(RedbError::Corrupt);
        }
    }
    tx.open_table(HEADER)
        .map_err(db)?
        .insert(
            0,
            encode(
                identity,
                Usage {
                    limits,
                    records: 0,
                    bytes: 0,
                },
            )
            .as_slice(),
        )
        .map_err(db)?;
    tx.open_table(RECORDS).map_err(db)?;
    tx.open_table(POSITIONS).map_err(db)?;
    Ok(())
}
/// Checks the header and row counts after full readiness validation. Enumerates
/// table metadata to detect partial or wrong-type schemas without scanning records.
pub fn read_usage_in(
    tx: &ReadTransaction,
    identity: IdentityHash,
) -> Result<Option<Usage>, RedbError> {
    for table in tx.list_multimap_tables().map_err(db)? {
        use redb::MultimapTableHandle;
        if [HEADER.name(), RECORDS.name(), POSITIONS.name()].contains(&table.name()) {
            return Err(RedbError::Corrupt);
        }
    }
    let count = presence(tx.list_tables().map_err(db)?);
    if count == 0 {
        return Ok(None);
    }
    if count != 3 {
        return Err(RedbError::Corrupt);
    }
    let header = tx.open_table(HEADER).map_err(db)?;
    if header.len().map_err(db)? != 1 {
        return Err(RedbError::Corrupt);
    }
    let usage = decode(
        header
            .get(0)
            .map_err(db)?
            .ok_or(RedbError::Corrupt)?
            .value(),
        identity,
    )?;
    let records = tx.open_table(RECORDS).map_err(db)?;
    let positions = tx.open_table(POSITIONS).map_err(db)?;
    if records.len().map_err(db)? != usage.records || positions.len().map_err(db)? != usage.records
    {
        return Err(RedbError::Corrupt);
    }
    Ok(Some(usage))
}
/// Strict selection/readiness inspection. Scans the declared record-count bound
/// with one borrowed row at a time; it does not allocate all retained history.
pub fn inspect_in(
    tx: &ReadTransaction,
    identity: IdentityHash,
) -> Result<Option<Usage>, RedbError> {
    let Some(usage) = read_usage_in(tx, identity)? else {
        return Ok(None);
    };
    let records = tx.open_table(RECORDS).map_err(db)?;
    let positions = tx.open_table(POSITIONS).map_err(db)?;
    let mut bytes = 0u64;
    for row in records.iter().map_err(db)? {
        let (key, value) = row.map_err(db)?;
        let key = key.value();
        let value = value.value();
        if key.len() != 104 {
            return Err(RedbError::Corrupt);
        }
        let parsed = parse_row(value)?;
        // Validate stored canonical bytes independently of today's delivery or
        // admission policy. Physical row bounds limit all decoder allocations.
        let record = decode_record(parsed.record)?;
        prns_groups_store::StoreIncarnation::new(
            key[16..40].try_into().map_err(|_| RedbError::Corrupt)?,
        )
        .map_err(|_| RedbError::Corrupt)?;
        if record.metadata.group.0 != key[40..72]
            || record.metadata.record.0 != key[72..]
            || parsed.flags.len() != record.targets.len()
        {
            return Err(RedbError::Corrupt);
        }
        let mut position = [0; 80];
        position[..72].copy_from_slice(&key[..72]);
        position[72..].copy_from_slice(&parsed.position.to_be_bytes());
        if positions
            .get(position.as_slice())
            .map_err(db)?
            .is_none_or(|v| v.value() != &key[72..])
        {
            return Err(RedbError::Corrupt);
        }
        bytes = bytes
            .checked_add(charge(value.len())?)
            .ok_or(RedbError::Corrupt)?;
    }
    if bytes != usage.bytes {
        return Err(RedbError::Corrupt);
    }
    Ok(Some(usage))
}
pub(super) fn usage_in(tx: &WriteTransaction, identity: IdentityHash) -> Result<Usage, RedbError> {
    if presence(tx.list_tables().map_err(db)?) != 3 {
        return Err(RedbError::Corrupt);
    }
    let h = tx.open_table(HEADER).map_err(db)?;
    if h.len().map_err(db)? != 1 {
        return Err(RedbError::Corrupt);
    }
    let usage = decode(
        h.get(0).map_err(db)?.ok_or(RedbError::Corrupt)?.value(),
        identity,
    )?;
    if tx.open_table(RECORDS).map_err(db)?.len().map_err(db)? != usage.records
        || tx.open_table(POSITIONS).map_err(db)?.len().map_err(db)? != usage.records
    {
        return Err(RedbError::Corrupt);
    }
    Ok(usage)
}
pub(super) fn save(
    tx: &WriteTransaction,
    identity: IdentityHash,
    usage: Usage,
) -> Result<(), RedbError> {
    tx.open_table(HEADER)
        .map_err(db)?
        .insert(0, encode(identity, usage).as_slice())
        .map_err(db)?;
    Ok(())
}
pub(super) fn record_key(context: CommitContext, group: GroupId, id: RecordId) -> [u8; 104] {
    let mut b = [0; 104];
    b[..16].copy_from_slice(&context.namespace.0);
    b[16..40].copy_from_slice(context.incarnation.as_bytes());
    b[40..72].copy_from_slice(&group.0);
    b[72..].copy_from_slice(&id.0);
    b
}
pub(super) fn position_key(context: CommitContext, group: GroupId, position: u64) -> [u8; 80] {
    let mut b = [0; 80];
    b[..72].copy_from_slice(&record_key(context, group, RecordId([0; 32]))[..72]);
    b[72..].copy_from_slice(&position.to_be_bytes());
    b
}
pub(super) fn charge(row_bytes: usize) -> Result<u64, RedbError> {
    u64::try_from(
        row_bytes
            .checked_add(104 + 80 + 32)
            .ok_or(RedbError::Capacity)?,
    )
    .map_err(|_| RedbError::Capacity)
}
pub(super) struct Row<'a> {
    pub author: IdentityHash,
    pub position: u64,
    pub authored: bool,
    pub flags: &'a [u8],
    pub record: &'a [u8],
}
pub(super) fn parse_row(bytes: &[u8]) -> Result<Row<'_>, RedbError> {
    if bytes.len() < ROW_FIXED || bytes.len() > MAX_ROW_BYTES || &bytes[..4] != ROW_MAGIC {
        return Err(RedbError::Corrupt);
    }
    let author = IdentityHash::new(bytes[4..20].try_into().map_err(|_| RedbError::Corrupt)?);
    let position = u64::from_be_bytes(bytes[20..28].try_into().map_err(|_| RedbError::Corrupt)?);
    let authored = match bytes[28] {
        0 => false,
        1 => true,
        _ => return Err(RedbError::Corrupt),
    };
    let record_len =
        u32::from_be_bytes(bytes[29..33].try_into().map_err(|_| RedbError::Corrupt)?) as usize;
    let flags_len =
        u32::from_be_bytes(bytes[33..37].try_into().map_err(|_| RedbError::Corrupt)?) as usize;
    let boundary = ROW_FIXED.checked_add(flags_len).ok_or(RedbError::Corrupt)?;
    if position == 0
        || boundary.checked_add(record_len) != Some(bytes.len())
        || bytes[ROW_FIXED..boundary].iter().any(|f| *f > 1)
    {
        return Err(RedbError::Corrupt);
    }
    Ok(Row {
        author,
        position,
        authored,
        flags: &bytes[ROW_FIXED..boundary],
        record: &bytes[boundary..],
    })
}

pub(super) fn decode_record(bytes: &[u8]) -> Result<prns_groups_service::Record, RedbError> {
    prns_groups_service::projection::decode_record(
        bytes,
        prns_groups_service::Limits {
            max_records: 1,
            max_targets: MAX_ROW_BYTES,
            max_body_bytes: MAX_ROW_BYTES,
            max_changes: 1,
            max_envelopes: 1,
            max_retained_wire_bytes: MAX_ROW_BYTES,
            maximum_window: prns_groups_core::Seconds(u64::MAX),
            future_skew: prns_groups_core::Seconds(0),
        },
    )
    .map_err(|_| RedbError::Corrupt)
}
