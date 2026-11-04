use crate::{
    codec::{self, Packet},
    storage::*,
    *,
};
use alloc::vec::Vec;
use core::cell::RefCell;
use prns_groups_core::{
    Admission as TimeAdmission, DeliveryProgress, ReceiptStatus, RemovalStatus, TargetProgress,
};
use prns_groups_mls::{
    Adapter, Admission, ApplicationTransaction, Command, CompositionError, Event, MemberBinding,
};
use prns_groups_store::{AtomicBackend, CommitReceipt, Coordinator, RecoveryOutcome};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

type ServiceResult<T, B> = Result<T, Error<<B as AtomicBackend>::Error>>;

/// A bounded record service over one endpoint's exclusively owned adapter.
/// The caller authenticates enrollment/control descriptors and serializes use.
/// Application events and wire output must only be published after success.
pub struct GroupService<'a, B> {
    adapter: Adapter<B>,
    descriptor: Descriptor<'a>,
    limits: Limits,
    local: Endpoint,
    configuration: [u8; 32],
    managed: bool,
}

impl<'a, B: AtomicBackend + Send> GroupService<'a, B> {
    pub fn new(
        adapter: Adapter<B>,
        descriptor: Descriptor<'a>,
        limits: Limits,
    ) -> ServiceResult<Self, B> {
        Self::configured(adapter, descriptor, limits, false)
    }

    /// The caller supplies a descriptor authenticated and committed with the
    /// MLS transition. Membership may advance; group, local identity and limits
    /// remain immutable. Never construct this from unverified network metadata.
    /// After local removal or closure, the last active descriptor permits
    /// retained history queries; the durable MLS terminal fence blocks traffic.
    pub fn new_managed(
        adapter: Adapter<B>,
        descriptor: Descriptor<'a>,
        limits: Limits,
    ) -> ServiceResult<Self, B> {
        Self::configured(adapter, descriptor, limits, true)
    }

    fn configured(
        adapter: Adapter<B>,
        descriptor: Descriptor<'a>,
        limits: Limits,
        managed: bool,
    ) -> ServiceResult<Self, B> {
        let local = endpoint(&adapter.local_binding())?;
        if descriptor.readers.is_empty()
            || descriptor.readers.len() > limits.max_targets
            || descriptor.readers.windows(2).any(|p| p[0] >= p[1])
            || !descriptor.readers.contains(&local)
            || limits.max_records == 0
            || limits.max_envelopes == 0
            || limits.max_retained_wire_bytes == 0
            || limits.max_changes == 0
            || limits.max_body_bytes == 0
            || limits.maximum_window.0 == 0
        {
            return Err(Fault::Configuration.into());
        }
        let mut hash = Sha256::new();
        if managed {
            hash.update(b"prns.groups.service.managed.configuration.v1\0");
        } else {
            hash.update(b"prns.groups.service.configuration.v1\0");
        }
        hash.update(descriptor.group.0);
        if !managed {
            hash.update(descriptor.control.head.0);
            hash.update(descriptor.control.era.0 .0);
        }
        hash.update(local.id.0);
        hash.update(local.incarnation.get().to_be_bytes());
        if !managed {
            for reader in descriptor.readers {
                hash.update(reader.id.0);
                hash.update(reader.incarnation.get().to_be_bytes());
            }
        }
        for limit in [
            limits.max_records,
            limits.max_targets,
            limits.max_body_bytes,
            limits.max_changes,
            limits.max_envelopes,
            limits.max_retained_wire_bytes,
        ] {
            hash.update(
                u64::try_from(limit)
                    .map_err(|_| Fault::Configuration)?
                    .to_be_bytes(),
            );
        }
        hash.update(limits.maximum_window.0.to_be_bytes());
        hash.update(limits.future_skew.0.to_be_bytes());
        let service = Self {
            adapter,
            descriptor,
            limits,
            local,
            configuration: hash.finalize().into(),
            managed,
        };
        service.check_configuration()?;
        Ok(service)
    }

    pub const fn group(&self) -> GroupId {
        self.descriptor.group
    }

