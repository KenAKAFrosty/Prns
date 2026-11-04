//! Archive-backed projection into the ordinary canonical record API. Protocol
//! feed pruning does not affect history. Flags mean exactly the durable target
//! receipt observed locally; for a received record, ReceiptQueued proves this
//! local target's durability and says nothing about other targets' delivery.
use super::schema::*;
use crate::{
    ChangeToken, Error, Provisioned, RecordCursor, RecordDeliveryProgress, RecordPage,
    RecordSelection, StoredRecord,
};
use prns_groups_core::{
    ClockReading, DeliveryProgress, ReceiptStatus, RecordId, RemovalStatus, TargetProgress,
};
use prns_groups_store_redb::CommitContext;
use redb::{Database, ReadableDatabase};

fn context(cfg: &Provisioned) -> CommitContext {
    CommitContext {
        namespace: cfg.namespace,
        incarnation: cfg.incarnation,
    }
}
fn project(
    cfg: &Provisioned,
    bytes: &[u8],
    id: RecordId,
    clock: ClockReading,
) -> Result<StoredRecord, Error> {
    let row = parse_row(bytes)?;
    let record = decode_record(row.record)?;
    if record.metadata.group != cfg.group
        || record.metadata.record != id
        || row.flags.len() != record.targets.len()
    {
        return Err(Error::Corrupt);
    }
    let binding = cfg.identity.binding();
    let authored = record.metadata.author.id.0 == binding.endpoint
        && record.metadata.author.incarnation.get() == binding.incarnation;
    if row.authored != authored || (authored && row.author != cfg.local_reticulum_identity) {
        return Err(Error::Corrupt);
    }
    let mut targets = targets(cfg, &record, row.flags)?;
    let summary = DeliveryProgress::new(&record.metadata, &mut targets)
        .map_err(|_| Error::Corrupt)?
        .summary(clock, cfg.service_limits.future_skew);
    let progress = RecordDeliveryProgress {
        membership_changed: (record.metadata.original_control.era != cfg.control.era
            || cfg.lifecycle != crate::GroupLifecycle::Active)
            && !summary.all_received(),
        summary,
    };
    Ok(StoredRecord {
        client_request_id: row.authored.then_some(crate::RequestId(record.nonce)),
        record,
        position: ChangeToken {
            group: cfg.group,
            incarnation: cfg.incarnation,
            position: row.position,
        },
        author_identity: row.author,
        outgoing: row.authored,
        progress,
    })
}

fn targets(
    cfg: &Provisioned,
    record: &prns_groups_service::Record,
    flags: &[u8],
) -> Result<Vec<TargetProgress>, Error> {
    let mut targets = Vec::new();
    targets
        .try_reserve_exact(record.targets.len())
        .map_err(|_| Error::Storage(prns_groups_store_redb::RedbError::Allocation))?;
    for (target, flag) in record.targets.iter().zip(flags) {
        targets.push(TargetProgress::restore(
            *target,
            if *flag == 1 {
                ReceiptStatus::Received
            } else {
                ReceiptStatus::Awaiting
            },
            if cfg.owner_identity.is_some() && !cfg.readers.contains(target) {
                RemovalStatus::Removed
            } else {
                RemovalStatus::NotKnownRemoved
            },
        ));
    }
    Ok(targets)
}

pub(crate) fn delivery_targets(
    database: &Database,
    cfg: &Provisioned,
    id: RecordId,
) -> Result<Option<Vec<TargetProgress>>, Error> {
    read_record(database, cfg, id, |bytes| {
        let row = parse_row(bytes)?;
        let record = decode_record(row.record)?;
        if record.metadata.group != cfg.group
            || record.metadata.record != id
            || row.flags.len() != record.targets.len()
        {
            return Err(Error::Corrupt);
        }
        targets(cfg, &record, row.flags)
    })
}

pub(crate) fn record(
    database: &Database,
    cfg: &Provisioned,
    id: RecordId,
    clock: ClockReading,
) -> Result<Option<StoredRecord>, Error> {
    read_record(database, cfg, id, |bytes| project(cfg, bytes, id, clock))
}

