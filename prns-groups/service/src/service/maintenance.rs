//! Expiry-only, bounded retirement. A record's deadline ends retention even when
//! some receipts are missing; retirement never manufactures a receipt.
use super::*;
use prns_groups_store::Revision;

const FLOOR: u8 = b'T';
const LAST: u8 = b'M';

/// An immutable plan produced from exclusively owned committed state. Limits
/// bound scans; caller budgets bound selected records and output retirements.
pub struct MaintenancePlan {
    group: GroupId,
    configuration: [u8; 32],
    revision: Revision,
    floor: Timestamp,
    old_floor: Option<Zeroizing<Vec<u8>>>,
    change_head: u64,
    inventory_head: u64,
    records: Vec<RecordId>,
    operations: Vec<OperationId>,
    facts: Vec<Fact>,
    bootstrap: Vec<OperationId>,
    migrate_reservations: bool,
    changes: Vec<Change>,
    envelopes: Vec<inventory::Retained>,
    tombstones: Vec<RecordId>,
}

impl MaintenancePlan {
    pub fn records(&self) -> &[RecordId] {
        &self.records
    }
    pub fn operations(&self) -> &[OperationId] {
        &self.operations
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
            && self.operations.is_empty()
            && self.bootstrap.is_empty()
            && !self.migrate_reservations
    }

    fn fingerprint(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"prns.groups.service.maintenance.v1\0");
        hash.update(self.group.0);
        hash.update(self.configuration);
        hash.update(self.revision.0.to_be_bytes());
        hash.update(self.floor.0.to_be_bytes());
        hash.update([u8::from(self.migrate_reservations)]);
        for id in &self.records {
            hash.update([1]);
            hash.update(id.0);
        }
        for id in &self.operations {
            hash.update([2]);
            hash.update(id.0);
        }
        for id in &self.bootstrap {
            hash.update([3]);
            hash.update(id.0);
        }
        hash.finalize().into()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaintenanceOutcome {
    pub commit: CommitReceipt,
    pub records_removed: usize,
    pub operations_retired: usize,
    pub envelopes_removed: usize,
    pub changes_removed: usize,
}

pub(super) fn tombstone(deadline: Timestamp) -> [u8; 9] {
    let mut bytes = [2; 9];
    bytes[1..].copy_from_slice(&deadline.0.to_be_bytes());
    bytes
}

pub(super) fn valid_tombstone(bytes: &[u8]) -> bool {
    bytes.len() == 9 && bytes[0] == 2
}

fn fact_record(fact: Fact) -> Option<RecordId> {
    match fact.result {
        ResultFact::Published(id)
        | ResultFact::ReceiptQueued(id)
        | ResultFact::Incoming(Incoming::Record(id)) => Some(id),
        ResultFact::Incoming(Incoming::Receipt(receipt)) => Some(receipt.record),
        ResultFact::Incoming(Incoming::Rejected(_)) => None,
    }
}

impl<B: AtomicBackend + Send> GroupService<'_, B> {
    pub fn expiry_floor(&self) -> ServiceResult<Timestamp, B> {
        Ok(Timestamp(
            self.read(&key(FLOOR, &self.group().0))?
                .map(|bytes| counter(&bytes))
                .transpose()?
                .unwrap_or(0),
        ))
    }

    pub(super) fn current_clock(&self, clock: ClockReading) -> ServiceResult<ClockReading, B> {
        let floor = self.expiry_floor()?;
        Ok(match clock {
            ClockReading::Trusted(interval) => ClockReading::Trusted(
                ClockInterval::new(interval.lower().max(floor), interval.upper().max(floor))
                    .map_err(|_| Fault::Corrupt)?,
            ),
            ClockReading::Unknown => ClockReading::Unknown,
        })
    }
    /// Includes MLS enrollment/control operations, which this service never
    /// retires. The native owner uses the surviving minimum for receipt fences.
    pub fn pending_operations(&self) -> ServiceResult<Vec<OperationId>, B> {
        self.adapter.pending_operations().map_err(Error::Security)
    }