    pub const fn local_endpoint(&self) -> Endpoint {
        self.local
    }

    pub const fn readers(&self) -> &[Endpoint] {
        self.descriptor.readers
    }

    pub fn coordinator(&self) -> &Coordinator<B> {
        self.adapter.coordinator()
    }

    /// A repeat operation must have the same immutable publication, including
    /// nonce, targets and deadline. Retrying never encrypts again.
    pub fn publish(
        &mut self,
        operation: OperationId,
        request: &Publication<'_>,
        clock: ClockReading,
    ) -> ServiceResult<Published, B> {
        self.require_active()?;
        self.check_configuration()?;
        if request
            .targets
            .iter()
            .any(|target| !self.descriptor.readers.contains(target))
        {
            return Err(Fault::InvalidTargets.into());
        }
        let bytes = codec::encode_record(
            self.descriptor.group,
            self.descriptor.control,
            self.local,
            request,
            self.limits,
        )?;
        let fingerprint = self.fingerprint(b"publish", &bytes);
        if let Some(fact) = self.prior(operation, fingerprint)? {
            return match fact.result {
                ResultFact::Published(record) => Ok(Published {
                    record,
                    commit: self.receipt(operation)?,
                }),
                _ => Err(Fault::Corrupt.into()),
            };
        }
        match request
            .validity
            .admission(self.current_clock(clock)?, self.limits.future_skew)
        {
            TimeAdmission::Accept => {}
            TimeAdmission::Expired => return Err(Fault::Expired.into()),
            TimeAdmission::TimeUncertain => return Err(Fault::TimeUncertain.into()),
        }
        let record = match codec::decode(&bytes, self.limits)? {
            Packet::Record(record) => record,
            _ => return Err(Fault::Corrupt.into()),
        };
        let id = record.metadata.record;
        // A new operation cannot reset evidence for an existing immutable record.
        if self.record(id)?.is_some() {
            return Err(Fault::IdempotencyConflict.into());
        }
        let committed = self
            .adapter
            .compose(
                operation,
                Command::Send {
                    group: self.descriptor.group,
                    body: &bytes,
                },
                |_, _, _| Ok(Admission::Accept),
                |tx, output| {
                    self.initialize(tx, operation)?;
                    self.insert_record(tx, &record, &bytes)?;
                    self.retain_output(tx, operation, output, Incoming::Record(id))?;
                    self.append(tx, operation, ChangeKind::Published(id))?;
                    self.put_fact(
                        tx,
                        operation,
                        Fact {
                            fingerprint,
                            result: ResultFact::Published(id),
                        },
                        Some(record.metadata.validity.deadline()),
                    )
                },
            )
            .map_err(composition)?;
        Ok(Published {
            record: id,
            commit: committed.receipt,
        })
    }

    /// Recover the exact committed MLS ciphertext. It remains subject to the
    /// adapter's authorization-era fence and receipt-retirement policy.
    pub fn outgoing(&self, operation: OperationId) -> ServiceResult<Option<Zeroizing<Vec<u8>>>, B> {
        self.require_active()?;
        self.check_configuration()?;
        let Some(bytes) = self.read(&key(b'o', &operation.0))? else {
            return Ok(None);
        };
        let fact = decode_fact(&bytes)?;
        if matches!(fact.result, ResultFact::Incoming(_)) {
            return Ok(None);
        }
        let record = match fact.result {
            ResultFact::Published(id) | ResultFact::ReceiptQueued(id) => id,
            ResultFact::Incoming(_) => return Ok(None),
        };
        if self.record(record)?.is_none() {
            return Err(Fault::EvidencePruned.into());
        }
        let Some(mut output) = self.adapter.retry(operation).map_err(Error::Security)? else {
            return Err(Fault::EvidencePruned.into());
        };
        if output.context.map(|context| context.group) != Some(self.descriptor.group) {
            return Err(Fault::Configuration.into());
        }
        if output.messages.len() != 1 || output.event != Event::Sent {
            return Err(Fault::Corrupt.into());
        }
        Ok(output.messages.pop().map(Zeroizing::new))
    }