fn read_record<T>(
    database: &Database,
    cfg: &Provisioned,
    id: RecordId,
    inspect: impl FnOnce(&[u8]) -> Result<T, Error>,
) -> Result<Option<T>, Error> {
    let read = database.begin_read().map_err(db)?;
    read_usage_in(&read, cfg.local_reticulum_identity)?.ok_or(Error::Configuration)?;
    let records = read.open_table(RECORDS).map_err(db)?;
    let key = record_key(context(cfg), cfg.group, id);
    let Some(value) = records.get(key.as_slice()).map_err(db)? else {
        return Ok(None);
    };
    let row = parse_row(value.value())?;
    let index = position_key(context(cfg), cfg.group, row.position);
    if read
        .open_table(POSITIONS)
        .map_err(db)?
        .get(index.as_slice())
        .map_err(db)?
        .is_none_or(|value| value.value() != id.0)
    {
        return Err(Error::Corrupt);
    }
    inspect(value.value()).map(Some)
}

pub(crate) fn page(
    database: &Database,
    cfg: &Provisioned,
    before: Option<RecordCursor>,
    limit: usize,
    clock: ClockReading,
    selection: RecordSelection,
    head: u64,
) -> Result<RecordPage, Error> {
    if limit == 0
        || limit > 100
        || before.is_some_and(|cursor| {
            cursor.group != cfg.group
                || cursor.incarnation != cfg.incarnation
                || cursor.before.is_some_and(|position| position > head)
        })
    {
        return Err(Error::InvalidInput);
    }
    let read = database.begin_read().map_err(db)?;
    read_usage_in(&read, cfg.local_reticulum_identity)?.ok_or(Error::Configuration)?;
    let rows = read.open_table(RECORDS).map_err(db)?;
    let positions = read.open_table(POSITIONS).map_err(db)?;
    let first = position_key(context(cfg), cfg.group, 0);
    // Inclusive upper bound avoids overflowing a maximal feed position.
    let last_position = match before.and_then(|cursor| cursor.before) {
        Some(0) => return Err(Error::InvalidInput),
        Some(position) => position - 1,
        None => head,
    };
    let last = position_key(context(cfg), cfg.group, last_position);
    let mut records = Vec::new();
    records
        .try_reserve_exact(limit)
        .map_err(|_| Error::Storage(prns_groups_store_redb::RedbError::Allocation))?;
    let mut previous = None;
    let mut next_before = None;
    // Bound inspected rows, not just matching rows: a text filter cannot scan
    // a million opaque records in one owner job. Empty pages can have a cursor.
    for (scanned, entry) in positions
        .range(first.as_slice()..=last.as_slice())
        .map_err(db)?
        .rev()
        .take(limit + 1)
        .enumerate()
    {
        let (key, value) = entry.map_err(db)?;
        if scanned == limit {
            next_before = previous.map(|position| RecordCursor {
                group: cfg.group,
                incarnation: cfg.incarnation,
                before: Some(position),
            });
            break;
        }
        let position = u64::from_be_bytes(
            key.value()
                .get(72..)
                .ok_or(Error::Corrupt)?
                .try_into()
                .map_err(|_| Error::Corrupt)?,
        );
        let id = RecordId(value.value().try_into().map_err(|_| Error::Corrupt)?);
        let record_key = record_key(context(cfg), cfg.group, id);
        let value = rows
            .get(record_key.as_slice())
            .map_err(db)?
            .ok_or(Error::Corrupt)?;
        let item = project(cfg, value.value(), id, clock)?;
        if item.position.position != position {
            return Err(Error::Corrupt);
        }
        previous = Some(position);
        if selection.includes(&item.record) {
            records.push(item);
        }
    }
    Ok(RecordPage {
        records,
        visited_heads: vec![ChangeToken {
            group: cfg.group,
            incarnation: cfg.incarnation,
            position: head,
        }],
        current_incarnation: cfg.incarnation,
        next_before,
    })
}