    pub(super) fn effective_clock(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        clock: ClockReading,
    ) -> ServiceResult<ClockReading, B> {
        let Some(bytes) = tx.read(&key(FLOOR, &self.group().0))? else {
            return Ok(clock);
        };
        let floor = Timestamp(counter(&bytes)?);
        Ok(match clock {
            ClockReading::Trusted(interval) => ClockReading::Trusted(
                ClockInterval::new(interval.lower().max(floor), interval.upper().max(floor))
                    .map_err(|_| Fault::Corrupt)?,
            ),
            ClockReading::Unknown => ClockReading::Unknown,
        })
    }

    fn operation_record(
        &self,
        operation: OperationId,
        fact: Fact,
    ) -> ServiceResult<Option<RecordId>, B> {
        if let Some(record) = fact_record(fact) {
            return Ok(Some(record));
        }
        self.read(&key(b'z', &operation.0))?
            .map(|bytes| {
                bytes
                    .as_slice()
                    .try_into()
                    .map(RecordId)
                    .map_err(|_| Fault::Corrupt.into())
            })
            .transpose()
    }

    fn operation_deadline(
        &self,
        operation: OperationId,
        record: Option<RecordId>,
    ) -> ServiceResult<Option<Timestamp>, B> {
        if let Some(bytes) = self.read(&key(b'a', &operation.0))? {
            return Ok(Some(Timestamp(counter(&bytes)?)));
        }
        if let Some(id) = record {
            if let Some(record) = self.record(id)? {
                return Ok(Some(record.metadata.validity.deadline()));
            }
            if let Some(bytes) = self.read(&record_key(b'e', self.group(), id))? {
                if !valid_tombstone(&bytes) {
                    return Err(Fault::Corrupt.into());
                }
                return Ok(Some(Timestamp(counter(&bytes[1..])?)));
            }
        }
        Ok(None)
    }