    /// Read the exact locally committed result of these ciphertext bytes,
    /// including authenticated rejection. This performs no decryption, receipt
    /// repair, maintenance or new admission. Absence is not noncommit evidence
    /// for an uncertain operation; recover its exact OperationTicket instead.
    pub fn received_result(&self, wire: &[u8]) -> ServiceResult<Option<Incoming>, B> {
        if wire.len() > self.adapter.max_wire_bytes() {
            return Err(Fault::Capacity.into());
        }
        self.check_configuration()?;
        self.received_result_for(self.envelope_id(wire))
    }
    fn received_result_for(&self, envelope: EnvelopeId) -> ServiceResult<Option<Incoming>, B> {
        self.read(&key(b'd', &envelope.0))?
            .map(|bytes| decode_incoming(&bytes).map_err(Error::from))
            .transpose()
    }

    /// Authenticated rejection consumes the receive key and records its reason
    /// atomically. Local capacity/storage failures abort so exact input can retry.
    pub fn receive(
        &mut self,
        operation: OperationId,
        wire: &[u8],
        clock: ClockReading,
    ) -> ServiceResult<Incoming, B> {
        self.require_active()?;
        if wire.len() > self.adapter.max_wire_bytes() {
            return Err(Fault::Capacity.into());
        }
        self.check_configuration()?;
        let fingerprint = self.fingerprint(b"receive", wire);
        if let Some(fact) = self.prior(operation, fingerprint)? {
            return match fact.result {
                ResultFact::Incoming(result) => Ok(result),
                _ => Err(Fault::Corrupt.into()),
            };
        }
        let envelope = EnvelopeId(fingerprint);
        if let Some(result) = self.received_result_for(envelope)? {
            // No new operation is committed or ticket consumed for exact repeats.
            return Ok(result);
        }
        let decision = RefCell::new(None);
        let committed = self
            .adapter
            .compose(
                operation,
                Command::Receive {
                    group: self.descriptor.group,
                    bytes: wire,
                },
                |tx, sender, body| {
                    let mut decided = self.admit(tx, sender, body, clock)?;
                    if matches!(decided.packet, Some(Packet::Receipt { .. }))
                        && wire.len() > MAX_RECEIPT_WIRE_BYTES
                    {
                        decided = Decision::reject(RejectReason::InvalidReceipt);
                    }
                    let admission = match decided.result {
                        Incoming::Rejected(reason) => Admission::Reject(reason as u16),
                        _ => Admission::Accept,
                    };
                    *decision.borrow_mut() = Some(decided);
                    Ok(admission)
                },
                |tx, output| {
                    if !matches!(output.event, Event::Application(_) | Event::Rejected(_)) {
                        return Err(Fault::UnsupportedControl.into());
                    }
                    // Oversized authenticated plaintext is rejected by the
                    // adapter before it invokes application admission.
                    if output.event == Event::Rejected(u16::MAX) && decision.borrow().is_none() {
                        *decision.borrow_mut() =
                            Some(Decision::reject(RejectReason::InvalidPacket));
                    }
                    let borrowed = decision.borrow();
                    let decided = borrowed.as_ref().ok_or(Fault::Corrupt)?;
                    self.initialize(tx, operation)?;
                    if let Some(record) = decided.expired_record {
                        let deadline = decided.deadline.ok_or(Fault::Corrupt)?;
                        tx.put(
                            &record_key(b'e', self.descriptor.group, record),
                            &maintenance::tombstone(deadline),
                        )?;
                        tx.put(&key(b'z', &operation.0), &record.0)?;
                    }
                    match &decided.packet {
                        Some(Packet::Record(record)) => {
                            let stored = tx.read(&record_key(
                                b'r',
                                self.descriptor.group,
                                record.metadata.record,
                            ))?;
                            if stored.is_none() {
                                let bytes = self.encode_record(record)?;
                                self.insert_record(tx, record, &bytes)?;
                                self.append(
                                    tx,
                                    operation,
                                    ChangeKind::Received(record.metadata.record),
                                )?;
                            }
                        }
                        Some(Packet::Receipt { receipt, .. }) => {
                            if self.apply_receipt(tx, *receipt)? {
                                self.append(tx, operation, ChangeKind::Receipt(*receipt))?;
                            }
                        }
                        None => {
                            if let Incoming::Rejected(reason) = decided.result {
                                self.append(
                                    tx,
                                    operation,
                                    ChangeKind::Rejected { envelope, reason },
                                )?;
                            }
                        }
                    }
                    if !matches!(decided.result, Incoming::Rejected(_)) {
                        self.retain(tx, wire, decided.result, None)?;
                    }
                    tx.put(&key(b'd', &envelope.0), &encode_incoming(decided.result)?)?;
                    self.put_fact(
                        tx,
                        operation,
                        Fact {
                            fingerprint,
                            result: ResultFact::Incoming(decided.result),
                        },
                        decided.deadline.or(match clock {
                            ClockReading::Trusted(interval) => Some(Timestamp(
                                interval
                                    .lower()
                                    .0
                                    .saturating_add(self.limits.maximum_window.0),
                            )),
                            ClockReading::Unknown => None,
                        }),
                    )
                },
            )
            .map_err(composition)?;
        drop(committed);
        decision
            .into_inner()
            .map(|decision| decision.result)
            .ok_or(Fault::Corrupt.into())
    }

