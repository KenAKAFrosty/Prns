//! Bounded views of this service's already validated staged application writes.
//!
//! This is storage composition, not authentication or a new wire/event format.
//! Native transaction participants use these owning codecs rather than copying
//! private service keys. No redb, runtime, text profile or archive is required.
use crate::{
    codec::{self, Packet},
    storage, ChangeCursor, ChangeKind, DurableReceipt, Fault, Limits, Record,
};
use alloc::vec::Vec;
use prns_groups_mls::application_key;
use prns_groups_store::Mutation;

pub struct Insertion<'a> {
    pub record: Record,
    /// Exact canonical encoding already staged by the owning service.
    pub encoded: &'a [u8],
    pub position: ChangeCursor,
    pub authored: bool,
}
pub enum RecordWrite<'a> {
    Insert(Insertion<'a>),
    Receipt(DurableReceipt),
}

/// Decode the canonical stored record without rebuilding its field inventory.
/// The result is not authenticated by this decoder alone.
pub fn decode_record(bytes: &[u8], limits: Limits) -> Result<Record, Fault> {
    match codec::decode(bytes, limits)? {
        Packet::Record(record) => Ok(record),
        _ => Err(Fault::Corrupt),
    }
}

/// Inspect at most max_changes staged writes. Allocation is bounded by that
/// count and the owning service's record/target/body limits. Deletions intentionally
/// produce no archive removal; only the domain's explicit deletion may do that.
pub fn records(
    changes: &[Mutation],
    max_changes: usize,
    limits: Limits,
) -> Result<Vec<RecordWrite<'_>>, Fault> {
    if changes.len() > max_changes {
        return Err(Fault::Capacity);
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(changes.len())
        .map_err(|_| Fault::Allocation)?;
    for (index, mutation) in changes.iter().enumerate() {
        if changes[..index]
            .iter()
            .any(|other| other.key() == mutation.key())
        {
            return Err(Fault::Corrupt);
        }
        let Some(key) = application_key(mutation.key()) else {
            continue;
        };
        let Some(value) = mutation.value() else {
            continue;
        };
        if key.first() != Some(&b'F') {
            continue;
        }
        if key.len() != 41 {
            return Err(Fault::Corrupt);
        }
        let change = storage::decode_change(value)?;
        let group = crate::GroupId(key[1..33].try_into().map_err(|_| Fault::Corrupt)?);
        if change.cursor.0 == 0 || storage::feed_key(group, change.cursor).as_slice() != key {
            return Err(Fault::Corrupt);
        }
        match change.kind {
            ChangeKind::Published(id) | ChangeKind::Received(id) => {
                let record_key = storage::record_key(b'r', group, id);
                let encoded = changes
                    .iter()
                    .find_map(|mutation| {
                        (application_key(mutation.key()) == Some(record_key.as_slice()))
                            .then(|| mutation.value())
                            .flatten()
                    })
                    .ok_or(Fault::Corrupt)?;
                let record = decode_record(encoded, limits)?;
                if record.metadata.group != group || record.metadata.record != id {
                    return Err(Fault::Corrupt);
                }
                if result.iter().any(|write| matches!(write, RecordWrite::Insert(old) if old.record.metadata.group == group && old.record.metadata.record == id)) { return Err(Fault::Corrupt); }
                result.push(RecordWrite::Insert(Insertion {
                    record,
                    encoded,
                    position: change.cursor,
                    authored: matches!(change.kind, ChangeKind::Published(_)),
                }));
            }
            ChangeKind::Receipt(receipt) | ChangeKind::ReceiptQueued(receipt) => {
                if receipt.group != group {
                    return Err(Fault::Corrupt);
                }
                result.push(RecordWrite::Receipt(receipt));
            }
            ChangeKind::Rejected { .. } => (),
        }
    }
    // A future/corrupt insertion shape must not silently bypass selected history.
    let inserted = changes
        .iter()
        .filter(|mutation| {
            mutation.value().is_some()
                && application_key(mutation.key()).is_some_and(|key| key.first() == Some(&b'r'))
        })
        .count();
    if inserted
        != result
            .iter()
            .filter(|write| matches!(write, RecordWrite::Insert(_)))
            .count()
    {
        return Err(Fault::Corrupt);
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