    /// Only trusted, definitely expired records are selected. Prefer the earliest
    /// retained service output's record, then oldest creation time. Undelivered
    /// records remain intact before their deadline.
    /// Passing zero for one budget disables that class of work.
    pub fn maintenance_plan(
        &self,
        clock: ClockReading,
        max_records: usize,
        max_operations: usize,
    ) -> ServiceResult<MaintenancePlan, B> {
        self.check_configuration()?;
        if max_records > self.limits.max_records
            || max_operations
                > self
                    .limits
                    .max_envelopes
                    .saturating_add(self.limits.max_changes)
            || (max_records == 0 && max_operations == 0)
        {
            return Err(Fault::Capacity.into());
        }
        let ClockReading::Trusted(interval) = clock else {
            return Err(Fault::TimeUncertain.into());
        };
        let old_floor = self.read(&key(FLOOR, &self.group().0))?;
        // Rebuild missing reservation counters independently without creating
        // MLS output, before admission sums all groups' retained obligations.
        // Initialized idle stores do not write.
        let migrate_reservations = self.read(&key(b'g', &self.group().0))?.is_some()
            && self
                .read(&key(capacity::RESERVED, &self.group().0))?
                .is_none();
        let floor = Timestamp(
            interval.lower().0.max(
                old_floor
                    .as_ref()
                    .map(|b| counter(b))
                    .transpose()?
                    .unwrap_or(0),
            ),
        );
        let mut changes = Vec::new();
        let positions = self.positions(index::CHANGES, b'c', self.limits.max_changes)?;
        changes
            .try_reserve_exact(positions.len())
            .map_err(|_| Fault::Allocation)?;
        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(self.limits.max_records)
            .map_err(|_| Fault::Allocation)?;
        for position in positions {
            let bytes = self
                .read(&feed_key(self.group(), ChangeCursor(position)))?
                .ok_or(Fault::Corrupt)?;
            let change = decode_change(&bytes)?;
            if change.cursor.0 != position {
                return Err(Fault::Corrupt.into());
            }
            if let ChangeKind::Published(id) | ChangeKind::Received(id) = change.kind {
                let record = self.record(id)?.ok_or(Fault::Corrupt)?;
                if record.metadata.validity.deadline() <= floor {
                    candidates.push((record.metadata.validity.created_at(), id));
                }
            }
            changes.push(change);
        }
        let mut pending = self.pending_operations()?;
        // Facts use endpoint-wide operation IDs, while record/index keys belong
        // to one group. Never interpret another group's fact as a missing local
        // record or retire its recovery output when adapters share a backend.
        let mut retained = 0;
        for index in 0..pending.len() {
            let operation = pending[index];
            if self
                .adapter
                .output_context(operation)
                .map_err(Error::Security)?
                .is_some_and(|context| context.group == self.group())
            {
                pending[retained] = operation;
                retained += 1;
            }
        }
        pending.truncate(retained);
        // Releasing the earliest retained service output can also release a
        // coordinator's contiguous recovery-receipt prefix. Prefer its expired
        // record so bounded cleanup cannot get stuck behind newer retirements.
        let mut earliest = None;
        for operation in &pending {
            if let Some(bytes) = self.read(&key(b'o', &operation.0))? {
                earliest = self.operation_record(*operation, decode_fact(&bytes)?)?;
                break;
            }
        }
        candidates.sort_unstable_by_key(|(created, id)| (Some(*id) != earliest, *created, *id));
        let mut records = Vec::new();
        records
            .try_reserve_exact(max_records.min(candidates.len()))
            .map_err(|_| Fault::Allocation)?;
        records.extend(candidates.into_iter().take(max_records).map(|(_, id)| id));

        let mut operations = Vec::new();
        let mut facts = Vec::new();
        let mut bootstrap = Vec::new();
        let bound = max_operations.min(pending.len());
        operations
            .try_reserve_exact(bound)
            .map_err(|_| Fault::Allocation)?;
        facts
            .try_reserve_exact(bound)
            .map_err(|_| Fault::Allocation)?;
        bootstrap
            .try_reserve_exact(bound)
            .map_err(|_| Fault::Allocation)?;
        let mut remaining_records = Vec::new();
        remaining_records
            .try_reserve_exact(pending.len())
            .map_err(|_| Fault::Allocation)?;
        let mut possible_tombstones = Vec::new();
        possible_tombstones
            .try_reserve_exact(pending.len().saturating_add(records.len()))
            .map_err(|_| Fault::Allocation)?;
        possible_tombstones.extend(records.iter().copied());
        for operation in pending {
            let Some(bytes) = self.read(&key(b'o', &operation.0))? else {
                continue;
            };
            let fact = decode_fact(&bytes)?;
            let record = self.operation_record(operation, fact)?;
            let deadline = self.operation_deadline(operation, record)?;
            let selected = deadline.is_some_and(|deadline| deadline <= floor)
                && match record {
                    Some(id) => records.contains(&id) || self.record(id)?.is_none(),
                    None => true,
                }
                && operations.len() + bootstrap.len() < max_operations;
            if selected {
                operations.push(operation);
                facts.push(fact);
                if let Some(id) = record {
                    possible_tombstones.push(id);
                }
            } else {
                if let Some(id) = record {
                    remaining_records.push(id);
                }
                // Rejections received with an unknown clock have no deadline.
                // Establish one on the first bounded pass with a trusted clock;
                // never delete those facts immediately.
                if deadline.is_none() && operations.len() + bootstrap.len() < max_operations {
                    bootstrap.push(operation);
                }
            }
        }
        possible_tombstones.sort_unstable();
        possible_tombstones.dedup();
        possible_tombstones.retain(|id| !remaining_records.contains(id));
        changes.retain(|change| matches!(change.kind, ChangeKind::Published(id) | ChangeKind::Received(id) if records.contains(&id)) || operations.contains(&change.operation));
        let positions =
            self.positions(index::ENVELOPES, inventory::HEAD, self.limits.max_envelopes)?;
        let mut envelopes = Vec::new();
        envelopes
            .try_reserve_exact(operations.len())
            .map_err(|_| Fault::Allocation)?;
        for position in positions {
            let bytes = self
                .read(&inventory::sequence_key(
                    self.group(),
                    InventoryCursor(position),
                ))?
                .ok_or(Fault::Corrupt)?;
            let id = EnvelopeId(bytes.as_slice().try_into().map_err(|_| Fault::Corrupt)?);
            let retained = self.retained(id)?.ok_or(Fault::Corrupt)?;
            if retained.info.position.0 != position {
                return Err(Fault::Corrupt.into());
            }
            if retained.output.is_some_and(|op| operations.contains(&op))
                || facts
                    .iter()
                    .any(|f| matches!(f.result, ResultFact::Incoming(_)) && f.fingerprint == id.0)
            {
                envelopes.push(retained);
            }
        }
        Ok(MaintenancePlan {
            group: self.group(),
            configuration: self.configuration,
            revision: self.coordinator().head()?,
            floor,
            old_floor,
            change_head: self
                .read(&key(b'c', &self.group().0))?
                .map(|b| counter(&b))
                .transpose()?
                .unwrap_or(0),
            inventory_head: self
                .read(&key(inventory::HEAD, &self.group().0))?
                .map(|b| counter(&b))
                .transpose()?
                .unwrap_or(0),
            records,
            operations,
            facts,
            bootstrap,
            migrate_reservations,
            changes,
            envelopes,
            tombstones: possible_tombstones,
        })
    }