    /// Queue a receipt only for a locally durable record. Receipt intent survives
    /// restart between acceptance and this separate encryption transaction.
    pub fn emit_receipt(
        &mut self,
        operation: OperationId,
        id: RecordId,
    ) -> ServiceResult<ReceiptOutput, B> {
        self.check_configuration()?;
        if !self.in_current_era(id)? {
            return Err(Fault::ConfigurationChanged.into());
        }
        let receipt = DurableReceipt {
            group: self.descriptor.group,
            record: id,
            target: self.local,
        };
        let record = self.record(id)?.ok_or(Fault::MissingRecord)?;
        let bytes = codec::encode_receipt(&receipt, record.metadata.validity.deadline())?;
        let fingerprint = self.fingerprint(b"receipt", &bytes);
        if let Some(fact) = self.prior(operation, fingerprint)? {
            return match fact.result {
                ResultFact::ReceiptQueued(record) if record == id => Ok(ReceiptOutput {
                    record,
                    commit: self.receipt(operation)?,
                }),
                _ => Err(Fault::Corrupt.into()),
            };
        }
        if !record.targets.contains(&self.local) {
            return Err(Fault::InvalidTargets.into());
        }
        let intent_key = record_key(b'q', self.descriptor.group, id);
        let intent = self.read(&intent_key)?.ok_or(Fault::Corrupt)?;
        if intent.as_slice() != [0] {
            return Err(Fault::IdempotencyConflict.into());
        }
        let committed = self
            .adapter
            .compose(
                operation,
                Command::Send {
                    group: self.descriptor.group,
                    body: &bytes,
                },
                |_, _, _| Ok(Admission::Accept),
                |tx, output| {
                    self.initialize(tx, operation)?;
                    self.apply_receipt(tx, receipt)?;
                    self.retain_output(tx, operation, output, Incoming::Receipt(receipt))?;
                    let mut intent = [0u8; 33];
                    intent[0] = 1;
                    intent[1..].copy_from_slice(&operation.0);
                    tx.put(&intent_key, &intent)?;
                    self.append(tx, operation, ChangeKind::ReceiptQueued(receipt))?;
                    self.put_fact(
                        tx,
                        operation,
                        Fact {
                            fingerprint,
                            result: ResultFact::ReceiptQueued(id),
                        },
                        Some(record.metadata.validity.deadline()),
                    )
                },
            )
            .map_err(composition)?;
        Ok(ReceiptOutput {
            record: id,
            commit: committed.receipt,
        })
    }

