use super::schema::*;
use crate::{enrollment::Enrollment, Provisioned};
use prns_groups_core::{Endpoint, EndpointId, EndpointIncarnation, GroupId, RecordId};
use prns_groups_service::{
    projection::{self, RecordWrite},
    Limits as ServiceLimits,
};
use prns_groups_store::{Mutation, OperationRejection, OperationTicket};
use prns_groups_store_redb::{CommitContext, CommitRead, RedbError, TransactionParticipant};
use prns_host::IdentityHash;
use redb::{ReadableTable, WriteTransaction};
use zeroize::Zeroizing;

/// Immutable authenticated context, copied without retaining private MLS keys.
/// Construct from the owning provisioning path, never foreign/unverified input.
pub struct Participant {
    pub(super) identity: IdentityHash,
    pub(super) context: CommitContext,
    pub(super) group: GroupId,
    pub(super) local: Endpoint,
    pub(super) authors: Vec<(Endpoint, IdentityHash)>,
    pub(super) limits: ServiceLimits,
    pub(super) max_changes: usize,
    pub(super) managed: bool,
    pub(super) store_limits: prns_groups_store_redb::RedbLimits,
}
impl Participant {
    pub fn new(cfg: &Provisioned) -> Result<Self, RedbError> {
        let binding = cfg.identity.binding();
        let local = Endpoint {
            id: EndpointId(binding.endpoint),
            incarnation: EndpointIncarnation::new(binding.incarnation)
                .map_err(|_| RedbError::Configuration)?,
        };
        let managed = cfg.owner_identity.is_some();
        let mut authors = Vec::new();
        authors
            .try_reserve_exact(
                cfg.peers
                    .len()
                    .checked_add(if managed { 0 } else { cfg.history_peers.len() })
                    .ok_or(RedbError::Capacity)?,
            )
            .map_err(|_| RedbError::Allocation)?;
        authors.extend(
            cfg.peers
                .iter()
                .chain(cfg.history_peers.iter().filter(|_| !managed))
                .map(|peer| (peer.endpoint, peer.identity)),
        );
        Ok(Self {
            identity: cfg.local_reticulum_identity,
            context: CommitContext {
                namespace: cfg.namespace,
                incarnation: cfg.incarnation,
            },
            group: cfg.group,
            local,
            authors,
            limits: cfg.service_limits,
            max_changes: cfg.store_limits.transaction_limits()?.max_changes,
            managed,
            store_limits: cfg.store_limits,
        })
    }
    fn validate_context(&self, context: CommitContext) -> Result<(), RedbError> {
        if context != self.context {
            return Err(RedbError::ForeignIncarnation);
        }
        Ok(())
    }
    fn live_in(&self, tx: &WriteTransaction) -> Result<bool, RedbError> {
        if !self.managed {
            return Ok(true);
        }
        Enrollment::archive_binding_in(
            tx,
            self.identity,
            self.group,
            self.context.namespace,
            self.context.incarnation,
        )
        .map_err(|error| match error {
            crate::enrollment::EnrollmentError::Storage(error) => error,
            crate::enrollment::EnrollmentError::Corrupt => RedbError::Corrupt,
            _ => RedbError::Configuration,
        })
    }
    fn author(
        &self,
        values: &CommitRead<'_>,
        endpoint: Endpoint,
    ) -> Result<IdentityHash, RedbError> {
        if endpoint == self.local {
            return Ok(self.identity);
        }
        let mut matches = self
            .authors
            .iter()
            .filter(|(e, _)| *e == endpoint)
            .map(|(_, id)| *id);
        let identity = matches.next();
        if matches.any(|other| Some(other) != identity) {
            return Err(RedbError::Corrupt);
        }
        match identity {
            Some(identity) => Ok(identity),
            None if self.managed => {
                crate::enrollment::historical_author(values, self.store_limits, endpoint)
                    .map_err(|error| match error {
                        crate::enrollment::EnrollmentError::Storage(error) => error,
                        crate::enrollment::EnrollmentError::Capacity => RedbError::Capacity,
                        _ => RedbError::Corrupt,
                    })?
                    .ok_or(RedbError::Corrupt)
            }
            None => Err(RedbError::Corrupt),
        }
    }
}
fn projection_error(error: prns_groups_service::Fault) -> RedbError {
    match error {
        prns_groups_service::Fault::Allocation => RedbError::Allocation,
        prns_groups_service::Fault::Capacity => RedbError::Capacity,
        _ => RedbError::Corrupt,
    }
}
impl TransactionParticipant for Participant {
    fn stage(
        &self,
        tx: &WriteTransaction,
        context: CommitContext,
        values: &CommitRead<'_>,
        _ticket: OperationTicket,
        changes: &[Mutation],
    ) -> Result<Option<OperationRejection>, RedbError> {
        self.validate_context(context)?;
        if !self.live_in(tx)? {
            return Ok(Some(OperationRejection::ForeignIncarnation));
        }
        let mut usage = usage_in(tx, self.identity)?;
        let projected = projection::records(changes, self.max_changes, self.limits)
            .map_err(projection_error)?;
        let mut records = tx.open_table(RECORDS).map_err(db)?;
        let mut positions = tx.open_table(POSITIONS).map_err(db)?;
        for item in projected {
            match item {
                RecordWrite::Insert(insert) => {
                    let record = &insert.record;
                    if record.metadata.group != self.group
                        || insert.authored != (record.metadata.author == self.local)
                    {
                        return Err(RedbError::Corrupt);
                    }
                    let author = self.author(values, record.metadata.author)?;
                    let key = record_key(context, self.group, record.metadata.record);
                    let position = position_key(context, self.group, insert.position.0);
                    if let Some(old) = records.get(key.as_slice()).map_err(db)? {
                        let old = parse_row(old.value())?;
                        if old.record != insert.encoded
                            || old.author != author
                            || old.authored != insert.authored
                            || old.position != insert.position.0
                            || old.flags.len() != record.targets.len()
                        {
                            return Err(RedbError::Corrupt);
                        }
                        if positions
                            .get(position.as_slice())
                            .map_err(db)?
                            .is_none_or(|v| v.value() != record.metadata.record.0)
                        {
                            return Err(RedbError::Corrupt);
                        }
                        continue;
                    }
                    if positions.get(position.as_slice()).map_err(db)?.is_some() {
                        return Err(RedbError::Corrupt);
                    }
                    let size = ROW_FIXED
                        .checked_add(record.targets.len())
                        .and_then(|n| n.checked_add(insert.encoded.len()))
                        .ok_or(RedbError::Capacity)?;
                    if size > MAX_ROW_BYTES {
                        return Ok(Some(OperationRejection::StorageCapacity));
                    }
                    let count = usage
                        .records
                        .checked_add(1)
                        .ok_or(RedbError::CounterExhausted)?;
                    let bytes = usage
                        .bytes
                        .checked_add(charge(size)?)
                        .ok_or(RedbError::CounterExhausted)?;
                    if count > usage.limits.max_records || bytes > usage.limits.max_bytes {
                        return Ok(Some(OperationRejection::StorageCapacity));
                    }
                    let mut row = Zeroizing::new(Vec::new());
                    row.try_reserve_exact(size)
                        .map_err(|_| RedbError::Allocation)?;
                    row.extend_from_slice(ROW_MAGIC);
                    row.extend_from_slice(author.as_bytes());
                    row.extend_from_slice(&insert.position.0.to_be_bytes());
                    row.push(u8::from(insert.authored));
                    row.extend_from_slice(
                        &u32::try_from(insert.encoded.len())
                            .map_err(|_| RedbError::Capacity)?
                            .to_be_bytes(),
                    );
                    row.extend_from_slice(
                        &u32::try_from(record.targets.len())
                            .map_err(|_| RedbError::Capacity)?
                            .to_be_bytes(),
                    );
                    row.resize(ROW_FIXED + record.targets.len(), 0);
                    row.extend_from_slice(insert.encoded);
                    records.insert(key.as_slice(), row.as_slice()).map_err(db)?;
                    positions
                        .insert(position.as_slice(), record.metadata.record.0.as_slice())
                        .map_err(db)?;
                    usage.records = count;
                    usage.bytes = bytes;
                }
                RecordWrite::Receipt(receipt) => {
                    if receipt.group != self.group {
                        return Err(RedbError::Corrupt);
                    }
                    let key = record_key(context, self.group, receipt.record);
                    let updated = {
                        let old = records
                            .get(key.as_slice())
                            .map_err(db)?
                            .ok_or(RedbError::Corrupt)?;
                        let parsed = parse_row(old.value())?;
                        let record = projection::decode_record(parsed.record, self.limits)
                            .map_err(projection_error)?;
                        if record.metadata.group != self.group
                            || record.metadata.record != receipt.record
                            || parsed.flags.len() != record.targets.len()
                        {
                            return Err(RedbError::Corrupt);
                        }
                        let target = record
                            .targets
                            .iter()
                            .position(|endpoint| *endpoint == receipt.target)
                            .ok_or(RedbError::Corrupt)?;
                        if parsed.flags[target] == 1 {
                            continue;
                        }
                        let mut row = Zeroizing::new(Vec::new());
                        row.try_reserve_exact(old.value().len())
                            .map_err(|_| RedbError::Allocation)?;
                        row.extend_from_slice(old.value());
                        row[ROW_FIXED + target] = 1;
                        row
                    };
                    records
                        .insert(key.as_slice(), updated.as_slice())
                        .map_err(db)?;
                }
            }
        }
        drop(records);
        drop(positions);
        save(tx, self.identity, usage)?;
        Ok(None)
    }
    fn delete_in(&self, tx: &WriteTransaction, context: CommitContext) -> Result<(), RedbError> {
        self.validate_context(context)?;
        if self.managed && self.live_in(tx)? {
            return Err(RedbError::Configuration);
        }
        delete_rows(tx, self.identity, self.group, context)
    }
}

