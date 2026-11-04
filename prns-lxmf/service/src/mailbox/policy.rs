//! Durable admission accounting and strict bounded row encoding.
use super::*;
use redb::{MultimapTableHandle, TableHandle};
use std::io::{self, Write};

const FORMAT: u64 = 2;
const FORMAT_KEY: &str = "lxmf_format";
const RECORD_LIMIT: &str = "lxmf_record_limit";
const BYTE_LIMIT: &str = "lxmf_byte_limit";
const PAGE_ROWS: &str = "lxmf_page_rows";
const PAGE_BYTES: &str = "lxmf_page_bytes";
const RECORDS: &str = "lxmf_records";
const ACCOUNTED: &str = "lxmf_accounted_bytes";
pub(super) const QUEUED: TableDefinition<u64, u64> = TableDefinition::new("lxmf_queued_v2");

/// Persisted local resource policy. Accounted bytes reserve each immutable
/// message's largest terminal representation and both required indexes; they
/// are logical data bytes, not redb page/allocator or caller-snapshot memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailboxLimits {
    pub max_records: u64,
    pub max_accounted_bytes: u64,
    pub max_page_rows: u32,
    pub max_page_bytes: u64,
}
impl Default for MailboxLimits {
    fn default() -> Self {
        Self {
            max_records: 100_000,
            max_accounted_bytes: 256 * 1024 * 1024,
            max_page_rows: 256,
            max_page_bytes: 1024 * 1024,
        }
    }
}
impl MailboxLimits {
    pub fn validate(self) -> Result<(), MailboxFailure> {
        if self.max_records == 0
            || self.max_accounted_bytes == 0
            || self.max_page_rows == 0
            || self.max_page_bytes
                < maximum_owned_row_bytes() as u64
                    + std::mem::size_of::<Vec<DurableLxmfMessage>>() as u64
            || usize::try_from(self.max_page_bytes).is_err()
            || usize::try_from(self.max_records).is_err()
        {
            return Err(MailboxFailure::Capacity);
        }
        Ok(())
    }
}

pub(super) struct Header {
    pub limits: MailboxLimits,
    pub records: u64,
    pub accounted: u64,
}
impl Header {
    pub fn read(meta: &impl ReadableTable<&'static str, u64>) -> Result<Self, MailboxFailure> {
        if required_meta(meta, FORMAT_KEY)? != FORMAT {
            return Err(MailboxFailure::reset_required(
                "unsupported LXMF mailbox format",
            ));
        }
        let limits = MailboxLimits {
            max_records: required_meta(meta, RECORD_LIMIT)?,
            max_accounted_bytes: required_meta(meta, BYTE_LIMIT)?,
            max_page_rows: u32::try_from(required_meta(meta, PAGE_ROWS)?)
                .map_err(|_| MailboxFailure::reset_required("mailbox page row width"))?,
            max_page_bytes: required_meta(meta, PAGE_BYTES)?,
        };
        limits
            .validate()
            .map_err(|_| MailboxFailure::reset_required("invalid stored mailbox policy"))?;
        let header = Self {
            limits,
            records: required_meta(meta, RECORDS)?,
            accounted: required_meta(meta, ACCOUNTED)?,
        };
        if header.records > limits.max_records || header.accounted > limits.max_accounted_bytes {
            return Err(MailboxFailure::reset_required(
                "mailbox accounting exceeds stored policy",
            ));
        }
        Ok(header)
    }
}