    pub fn record(&self, id: RecordId) -> ServiceResult<Option<Record>, B> {
        let Some(bytes) = self.read(&record_key(b'r', self.descriptor.group, id))? else {
            return Ok(None);
        };
        match codec::decode(&bytes, self.limits)? {
            Packet::Record(record)
                if record.metadata.record == id
                    && record.metadata.group == self.descriptor.group =>
            {
                Ok(Some(record))
            }
            _ => Err(Fault::Corrupt.into()),
        }
    }

    pub fn has_pending_receipt(&self, id: RecordId) -> ServiceResult<bool, B> {
        if !self.in_current_era(id)? {
            return Ok(false);
        }
        let Some(bytes) = self.read(&record_key(b'q', self.descriptor.group, id))? else {
            return Ok(false);
        };
        match bytes.as_slice() {
            [0] => Ok(true),
            [1, ..] if bytes.len() == 33 => Ok(false),
            _ => Err(Fault::Corrupt.into()),
        }
    }

    /// An authorization change ends forwarding and receipt emission for old
    /// records. Existing plaintext and authentic delivery evidence remain local.
    pub fn in_current_era(&self, id: RecordId) -> ServiceResult<bool, B> {
        if self
            .adapter
            .is_retired(self.group())
            .map_err(Error::Security)?
        {
            return Ok(false);
        }
        if !self.managed {
            return Ok(true);
        }
        Ok(self.record(id)?.is_some_and(|record| {
            record.metadata.original_control.era == self.descriptor.control.era
        }))
    }

