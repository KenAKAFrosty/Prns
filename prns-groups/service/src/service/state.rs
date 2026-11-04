use super::*;

/// Whether this adapter retains a record-service fact for the exact operation.
/// This lookup neither decodes the fact nor changes state. Absence does not
/// establish noncommit or authorize retirement: the caller must still verify
/// the MLS output, its group and any enrollment replay obligations.
pub fn has_operation_fact<B: AtomicBackend + Send>(
    adapter: &Adapter<B>,
    operation: OperationId,
) -> Result<bool, prns_groups_mls::Error<B::Error>> {
    adapter
        .application_read(&key(b'o', &operation.0))
        .map(|fact| fact.is_some())
}

impl<B: AtomicBackend + Send> GroupService<'_, B> {
    pub(super) fn require_active(&self) -> ServiceResult<(), B> {
        if self
            .adapter
            .is_retired(self.group())
            .map_err(Error::Security)?
        {
            return Err(Error::Security(prns_groups_mls::Error::GroupMissing));
        }
        Ok(())
    }
    pub(super) fn fingerprint(&self, domain: &[u8], bytes: &[u8]) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"prns.groups.service.operation.v1\0");
        hash.update(domain);
        hash.update(self.descriptor.group.0);
        hash.update(bytes);
        hash.finalize().into()
    }
    pub(super) fn read(&self, key: &[u8]) -> ServiceResult<Option<Zeroizing<Vec<u8>>>, B> {
        self.adapter.application_read(key).map_err(Error::Security)
    }
    pub(super) fn prior(
        &self,
        operation: OperationId,
        fingerprint: [u8; 32],
    ) -> ServiceResult<Option<Fact>, B> {
        let Some(bytes) = self.read(&key(b'o', &operation.0))? else {
            return Ok(None);
        };
        let fact = decode_fact(&bytes)?;
        if fact.fingerprint != fingerprint {
            return Err(Fault::IdempotencyConflict.into());
        }
        Ok(Some(fact))
    }
    pub(super) fn receipt(&self, operation: OperationId) -> ServiceResult<CommitReceipt, B> {
        match self.coordinator().recover(operation)? {
            RecoveryOutcome::Committed(receipt) => Ok(receipt),
            RecoveryOutcome::Retired { .. } => Err(Fault::EvidencePruned.into()),
            _ => Err(Fault::Corrupt.into()),
        }
    }
    pub(super) fn check_configuration(&self) -> ServiceResult<(), B> {
        match self.read(&key(b'g', &self.descriptor.group.0))? {
            Some(bytes) if bytes.as_slice() == self.configuration => self.check_inventory(true),
            Some(_) => Err(Fault::ConfigurationChanged.into()),
            None => {
                if self.read(&key(b'n', &self.descriptor.group.0))?.is_some()
                    || self.read(&key(b'c', &self.descriptor.group.0))?.is_some()
                {
                    return Err(Fault::Corrupt.into());
                }
                self.check_inventory(false)
            }
        }
    }
    pub(super) fn initialize(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        operation: OperationId,
    ) -> ServiceResult<(), B> {
        let config = key(b'g', &self.descriptor.group.0);
        match tx.read(&config)? {
            Some(bytes) if bytes.as_slice() == self.configuration => {
                self.initialize_indexes(tx)?;
                self.initialize_reservations(tx)
            }
            Some(_) => Err(Fault::ConfigurationChanged.into()),
            None => {
                tx.put(&config, &self.configuration)?;
                tx.put(&key(b'n', &self.descriptor.group.0), &0u64.to_be_bytes())?;
                tx.put(&key(b'c', &self.descriptor.group.0), &0u64.to_be_bytes())?;
                self.initialize_inventory(tx, operation)?;
                self.initialize_indexes(tx)?;
                self.initialize_reservations(tx)?;
                Ok(())
            }
        }
    }
    pub(super) fn put_fact(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        operation: OperationId,
        fact: Fact,
        deadline: Option<Timestamp>,
    ) -> ServiceResult<(), B> {
        tx.put(&key(b'o', &operation.0), &encode_fact(&fact)?)?;
        if let Some(deadline) = deadline {
            tx.put(&key(b'a', &operation.0), &deadline.0.to_be_bytes())?;
        }
        if self
            .adapter_reserved_receipts(tx)?
            .checked_add(2)
            .ok_or(Fault::Capacity)?
            > tx.remaining_output_capacity()? as u64
        {
            return Err(Fault::Capacity.into());
        }
        Ok(())
    }
    pub(super) fn append(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        operation: OperationId,
        kind: ChangeKind,
    ) -> ServiceResult<(), B> {
        let head_key = key(b'c', &self.descriptor.group.0);
        let bytes = tx.read(&head_key)?.ok_or(Fault::Corrupt)?;
        let next = counter(&bytes)?.checked_add(1).ok_or(Fault::Capacity)?;
        let mut positions = self.tx_positions(tx, index::CHANGES, b'c', self.limits.max_changes)?;
        if (positions.len() as u64)
            .checked_add(1)
            .ok_or(Fault::Capacity)?
            .checked_add(self.reserved_receipts(tx)?)
            .ok_or(Fault::Capacity)?
            > self.limits.max_changes as u64
        {
            return Err(Fault::Capacity.into());
        }
        let change = Change {
            cursor: ChangeCursor(next),
            operation,
            kind,
        };
        tx.put(
            &feed_key(self.descriptor.group, change.cursor),
            &encode_change(&change)?,
        )?;
        positions.push(next);
        tx.put(
            &key(index::CHANGES, &self.group().0),
            &index::encode_positions(&positions)?,
        )?;
        tx.put(&head_key, &next.to_be_bytes())?;
        Ok(())
    }
    pub(super) fn insert_record(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        record: &Record,
        bytes: &[u8],
    ) -> ServiceResult<(), B> {
        let count_key = key(b'n', &self.descriptor.group.0);
        let count = counter(&tx.read(&count_key)?.ok_or(Fault::Corrupt)?)?
            .checked_add(1)
            .ok_or(Fault::Capacity)?;
        if count > self.limits.max_records as u64 {
            return Err(Fault::Capacity.into());
        }
        let id = record.metadata.record;
        let mut flags = Zeroizing::new(Vec::new());
        flags
            .try_reserve_exact(record.targets.len())
            .map_err(|_| Fault::Allocation)?;
        flags.resize(record.targets.len(), 0);
        tx.put(&record_key(b'r', self.descriptor.group, id), bytes)?;
        tx.put(&record_key(b'p', self.descriptor.group, id), &flags)?;
        if record.targets.contains(&self.local) {
            tx.put(&record_key(b'q', self.descriptor.group, id), &[0])?;
        }
        self.reserve_receipts(tx, record.targets.len())?;
        tx.put(&count_key, &count.to_be_bytes())?;
        Ok(())
    }
    pub(super) fn encode_record(&self, record: &Record) -> Result<Zeroizing<Vec<u8>>, Fault> {
        codec::encode_record(
            record.metadata.group,
            record.metadata.original_control,
            record.metadata.author,
            &Publication {
                nonce: record.nonce,
                namespace: record.metadata.namespace,
                schema: record.metadata.schema,
                validity: record.metadata.validity,
                targets: &record.targets,
                body: &record.body,
            },
            self.limits,
        )
    }
    pub(super) fn apply_receipt(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        receipt: DurableReceipt,
    ) -> ServiceResult<bool, B> {
        let bytes = tx
            .read(&record_key(b'r', self.descriptor.group, receipt.record))?
            .ok_or(Fault::Corrupt)?;
        let Packet::Record(record) = codec::decode(&bytes, self.limits)? else {
            return Err(Fault::Corrupt.into());
        };
        if record.metadata.group != receipt.group || record.metadata.record != receipt.record {
            return Err(Fault::Corrupt.into());
        }
        let index = record
            .targets
            .iter()
            .position(|target| *target == receipt.target)
            .ok_or(Fault::Corrupt)?;
        let progress_key = record_key(b'p', self.descriptor.group, receipt.record);
        let mut flags = tx.read(&progress_key)?.ok_or(Fault::Corrupt)?;
        validate_flags(&flags, record.targets.len())?;
        if flags[index] == 1 {
            return Ok(false);
        }
        flags[index] = 1;
        tx.put(&progress_key, &flags)?;
        self.release_receipt(tx)?;
        Ok(true)
    }
    pub(super) fn admit(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        sender: &MemberBinding,
        body: &[u8],
        clock: ClockReading,
    ) -> ServiceResult<Decision, B> {
        let packet = match codec::decode(body, self.limits) {
            Ok(packet) => packet,
            Err(Fault::Allocation) => return Err(Fault::Allocation.into()),
            Err(_) => return Ok(Decision::reject(RejectReason::InvalidPacket)),
        };
        let sender = endpoint(sender)?;
        if !self.descriptor.readers.contains(&sender) {
            return Ok(Decision::reject(RejectReason::WrongAuthor));
        }
        let result = match &packet {
            Packet::Record(record) => {
                let m = record.metadata;
                if m.group != self.descriptor.group {
                    return Ok(Decision::reject(RejectReason::WrongGroup));
                }
                if m.author != sender {
                    return Ok(Decision::reject(RejectReason::WrongAuthor));
                }
                if m.original_control != self.descriptor.control {
                    return Ok(Decision::reject(RejectReason::WrongControl));
                }
                if !record.targets.contains(&self.local)
                    || record
                        .targets
                        .iter()
                        .any(|target| !self.descriptor.readers.contains(target))
                {
                    return Ok(Decision::reject(RejectReason::NotTargeted));
                }
                let expired = tx.read(&record_key(b'e', self.descriptor.group, m.record))?;
                if let Some(tombstone) = expired {
                    if !maintenance::valid_tombstone(&tombstone) {
                        return Err(Fault::Corrupt.into());
                    }
                    return Ok(Decision::reject(RejectReason::Expired));
                }
                match m
                    .validity
                    .admission(self.effective_clock(tx, clock)?, self.limits.future_skew)
                {
                    TimeAdmission::Accept => {}
                    TimeAdmission::Expired => {
                        return Ok(Decision {
                            expired_record: Some(m.record),
                            deadline: Some(m.validity.deadline()),
                            ..Decision::reject(RejectReason::Expired)
                        });
                    }
                    TimeAdmission::TimeUncertain => return Err(Fault::TimeUncertain.into()),
                }
                Incoming::Record(m.record)
            }
            Packet::Receipt { receipt, deadline } => {
                if receipt.group != self.descriptor.group {
                    return Ok(Decision::reject(RejectReason::WrongGroup));
                }
                if receipt.target != sender {
                    return Ok(Decision::reject(RejectReason::InvalidReceipt));
                }
                let Some(bytes) =
                    tx.read(&record_key(b'r', self.descriptor.group, receipt.record))?
                else {
                    if let ClockReading::Trusted(interval) = self.effective_clock(tx, clock)? {
                        if interval.lower() >= *deadline {
                            return Ok(Decision {
                                deadline: Some(*deadline),
                                ..Decision::reject(RejectReason::UnknownRecord)
                            });
                        }
                    }
                    if let Some(tombstone) =
                        tx.read(&record_key(b'e', self.descriptor.group, receipt.record))?
                    {
                        if !maintenance::valid_tombstone(&tombstone) {
                            return Err(Fault::Corrupt.into());
                        }
                        return Ok(Decision::reject(RejectReason::UnknownRecord));
                    }
                    return Err(Fault::MissingRecord.into());
                };
                let Packet::Record(record) = codec::decode(&bytes, self.limits)? else {
                    return Err(Fault::Corrupt.into());
                };
                if record.metadata.group != receipt.group
                    || record.metadata.record != receipt.record
                {
                    return Err(Fault::Corrupt.into());
                }
                if *deadline != record.metadata.validity.deadline() {
                    return Ok(Decision::reject(RejectReason::InvalidReceipt));
                }
                if self.managed
                    && record.metadata.original_control.era != self.descriptor.control.era
                {
                    return Ok(Decision::reject(RejectReason::WrongControl));
                }
                if !record.targets.contains(&receipt.target) {
                    return Ok(Decision::reject(RejectReason::InvalidReceipt));
                }
                Incoming::Receipt(*receipt)
            }
        };
        let deadline = match &packet {
            Packet::Record(record) => Some(record.metadata.validity.deadline()),
            Packet::Receipt { deadline, .. } => Some(*deadline),
        };
        Ok(Decision {
            result,
            packet: Some(packet),
            expired_record: None,
            deadline,
        })
    }
}