pub(super) fn initialize(
    write: &redb::WriteTransaction,
    limits: MailboxLimits,
) -> Result<(), MailboxFailure> {
    limits.validate()?;
    // First-create only: even empty or differently typed owned tables are
    // partial/existing state. Check before opening tables or staging metadata.
    let names = [
        LXMF_META.name(),
        LXMF_MESSAGES.name(),
        LXMF_INBOUND_IDS.name(),
        QUEUED.name(),
    ];
    let ordinary = write
        .list_tables()
        .map_err(|e| classify_redb_error(e.into(), "inspect existing mailbox tables"))?
        .any(|table| names.contains(&table.name()));
    let multimap = write
        .list_multimap_tables()
        .map_err(|e| classify_redb_error(e.into(), "inspect existing mailbox table kinds"))?
        .any(|table| names.contains(&table.name()));
    if ordinary || multimap {
        return Err(MailboxFailure::reset_required(
            "mailbox initialization requires absent owned tables",
        ));
    }
    let mut meta = write
        .open_table(LXMF_META)
        .map_err(|e| classify_redb_error(e.into(), "create mailbox metadata"))?;
    if meta
        .get(FORMAT_KEY)
        .map_err(|e| classify_redb_error(e.into(), "read mailbox format"))?
        .is_some()
        || meta
            .get(NEXT_LXMF_RECORD_ID_KEY)
            .map_err(|e| classify_redb_error(e.into(), "read mailbox next ID"))?
            .is_some()
    {
        return Err(MailboxFailure::reset_required(
            "mailbox initialization requires fresh state",
        ));
    }
    for (key, value) in [
        (FORMAT_KEY, FORMAT),
        (RECORD_LIMIT, limits.max_records),
        (BYTE_LIMIT, limits.max_accounted_bytes),
        (PAGE_ROWS, u64::from(limits.max_page_rows)),
        (PAGE_BYTES, limits.max_page_bytes),
        (RECORDS, 0),
        (ACCOUNTED, 0),
        (NEXT_LXMF_RECORD_ID_KEY, INITIAL_NEXT_LXMF_RECORD_ID),
        (LXMF_REVISION_KEY, INITIAL_LXMF_REVISION),
    ] {
        meta.insert(key, value)
            .map_err(|e| classify_redb_error(e.into(), "initialize mailbox metadata"))?;
    }
    let mut rows = write
        .open_table(LXMF_MESSAGES)
        .map_err(|e| classify_redb_error(e.into(), "create mailbox rows"))?;
    let mut ids = write
        .open_table(LXMF_INBOUND_IDS)
        .map_err(|e| classify_redb_error(e.into(), "create mailbox IDs"))?;
    let mut queued = write
        .open_table(QUEUED)
        .map_err(|e| classify_redb_error(e.into(), "create mailbox queued index"))?;
    if !rows
        .is_empty()
        .map_err(|e| classify_redb_error(e.into(), "check new mailbox rows"))?
        || !ids
            .is_empty()
            .map_err(|e| classify_redb_error(e.into(), "check new mailbox IDs"))?
        || !queued
            .is_empty()
            .map_err(|e| classify_redb_error(e.into(), "check new mailbox queued index"))?
    {
        return Err(MailboxFailure::reset_required(
            "mailbox initialization cannot replace existing records",
        ));
    }
    rows.insert(0, &[][..])
        .map_err(|e| classify_redb_error(e.into(), "initialize mailbox row marker"))?;
    ids.insert(&[][..], 0)
        .map_err(|e| classify_redb_error(e.into(), "initialize mailbox ID marker"))?;
    queued
        .insert(0, 0)
        .map_err(|e| classify_redb_error(e.into(), "initialize mailbox queue marker"))?;
    Ok(())
}

pub(super) fn check_markers(
    rows: &impl ReadableTable<u64, &'static [u8]>,
    ids: &impl ReadableTable<&'static [u8], u64>,
    queue: &impl ReadableTable<u64, u64>,
) -> Result<(), MailboxFailure> {
    if !rows
        .get(0)
        .map_err(|e| classify_redb_error(e.into(), "read mailbox row marker"))?
        .is_some_and(|v| v.value().is_empty())
        || ids
            .get(&[][..])
            .map_err(|e| classify_redb_error(e.into(), "read mailbox ID marker"))?
            .is_none_or(|v| v.value() != 0)
        || queue
            .get(0)
            .map_err(|e| classify_redb_error(e.into(), "read mailbox queue marker"))?
            .is_none_or(|v| v.value() != 0)
    {
        return Err(MailboxFailure::reset_required(
            "missing mailbox table marker",
        ));
    }
    Ok(())
}

pub(super) fn queued_generation(record: &StoredRecord) -> Option<u64> {
    match record.record_kind {
        StoredRecordKind::Outbound {
            generation,
            state: StoredOutboundState::Queued,
            ..
        } => Some(generation),
        _ => None,
    }
}