/// Deletion needs the exact retired catalog binding, not retained private keys
/// or a reconstructed provisioning object. It cannot stage fresh writes.
pub(super) struct Purge {
    pub identity: IdentityHash,
    pub group: GroupId,
    pub context: CommitContext,
}
impl TransactionParticipant for Purge {
    fn stage(
        &self,
        _: &WriteTransaction,
        _: CommitContext,
        _: &CommitRead<'_>,
        _: OperationTicket,
        _: &[Mutation],
    ) -> Result<Option<OperationRejection>, RedbError> {
        Ok(Some(OperationRejection::ForeignIncarnation))
    }
    fn delete_in(&self, tx: &WriteTransaction, context: CommitContext) -> Result<(), RedbError> {
        if context != self.context {
            return Err(RedbError::ForeignIncarnation);
        }
        match Enrollment::archive_binding_in(
            tx,
            self.identity,
            self.group,
            context.namespace,
            context.incarnation,
        ) {
            Ok(false) => delete_rows(tx, self.identity, self.group, context),
            Ok(true) => Err(RedbError::Configuration),
            Err(crate::enrollment::EnrollmentError::Storage(error)) => Err(error),
            Err(crate::enrollment::EnrollmentError::Corrupt) => Err(RedbError::Corrupt),
            Err(_) => Err(RedbError::Configuration),
        }
    }
}
fn delete_rows(
    tx: &WriteTransaction,
    identity: IdentityHash,
    group: GroupId,
    context: CommitContext,
) -> Result<(), RedbError> {
    let mut usage = usage_in(tx, identity)?;
    let mut records = tx.open_table(RECORDS).map_err(db)?;
    let mut positions = tx.open_table(POSITIONS).map_err(db)?;
    let first = record_key(context, group, RecordId([0; 32]));
    let last = record_key(context, group, RecordId([255; 32]));
    loop {
        let next = {
            let mut range = records
                .range(first.as_slice()..=last.as_slice())
                .map_err(db)?;
            match range.next().transpose().map_err(db)? {
                None => None,
                Some((key, value)) => {
                    let parsed = parse_row(value.value())?;
                    Some((
                        <[u8; 104]>::try_from(key.value()).map_err(|_| RedbError::Corrupt)?,
                        parsed.position,
                        charge(value.value().len())?,
                    ))
                }
            }
        };
        let Some((key, position, bytes)) = next else {
            break;
        };
        let index = position_key(context, group, position);
        if positions
            .remove(index.as_slice())
            .map_err(db)?
            .is_none_or(|value| value.value() != &key[72..])
        {
            return Err(RedbError::Corrupt);
        }
        records.remove(key.as_slice()).map_err(db)?;
        usage.records = usage.records.checked_sub(1).ok_or(RedbError::Corrupt)?;
        usage.bytes = usage.bytes.checked_sub(bytes).ok_or(RedbError::Corrupt)?;
    }
    drop(records);
    drop(positions);
    save(tx, identity, usage)
}