    /// Commit one prepared batch with its exact MLS outputs. After ambiguous
    /// commit, recover `operation` before invoking any further work. The last
    /// batch is replayable with the same plan; older result evidence may be gone.
    pub fn maintain(
        &mut self,
        operation: OperationId,
        plan: &MaintenancePlan,
    ) -> ServiceResult<MaintenanceOutcome, B> {
        if plan.group != self.group() || plan.configuration != self.configuration {
            return Err(Fault::ConfigurationChanged.into());
        }
        let fingerprint = plan.fingerprint();
        let mut marker = [0u8; 64];
        marker[..32].copy_from_slice(&operation.0);
        marker[32..].copy_from_slice(&fingerprint);
        if let Some(last) = self.read(&key(LAST, &self.group().0))? {
            if last.len() != 64 {
                return Err(Fault::Corrupt.into());
            }
            if last[..32] == operation.0 {
                if last.as_slice() != marker {
                    return Err(Fault::IdempotencyConflict.into());
                }
                return Ok(Self::maintenance_outcome(self.receipt(operation)?, plan));
            }
        }
        if self.coordinator().head()? != plan.revision {
            return Err(Fault::ConfigurationChanged.into());
        }
        if plan.is_empty() {
            return Err(Fault::Configuration.into());
        }
        let commit = self
            .adapter
            .maintain(operation, &plan.operations, |tx| {
                self.initialize(tx, operation)?;
                if counter(
                    &tx.read(&key(b'c', &self.group().0))?
                        .ok_or(Fault::Corrupt)?,
                )? != plan.change_head
                    || counter(
                        &tx.read(&key(inventory::HEAD, &self.group().0))?
                            .ok_or(Fault::Corrupt)?,
                    )? != plan.inventory_head
                    || tx.read(&key(FLOOR, &self.group().0))? != plan.old_floor
                {
                    return Err(Fault::ConfigurationChanged.into());
                }
                let mut count = counter(
                    &tx.read(&key(b'n', &self.group().0))?
                        .ok_or(Fault::Corrupt)?,
                )?;
                let mut reserved = self.reserved_receipts(tx)?;
                for id in &plan.records {
                    let bytes = tx
                        .read(&record_key(b'r', self.group(), *id))?
                        .ok_or(Fault::Corrupt)?;
                    let Packet::Record(record) = codec::decode(&bytes, self.limits)? else {
                        return Err(Fault::Corrupt.into());
                    };
                    if record.metadata.record != *id
                        || record.metadata.validity.deadline() > plan.floor
                    {
                        return Err(Fault::Corrupt.into());
                    }
                    let flags = tx
                        .read(&record_key(b'p', self.group(), *id))?
                        .ok_or(Fault::Corrupt)?;
                    validate_flags(&flags, record.targets.len())?;
                    if !self.managed
                        || record.metadata.original_control.era == self.descriptor.control.era
                    {
                        reserved = reserved
                            .checked_sub(flags.iter().filter(|flag| **flag == 0).count() as u64)
                            .ok_or(Fault::Corrupt)?;
                    }
                    tx.put(
                        &record_key(b'e', self.group(), *id),
                        &tombstone(record.metadata.validity.deadline()),
                    )?;
                    for tag in *b"rpq" {
                        tx.delete(&record_key(tag, self.group(), *id))?;
                    }
                    count = count.checked_sub(1).ok_or(Fault::Corrupt)?;
                }
                tx.put(&key(b'n', &self.group().0), &count.to_be_bytes())?;
                self.put_reserved(tx, reserved)?;
                for (op, fact) in plan.operations.iter().zip(&plan.facts) {
                    if tx.read(&key(b'o', &op.0))?.as_deref().map(Vec::as_slice)
                        != Some(encode_fact(fact)?.as_slice())
                    {
                        return Err(Fault::Corrupt.into());
                    }
                    for tag in *b"oaz" {
                        tx.delete(&key(tag, &op.0))?;
                    }
                    if matches!(fact.result, ResultFact::Incoming(_)) {
                        tx.delete(&key(b'd', &fact.fingerprint))?;
                    }
                }
                for op in &plan.bootstrap {
                    tx.put(
                        &key(b'a', &op.0),
                        &plan
                            .floor
                            .0
                            .saturating_add(self.limits.maximum_window.0)
                            .to_be_bytes(),
                    )?;
                }
                let mut positions =
                    self.tx_positions(tx, index::CHANGES, b'c', self.limits.max_changes)?;
                for change in &plan.changes {
                    tx.delete(&feed_key(self.group(), change.cursor))?;
                }
                positions.retain(|position| {
                    !plan
                        .changes
                        .iter()
                        .any(|change| change.cursor.0 == *position)
                });
                tx.put(
                    &key(index::CHANGES, &self.group().0),
                    &index::encode_positions(&positions)?,
                )?;
                // History removal is observable to revision-based projections
                // without retaining a new event forever for every cleanup pass.
                tx.put(
                    &key(b'c', &self.group().0),
                    &plan
                        .change_head
                        .checked_add(1)
                        .ok_or(Fault::Capacity)?
                        .to_be_bytes(),
                )?;
                let mut positions = self.tx_positions(
                    tx,
                    index::ENVELOPES,
                    inventory::HEAD,
                    self.limits.max_envelopes,
                )?;
                let mut bytes = counter(
                    &tx.read(&key(inventory::BYTES, &self.group().0))?
                        .ok_or(Fault::Corrupt)?,
                )?;
                for retained in &plan.envelopes {
                    for tag in [inventory::INFO, inventory::WIRE, b'd'] {
                        tx.delete(&key(tag, &retained.info.id.0))?;
                    }
                    tx.delete(&inventory::sequence_key(
                        self.group(),
                        retained.info.position,
                    ))?;
                    bytes = bytes
                        .checked_sub(retained.info.bytes as u64)
                        .ok_or(Fault::Corrupt)?;
                }
                positions.retain(|position| {
                    !plan
                        .envelopes
                        .iter()
                        .any(|retained| retained.info.position.0 == *position)
                });
                tx.put(
                    &key(index::ENVELOPES, &self.group().0),
                    &index::encode_positions(&positions)?,
                )?;
                tx.put(
                    &key(inventory::BYTES, &self.group().0),
                    &bytes.to_be_bytes(),
                )?;
                for id in &plan.tombstones {
                    if let Some(bytes) = tx.read(&record_key(b'e', self.group(), *id))? {
                        if !valid_tombstone(&bytes) {
                            return Err(Fault::Corrupt.into());
                        }
                        tx.delete(&record_key(b'e', self.group(), *id))?;
                    }
                }
                let mut hash = Sha256::new();
                hash.update(b"prns.groups.inventory.maintenance.v1\0");
                hash.update(fingerprint);
                hash.update(operation.0);
                let generation: [u8; 32] = hash.finalize().into();
                tx.put(&key(inventory::GENERATION, &self.group().0), &generation)?;
                tx.put(&key(FLOOR, &self.group().0), &plan.floor.0.to_be_bytes())?;
                tx.put(&key(LAST, &self.group().0), &marker)?;
                Ok(())
            })
            .map_err(composition)?;
        Ok(Self::maintenance_outcome(commit, plan))
    }

    fn maintenance_outcome(commit: CommitReceipt, plan: &MaintenancePlan) -> MaintenanceOutcome {
        MaintenanceOutcome {
            commit,
            records_removed: plan.records.len(),
            operations_retired: plan.operations.len(),
            envelopes_removed: plan.envelopes.len(),
            changes_removed: plan.changes.len(),
        }
    }
}