fn terminal_candidates(record: &StoredRecord) -> Vec<StoredRecord> {
    let mut candidates = Vec::new();
    match record.record_kind {
        StoredRecordKind::Inbound { .. } => {
            for verification in [
                StoredVerification::Verified,
                StoredVerification::SourceUnknown,
                StoredVerification::InvalidSignature,
            ] {
                let mut candidate = record.clone();
                candidate.record_kind = StoredRecordKind::Inbound {
                    verification,
                    verification_upgraded: true,
                };
                candidates.push(candidate);
            }
        }
        StoredRecordKind::Outbound { .. } => {
            for state in [
                StoredOutboundState::Queued,
                StoredOutboundState::Delivered {
                    delivered_at_millis: u64::MAX,
                    rtt_millis: RequiredNullableU64(Some(u64::MAX)),
                },
                StoredOutboundState::Failed {
                    last_failure: StoredFailure::DeliveryTimedOut,
                },
                StoredOutboundState::Failed {
                    last_failure: StoredFailure::LocalNodeStopped,
                },
                StoredOutboundState::Cancelled {
                    cancelled_at_millis: u64::MAX,
                },
            ] {
                let mut candidate = record.clone();
                candidate.record_kind = StoredRecordKind::Outbound {
                    generation: u64::MAX,
                    failed_attempts: u64::MAX,
                    state,
                };
                candidates.push(candidate);
            }
        }
    }
    candidates
}

pub(super) fn maximum_encoded_bytes() -> Result<usize, MailboxFailure> {
    // JSON u8 arrays cost at most four characters per element. Title+content
    // together occupy at most one bounded wire; exact_wire is one further wire.
    // Derive fixed keys/state/discriminants from the owning serializer itself.
    let mut empty = StoredRecord {
        message_id: [255; 32],
        source: [255; 16],
        destination: [255; 16],
        timestamp_unix_ms: u64::MAX,
        title: Vec::new(),
        content: Vec::new(),
        exact_wire: Vec::new(),
        record_kind: StoredRecordKind::Outbound {
            generation: 0,
            failed_attempts: 0,
            state: StoredOutboundState::Queued,
        },
    };
    let mut maximum = 0;
    for incoming in [false, true] {
        if incoming {
            empty.record_kind = StoredRecordKind::Inbound {
                verification: StoredVerification::InvalidSignature,
                verification_upgraded: true,
            };
        }
        for candidate in terminal_candidates(&empty) {
            let bytes = serde_json::to_vec(&candidate)
                .map_err(|e| MailboxFailure::unavailable(format!("encode mailbox bound: {e}")))?;
            maximum = maximum.max(bytes.len());
        }
    }
    maximum
        .checked_add(8 * MAX_BASIC_LXMF_WIRE_BYTES)
        .ok_or(MailboxFailure::Capacity)
}

pub(super) fn maximum_owned_row_bytes() -> usize {
    std::mem::size_of::<DurableLxmfMessage>() + 3 * MAX_BASIC_LXMF_WIRE_BYTES
}

pub(super) fn charge(record: &StoredRecord) -> Result<u64, MailboxFailure> {
    let mut normalized = record.clone();
    normalized.exact_wire.fill(255);
    let mut max = encode(&normalized)?.len();
    for candidate in terminal_candidates(&normalized) {
        max = max.max(encode(&candidate)?.len());
    }
    // Primary u64 key plus inbound ID (32+8) or queued-generation (8+8).
    let index = if matches!(record.record_kind, StoredRecordKind::Inbound { .. }) {
        40
    } else {
        16
    };
    u64::try_from(max + 8 + index).map_err(|_| MailboxFailure::Capacity)
}