    /// Whether a membership change would interrupt this endpoint's retained,
    /// authored deliveries in the current authorization era. Only authenticated
    /// receipts or definite expiry clear an obligation; an uncertain clock and
    /// transport acknowledgements do not. This is local evidence, not a group
    /// drain protocol. Serialize the check with the subsequent membership change.
    pub fn has_pending_authored_deliveries(&self, clock: ClockReading) -> ServiceResult<bool, B> {
        if self
            .adapter
            .is_retired(self.group())
            .map_err(Error::Security)?
        {
            return Ok(false);
        }
        self.check_configuration()?;
        let clock = self.current_clock(clock)?;
        for position in self.positions(index::CHANGES, b'c', self.limits.max_changes)? {
            let bytes = self
                .read(&feed_key(self.group(), ChangeCursor(position)))?
                .ok_or(Fault::Corrupt)?;
            let change = decode_change(&bytes)?;
            if change.cursor.0 != position {
                return Err(Fault::Corrupt.into());
            }
            let ChangeKind::Published(id) = change.kind else {
                continue;
            };
            let record = self.record(id)?.ok_or(Fault::Corrupt)?;
            if record.metadata.author != self.local {
                return Err(Fault::Corrupt.into());
            }
            if (self.managed && record.metadata.original_control.era != self.descriptor.control.era)
                || record
                    .metadata
                    .validity
                    .admission(clock, self.limits.future_skew)
                    == TimeAdmission::Expired
            {
                continue;
            }
            let flags = self
                .read(&record_key(b'p', self.group(), id))?
                .ok_or(Fault::Corrupt)?;
            validate_flags(&flags, record.targets.len())?;
            if flags.contains(&0) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Durable observations for this record's original, canonically ordered
    /// targets. Scratch allocation is bounded by the selected record limits.
    pub fn delivery_targets(&self, id: RecordId) -> ServiceResult<Option<Vec<TargetProgress>>, B> {
        self.record(id)?
            .map(|record| self.record_targets(&record))
            .transpose()
    }

    fn record_targets(&self, record: &Record) -> ServiceResult<Vec<TargetProgress>, B> {
        let flags = self
            .read(&record_key(
                b'p',
                self.descriptor.group,
                record.metadata.record,
            ))?
            .ok_or(Fault::Corrupt)?;
        validate_flags(&flags, record.targets.len())?;
        let mut targets = Vec::new();
        targets
            .try_reserve_exact(record.targets.len())
            .map_err(|_| Fault::Allocation)?;
        for (target, received) in record.targets.iter().zip(flags.iter()) {
            targets.push(TargetProgress::restore(
                *target,
                if *received == 1 {
                    ReceiptStatus::Received
                } else {
                    ReceiptStatus::Awaiting
                },
                if self.managed && !self.readers().contains(target) {
                    RemovalStatus::Removed
                } else {
                    RemovalStatus::NotKnownRemoved
                },
            ));
        }
        Ok(targets)
    }

    pub fn progress(&self, id: RecordId, clock: ClockReading) -> ServiceResult<DeliverySummary, B> {
        let record = self.record(id)?.ok_or(Fault::MissingRecord)?;
        let mut targets = self.record_targets(&record)?;
        let progress =
            DeliveryProgress::new(&record.metadata, &mut targets).map_err(|_| Fault::Corrupt)?;
        Ok(progress.summary(clock, self.limits.future_skew))
    }

    /// Notifications are only hints. Persist this cursor in the application and
    /// query after resuming to recover all committed changes and outbox jobs.
    pub fn changes(&self, after: ChangeCursor, page_size: usize) -> ServiceResult<ChangePage, B> {
        if page_size == 0 || page_size > self.limits.max_changes {
            return Err(Fault::Capacity.into());
        }
        let head = self
            .read(&key(b'c', &self.descriptor.group.0))?
            .map(|bytes| counter(&bytes))
            .transpose()?
            .unwrap_or(0);
        if after.0 > head {
            return Err(Fault::Configuration.into());
        }
        let positions = self.positions(index::CHANGES, b'c', self.limits.max_changes)?;
        let count = positions
            .iter()
            .filter(|position| **position > after.0)
            .count()
            .min(page_size);
        let mut changes = Vec::new();
        changes
            .try_reserve_exact(count)
            .map_err(|_| Fault::Allocation)?;
        let mut next = after;
        for position in positions
            .into_iter()
            .filter(|position| *position > after.0)
            .take(count)
        {
            next = ChangeCursor(position);
            let bytes = self
                .read(&feed_key(self.descriptor.group, next))?
                .ok_or(Fault::Corrupt)?;
            let change = decode_change(&bytes)?;
            if change.cursor != next {
                return Err(Fault::Corrupt.into());
            }
            changes.push(change);
        }
        if count < page_size {
            next = ChangeCursor(head);
        }
        Ok(ChangePage {
            changes,
            next,
            head: ChangeCursor(head),
        })
    }
}

struct Decision {
    result: Incoming,
    packet: Option<Packet>,
    expired_record: Option<RecordId>,
    deadline: Option<Timestamp>,
}
impl Decision {
    fn reject(reason: RejectReason) -> Self {
        Self {
            result: Incoming::Rejected(reason),
            packet: None,
            expired_record: None,
            deadline: None,
        }
    }
}
fn endpoint<E>(binding: &MemberBinding) -> Result<Endpoint, Error<E>> {
    Ok(Endpoint {
        id: EndpointId(binding.endpoint),
        incarnation: EndpointIncarnation::new(binding.incarnation)
            .map_err(|_| Fault::Configuration)?,
    })
}
fn counter(bytes: &[u8]) -> Result<u64, Fault> {
    Ok(u64::from_be_bytes(
        bytes.try_into().map_err(|_| Fault::Corrupt)?,
    ))
}
fn validate_flags(bytes: &[u8], count: usize) -> Result<(), Fault> {
    if bytes.len() != count || bytes.iter().any(|flag| *flag > 1) {
        Err(Fault::Corrupt)
    } else {
        Ok(())
    }
}
fn composition<E>(error: CompositionError<E, Error<E>>) -> Error<E> {
    match error {
        CompositionError::Mls(error) => Error::Security(error),
        CompositionError::Application(error) => error,
        CompositionError::ApplicationStorageFault => Error::Fault(Fault::ApplicationStorageFault),
    }
}

mod state;
pub use state::has_operation_fact;

mod dependencies;
pub use dependencies::DurableDependencies;
mod capacity;
mod index;
mod maintenance;
pub use maintenance::{MaintenanceOutcome, MaintenancePlan};
mod inventory;