pub(super) fn stage(
    write: &redb::WriteTransaction,
    id: u64,
    old: Option<&StoredRecord>,
    new: &StoredRecord,
) -> Result<(), MailboxFailure> {
    let mut meta = write
        .open_table(LXMF_META)
        .map_err(|e| classify_redb_error(e.into(), "open mailbox accounting"))?;
    let header = Header::read(&meta)?;
    if old.is_none() {
        let records = header
            .records
            .checked_add(1)
            .ok_or(MailboxFailure::Capacity)?;
        if id != records {
            return Err(MailboxFailure::reset_required(
                "mailbox record ID/count mismatch",
            ));
        }
        let bytes = header
            .accounted
            .checked_add(charge(new)?)
            .ok_or(MailboxFailure::Capacity)?;
        if records > header.limits.max_records || bytes > header.limits.max_accounted_bytes {
            return Err(MailboxFailure::Capacity);
        }
        meta.insert(RECORDS, records)
            .map_err(|e| classify_redb_error(e.into(), "advance mailbox record count"))?;
        meta.insert(ACCOUNTED, bytes)
            .map_err(|e| classify_redb_error(e.into(), "advance mailbox byte count"))?;
    } else if let Some(old) = old {
        if charge(old)? != charge(new)? {
            return Err(MailboxFailure::reset_required(
                "mutation changed immutable mailbox charge",
            ));
        }
    }

    let mut queue = write
        .open_table(QUEUED)
        .map_err(|e| classify_redb_error(e.into(), "open mailbox queue index"))?;
    let previous = queue
        .get(id)
        .map_err(|e| classify_redb_error(e.into(), "read mailbox queue index"))?
        .map(|v| v.value());
    if previous != old.and_then(queued_generation) {
        return Err(MailboxFailure::reset_required(
            "mailbox queued index mismatch",
        ));
    }
    if let Some(generation) = queued_generation(new) {
        queue
            .insert(id, generation)
            .map_err(|e| classify_redb_error(e.into(), "index queued message"))?;
    } else {
        queue
            .remove(id)
            .map_err(|e| classify_redb_error(e.into(), "retire queued message index"))?;
    }
    Ok(())
}

pub(super) fn encode(record: &StoredRecord) -> Result<Vec<u8>, MailboxFailure> {
    struct Capped {
        bytes: Vec<u8>,
        max: usize,
    }
    impl Write for Capped {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.len() > self.max - self.bytes.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "mailbox encoded row bound",
                ));
            }
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let max = maximum_encoded_bytes()?;
    let mut writer = Capped {
        bytes: Vec::with_capacity(max),
        max,
    };
    serde_json::to_writer(&mut writer, record)
        .map_err(|e| MailboxFailure::unavailable(format!("encode bounded mailbox row: {e}")))?;
    Ok(writer.bytes)
}

pub(super) fn bytes<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    struct Bytes;
    impl<'de> Visitor<'de> for Bytes {
        type Value = Vec<u8>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bounded LXMF byte array")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
            let mut bytes = Vec::with_capacity(MAX_BASIC_LXMF_WIRE_BYTES);
            while let Some(byte) = seq.next_element::<u8>()? {
                if bytes.len() == MAX_BASIC_LXMF_WIRE_BYTES {
                    return Err(de::Error::custom("LXMF byte array exceeds wire bound"));
                }
                bytes.push(byte);
            }
            Ok(bytes)
        }
    }
    deserializer.deserialize_seq(Bytes)
}

pub(super) fn read(read: &redb::ReadTransaction) -> Result<Header, MailboxFailure> {
    let meta = read
        .open_table(LXMF_META)
        .map_err(|e| classify_redb_error(e.into(), "open mailbox metadata"))?;
    let header = Header::read(&meta)?;
    let rows = read
        .open_table(LXMF_MESSAGES)
        .map_err(|e| classify_redb_error(e.into(), "open mailbox rows"))?;
    let ids = read
        .open_table(LXMF_INBOUND_IDS)
        .map_err(|e| classify_redb_error(e.into(), "open mailbox IDs"))?;
    let queue = read
        .open_table(QUEUED)
        .map_err(|e| classify_redb_error(e.into(), "open mailbox queue"))?;
    check_markers(&rows, &ids, &queue)?;
    Ok(header)
}

pub(super) fn page_capacity(
    limits: MailboxLimits,
    requested: usize,
) -> Result<usize, MailboxFailure> {
    if requested > limits.max_page_rows as usize {
        return Err(MailboxFailure::Capacity);
    }
    let bytes = usize::try_from(limits.max_page_bytes).map_err(|_| MailboxFailure::Capacity)?;
    let capacity =
        (bytes - std::mem::size_of::<Vec<DurableLxmfMessage>>()) / maximum_owned_row_bytes();
    Ok(requested.min(capacity))
}

/// Account returned allocation capacities, independently of encoded scratch.
pub(super) fn check_page(
    limits: MailboxLimits,
    messages: &Vec<DurableLxmfMessage>,
) -> Result<(), MailboxFailure> {
    let slots = messages
        .capacity()
        .checked_mul(std::mem::size_of::<DurableLxmfMessage>())
        .and_then(|n| n.checked_add(std::mem::size_of::<Vec<DurableLxmfMessage>>()))
        .ok_or(MailboxFailure::Capacity)?;
    let bytes = messages
        .iter()
        .try_fold(slots, |total, row| {
            total
                .checked_add(row.title.capacity())
                .and_then(|n| n.checked_add(row.content.capacity()))
                .and_then(|n| n.checked_add(row.exact_wire.capacity()))
        })
        .ok_or(MailboxFailure::Capacity)?;
    if u64::try_from(bytes).map_err(|_| MailboxFailure::Capacity)? > limits.max_page_bytes {
        return Err(MailboxFailure::Capacity);
    }
    Ok(())
}
pub(super) fn allocate_page(
    limits: MailboxLimits,
    capacity: usize,
) -> Result<Vec<DurableLxmfMessage>, MailboxFailure> {
    let mut rows = Vec::new();
    rows.try_reserve_exact(capacity)
        .map_err(|_| MailboxFailure::Capacity)?;
    check_page(limits, &rows)?;
    Ok(rows)
}

pub(super) fn inspect(read_tx: &redb::ReadTransaction) -> Result<(), MailboxFailure> {
    let header = read(read_tx)?;
    let meta = read_tx
        .open_table(LXMF_META)
        .map_err(|e| classify_redb_error(e.into(), "inspect mailbox metadata"))?;
    let rows = read_tx
        .open_table(LXMF_MESSAGES)
        .map_err(|e| classify_redb_error(e.into(), "inspect mailbox rows"))?;
    let ids = read_tx
        .open_table(LXMF_INBOUND_IDS)
        .map_err(|e| classify_redb_error(e.into(), "inspect mailbox IDs"))?;
    let queue = read_tx
        .open_table(QUEUED)
        .map_err(|e| classify_redb_error(e.into(), "inspect mailbox queue"))?;
    let (mut count, mut accounted, mut revision, mut incoming, mut queued) =
        (0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
    for item in rows
        .range(1..)
        .map_err(|e| classify_redb_error(e.into(), "inspect mailbox range"))?
    {
        let (k, v) = item.map_err(|e| classify_redb_error(e.into(), "inspect mailbox row"))?;
        let id = k.value();
        count = count
            .checked_add(1)
            .ok_or_else(|| MailboxFailure::reset_required("mailbox count overflow"))?;
        if id != count {
            return Err(MailboxFailure::reset_required("mailbox record ID sequence"));
        }
        let row = decode_record(id, v.value())?;
        accounted = accounted
            .checked_add(charge(&row)?)
            .ok_or_else(|| MailboxFailure::reset_required("mailbox accounting overflow"))?;
        revision = revision
            .checked_add(record_mutation_count(id, &row.record_kind)?)
            .ok_or_else(|| MailboxFailure::reset_required("mailbox revision overflow"))?;
        if matches!(row.record_kind, StoredRecordKind::Inbound { .. }) {
            incoming += 1;
            if ids
                .get(row.message_id.as_slice())
                .map_err(|e| classify_redb_error(e.into(), "inspect mailbox ID"))?
                .map(|v| v.value())
                != Some(id)
            {
                return Err(MailboxFailure::reset_required(
                    "mailbox inbound index association",
                ));
            }
        }
        let expected = queued_generation(&row);
        if queue
            .get(id)
            .map_err(|e| classify_redb_error(e.into(), "inspect queued generation"))?
            .map(|v| v.value())
            != expected
        {
            return Err(MailboxFailure::reset_required(
                "queued generation association",
            ));
        }
        queued += u64::from(expected.is_some());
    }
    if count != header.records
        || accounted != header.accounted
        || required_meta(&meta, NEXT_LXMF_RECORD_ID_KEY)?
            != count
                .checked_add(1)
                .ok_or_else(|| MailboxFailure::reset_required("mailbox ID overflow"))?
        || required_meta(&meta, LXMF_REVISION_KEY)? != revision
    {
        return Err(MailboxFailure::reset_required(
            "mailbox header accounting mismatch",
        ));
    }
    let indexed = ids
        .len()
        .map_err(|e| classify_redb_error(e.into(), "count mailbox IDs"))?;
    let queued_count = queue
        .len()
        .map_err(|e| classify_redb_error(e.into(), "count mailbox queued index"))?;
    // Every primary row already proved its exact secondary key. Cardinalities
    // (including each reserved marker) reject orphan/duplicate secondary keys.
    if indexed != incoming + 1 || queued_count != queued + 1 {
        return Err(MailboxFailure::reset_required("orphan mailbox index"));
    }
    Ok(())
}
