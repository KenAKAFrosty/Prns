//! Exact ciphertext custody for the fixed-membership slice. An inventory
//! advertises available bytes; it is never evidence of endpoint receipt.

use super::*;
use prns_groups_mls::Output;

pub(super) const GENERATION: u8 = b'G';
pub(super) const HEAD: u8 = b'H';
pub(super) const BYTES: u8 = b'B';
pub(super) const INFO: u8 = b'J';
pub(super) const WIRE: u8 = b'W';
pub(super) const SEQUENCE: u8 = b'I';
const INFO_BYTES: usize = 118;
const INFO_MAGIC: &[u8; 4] = b"PGE\x01";

#[derive(Clone, Copy)]
pub(super) struct Retained {
    pub(super) info: EnvelopeInfo,
    pub(super) output: Option<OperationId>,
}

impl<B: AtomicBackend + Send> GroupService<'_, B> {
    /// Stable across the group: hashes the exact MLS ciphertext and group scope.
    /// Network inventory hints must be checked against this ID before receiving.
    pub fn envelope_id(&self, wire: &[u8]) -> EnvelopeId {
        crate::envelope_id(self.descriptor.group, wire)
    }

    /// An unopened application ledger has no generation yet. Its first durable
    /// operation, including authenticated rejection, creates the generation.
    pub fn snapshot(&self) -> ServiceResult<Option<InventorySnapshot>, B> {
        self.check_configuration()?;
        let Some(generation) = self.read(&key(GENERATION, &self.descriptor.group.0))? else {
            return Ok(None);
        };
        let generation = generation
            .as_slice()
            .try_into()
            .map_err(|_| Fault::Corrupt)?;
        let head = counter(
            &self
                .read(&key(HEAD, &self.descriptor.group.0))?
                .ok_or(Fault::Corrupt)?,
        )?;
        Ok(Some(InventorySnapshot {
            generation,
            head: InventoryCursor(head),
        }))
    }

    /// Scan at most `scan_limit` entries from a frozen append-only inventory.
    /// Pages can be empty while `next < snapshot.head` because of filtering.
    /// Callers authenticate the transport peer before using its bound endpoint.
    pub fn inventory(
        &self,
        snapshot: InventorySnapshot,
        after: InventoryCursor,
        kind: InventoryKind,
        recipient: Endpoint,
        scan_limit: usize,
    ) -> ServiceResult<InventoryPage, B> {
        self.require_active()?;
        if scan_limit == 0 || scan_limit > self.limits.max_envelopes {
            return Err(Fault::Capacity.into());
        }
        if !self.descriptor.readers.contains(&recipient) {
            return Err(Fault::InvalidTargets.into());
        }
        let current = self.snapshot()?.ok_or(Fault::RestartInventory)?;
        if after > snapshot.head {
            return Err(Fault::InvalidEncoding.into());
        }
        if snapshot.generation != current.generation || snapshot.head > current.head {
            // A partial scan may outlive a replaced or restored remote ledger.
            // Its frozen prefix no longer exists; tell the client to discover
            // again instead of indefinitely retrying this stale continuation.
            return Err(Fault::RestartInventory.into());
        }
        let positions = self.positions(index::ENVELOPES, HEAD, self.limits.max_envelopes)?;
        let count = positions
            .iter()
            .filter(|p| **p > after.0 && **p <= snapshot.head.0)
            .count()
            .min(scan_limit);
        let mut items = Vec::new();
        items
            .try_reserve_exact(count)
            .map_err(|_| Fault::Allocation)?;
        let mut next = after;
        for position in positions
            .into_iter()
            .filter(|p| *p > after.0 && *p <= snapshot.head.0)
            .take(count)
        {
            next = InventoryCursor(position);
            let bytes = self
                .read(&sequence_key(self.descriptor.group, next))?
                .ok_or(Fault::Corrupt)?;
            let id = EnvelopeId(bytes.as_slice().try_into().map_err(|_| Fault::Corrupt)?);
            let retained = self.retained(id)?.ok_or(Fault::Corrupt)?;
            let info = retained.info;
            if info.position != next {
                return Err(Fault::Corrupt.into());
            }
            if info.kind != kind {
                continue;
            }
            let Some(record) = self.record(info.record)? else {
                // Expiry retirement fences forwarding before later bounded
                // batches finish removing every associated envelope.
                continue;
            };
            if self.managed && record.metadata.original_control.era != self.descriptor.control.era {
                continue;
            }
            if record.metadata.author == recipient || record.targets.contains(&recipient) {
                items.push(info);
            }
        }
        if count < scan_limit {
            next = snapshot.head;
        }
        Ok(InventoryPage {
            items,
            next,
            snapshot,
        })
    }

    /// Retrieve an exact retained envelope. The transport must apply the same
    /// authenticated fixed-reader membership check before releasing bytes.
    /// Delivery target filters are scheduling policy, not confidentiality bounds.
    /// Locally generated output still passes MLS authorization-era fences.
    pub fn envelope(&self, id: EnvelopeId) -> ServiceResult<Option<Zeroizing<Vec<u8>>>, B> {
        self.check_configuration()?;
        let Some(retained) = self.retained(id)? else {
            return Ok(None);
        };
        if self.record(retained.info.record)?.is_none()
            || !self.in_current_era(retained.info.record)?
        {
            return Ok(None);
        }
        let bytes = self.read(&key(WIRE, &id.0))?.ok_or(Fault::Corrupt)?;
        if bytes.len() != retained.info.bytes || self.envelope_id(&bytes) != id {
            return Err(Fault::Corrupt.into());
        }
        if let Some(operation) = retained.output {
            let output = self.outgoing(operation)?.ok_or(Fault::EvidencePruned)?;
            if output.as_slice() != bytes.as_slice() {
                return Err(Fault::Corrupt.into());
            }
        }
        Ok(Some(bytes))
    }

    /// Authoritative local metadata for an accepted or generated envelope.
    /// A remote inventory's kind, record and size are untrusted scheduling hints;
    /// compare them with this fact before using the already-known fast path.
    pub fn envelope_info(&self, id: EnvelopeId) -> ServiceResult<Option<EnvelopeInfo>, B> {
        self.check_configuration()?;
        Ok(self.retained(id)?.map(|retained| retained.info))
    }

    /// Whether this ledger has already accepted or generated an exact envelope.
    /// This says nothing about another endpoint's durable acceptance.
    pub fn has_envelope(&self, id: EnvelopeId) -> ServiceResult<bool, B> {
        self.check_configuration()?;
        Ok(self.retained(id)?.is_some())
    }

    /// Recover the exact committed local device receipt, without encrypting a
    /// replacement. A pending intent has no releasable receipt yet. Normal
    /// output-retirement and authorization-era fences still apply.
    pub fn local_receipt(&self, record: RecordId) -> ServiceResult<Option<Zeroizing<Vec<u8>>>, B> {
        self.check_configuration()?;
        if self.record(record)?.is_none() || !self.in_current_era(record)? {
            return Ok(None);
        }
        let Some(intent) = self.read(&record_key(b'q', self.descriptor.group, record))? else {
            return Ok(None);
        };
        match intent.as_slice() {
            [0] => Ok(None),
            [1, ..] if intent.len() == 33 => {
                let operation = OperationId(intent[1..].try_into().map_err(|_| Fault::Corrupt)?);
                let wire = self.outgoing(operation)?;
                if let Some(wire) = &wire {
                    let info = self
                        .envelope_info(self.envelope_id(wire))?
                        .ok_or(Fault::Corrupt)?;
                    if info.kind != InventoryKind::Receipt || info.record != record {
                        return Err(Fault::Corrupt.into());
                    }
                }
                Ok(wire)
            }
            _ => Err(Fault::Corrupt.into()),
        }
    }

    /// Select one durable authored record still awaiting this target's receipt.
    /// The disposable cursor rotates the bounded retained inventory and wraps
    /// once, so one unavailable record cannot permanently hide newer work.
    /// Reopening with cursor zero reconstructs work from committed facts; a
    /// transport acknowledgement never removes an obligation.
    pub fn pending_delivery(
        &self,
        recipient: Endpoint,
        after: InventoryCursor,
        clock: ClockReading,
    ) -> ServiceResult<Option<EnvelopeInfo>, B> {
        if self
            .adapter
            .is_retired(self.group())
            .map_err(Error::Security)?
        {
            return Ok(None);
        }
        self.check_configuration()?;
        if recipient == self.local || !self.descriptor.readers.contains(&recipient) {
            return Err(Fault::InvalidTargets.into());
        }
        let positions = self.positions(index::ENVELOPES, HEAD, self.limits.max_envelopes)?;
        for position in positions
            .iter()
            .copied()
            .filter(|position| *position > after.0)
            .chain(
                positions
                    .iter()
                    .copied()
                    .filter(|position| *position <= after.0),
            )
        {
            let bytes = self
                .read(&sequence_key(
                    self.descriptor.group,
                    InventoryCursor(position),
                ))?
                .ok_or(Fault::Corrupt)?;
            let id = EnvelopeId(bytes.as_slice().try_into().map_err(|_| Fault::Corrupt)?);
            let info = self.retained(id)?.ok_or(Fault::Corrupt)?.info;
            if info.position.0 != position {
                return Err(Fault::Corrupt.into());
            }
            if info.kind != InventoryKind::Record {
                continue;
            }
            let Some(record) = self.record(info.record)? else {
                continue;
            };
            if record.metadata.author != self.local
                || (self.managed
                    && record.metadata.original_control.era != self.descriptor.control.era)
                || record
                    .metadata
                    .validity
                    .admission(clock, self.limits.future_skew)
                    != TimeAdmission::Accept
            {
                continue;
            }
            let Some(target) = record
                .targets
                .iter()
                .position(|target| *target == recipient)
            else {
                continue;
            };
            let flags = self
                .read(&record_key(b'p', self.descriptor.group, info.record))?
                .ok_or(Fault::Corrupt)?;
            validate_flags(&flags, record.targets.len())?;
            if flags[target] == 0 {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }

    pub(super) fn retained(&self, id: EnvelopeId) -> ServiceResult<Option<Retained>, B> {
        let Some(bytes) = self.read(&key(INFO, &id.0))? else {
            return Ok(None);
        };
        let retained = decode_retained(&bytes)?;
        if retained.info.id != id
            || retained.info.position.0 == 0
            || retained.info.bytes == 0
            || retained.info.bytes > self.adapter.max_wire_bytes()
        {
            return Err(Fault::Corrupt.into());
        }
        Ok(Some(retained))
    }

    pub(super) fn initialize_inventory(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        operation: OperationId,
    ) -> ServiceResult<(), B> {
        let mut hash = Sha256::new();
        hash.update(b"prns.groups.service.inventory.generation.v1\0");
        hash.update(self.configuration);
        hash.update(operation.0);
        let generation: [u8; 32] = hash.finalize().into();
        tx.put(&key(GENERATION, &self.descriptor.group.0), &generation)?;
        tx.put(&key(HEAD, &self.descriptor.group.0), &0u64.to_be_bytes())?;
        tx.put(&key(BYTES, &self.descriptor.group.0), &0u64.to_be_bytes())?;
        Ok(())
    }

    pub(super) fn check_inventory(&self, initialized: bool) -> ServiceResult<(), B> {
        let generation = self.read(&key(GENERATION, &self.descriptor.group.0))?;
        let head = self.read(&key(HEAD, &self.descriptor.group.0))?;
        let bytes = self.read(&key(BYTES, &self.descriptor.group.0))?;
        match (initialized, generation, head, bytes) {
            (false, None, None, None) => Ok(()),
            (true, Some(generation), Some(head), Some(bytes)) if generation.len() == 32 => {
                let head = counter(&head)?;
                let bytes = counter(&bytes)?;
                let count = self
                    .positions(index::ENVELOPES, HEAD, self.limits.max_envelopes)?
                    .len() as u64;
                if bytes > self.limits.max_retained_wire_bytes as u64
                    || (count == 0) != (bytes == 0)
                    || bytes < count
                    || head < count
                {
                    return Err(Fault::Corrupt.into());
                }
                Ok(())
            }
            _ => Err(Fault::Corrupt.into()),
        }
    }

    pub(super) fn retain_output(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        operation: OperationId,
        output: &Output,
        result: Incoming,
    ) -> ServiceResult<(), B> {
        if output.context.map(|context| context.group) != Some(self.descriptor.group)
            || output.event != Event::Sent
            || output.messages.len() != 1
        {
            return Err(Fault::Corrupt.into());
        }
        self.retain(tx, &output.messages[0], result, Some(operation))
    }

    pub(super) fn retain(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        wire: &[u8],
        result: Incoming,
        output: Option<OperationId>,
    ) -> ServiceResult<(), B> {
        if wire.is_empty() || wire.len() > self.adapter.max_wire_bytes() {
            return Err(Fault::Capacity.into());
        }
        let (kind, record) = match result {
            Incoming::Record(record) => (InventoryKind::Record, record),
            Incoming::Receipt(receipt) => (InventoryKind::Receipt, receipt.record),
            Incoming::Rejected(_) => return Err(Fault::Corrupt.into()),
        };
        if kind == InventoryKind::Receipt && wire.len() > MAX_RECEIPT_WIRE_BYTES {
            return Err(Fault::Capacity.into());
        }
        let id = self.envelope_id(wire);
        if tx.read(&key(INFO, &id.0))?.is_some() {
            // Every accepted/generated envelope is deduplicated before a new MLS
            // operation. Finding it again inside a transaction is inconsistent.
            return Err(Fault::Corrupt.into());
        }
        let head_key = key(HEAD, &self.descriptor.group.0);
        let bytes_key = key(BYTES, &self.descriptor.group.0);
        let next = counter(&tx.read(&head_key)?.ok_or(Fault::Corrupt)?)?
            .checked_add(1)
            .ok_or(Fault::Capacity)?;
        let total = counter(&tx.read(&bytes_key)?.ok_or(Fault::Corrupt)?)?
            .checked_add(u64::try_from(wire.len()).map_err(|_| Fault::Capacity)?)
            .ok_or(Fault::Capacity)?;
        let reserved = self.reserved_receipts(tx)?;
        let reserved_bytes = reserved
            .checked_mul(MAX_RECEIPT_WIRE_BYTES as u64)
            .ok_or(Fault::Capacity)?;
        let mut positions =
            self.tx_positions(tx, index::ENVELOPES, HEAD, self.limits.max_envelopes)?;
        if (positions.len() as u64)
            .checked_add(1)
            .and_then(|n| n.checked_add(reserved))
            .ok_or(Fault::Capacity)?
            > self.limits.max_envelopes as u64
            || total.checked_add(reserved_bytes).ok_or(Fault::Capacity)?
                > self.limits.max_retained_wire_bytes as u64
        {
            return Err(Fault::Capacity.into());
        }
        let retained = Retained {
            info: EnvelopeInfo {
                position: InventoryCursor(next),
                id,
                kind,
                record,
                bytes: wire.len(),
            },
            output,
        };
        tx.put(&key(INFO, &id.0), &encode_retained(retained)?)?;
        tx.put(&key(WIRE, &id.0), wire)?;
        tx.put(
            &sequence_key(self.descriptor.group, retained.info.position),
            &id.0,
        )?;
        positions.push(next);
        tx.put(
            &key(index::ENVELOPES, &self.group().0),
            &index::encode_positions(&positions)?,
        )?;
        tx.put(&head_key, &next.to_be_bytes())?;
        tx.put(&bytes_key, &total.to_be_bytes())?;
        // Includes our own generated output: relayed echoes must never be fed
        // through MLS, whose sender cannot decrypt its own consumed ratchet key.
        tx.put(&key(b'd', &id.0), &encode_incoming(result)?)?;
        Ok(())
    }
}

pub(super) fn sequence_key(group: GroupId, cursor: InventoryCursor) -> [u8; 41] {
    let mut key = [0; 41];
    key[0] = SEQUENCE;
    key[1..33].copy_from_slice(&group.0);
    key[33..].copy_from_slice(&cursor.0.to_be_bytes());
    key
}

fn encode_retained(retained: Retained) -> Result<[u8; INFO_BYTES], Fault> {
    let mut bytes = [0; INFO_BYTES];
    bytes[..4].copy_from_slice(INFO_MAGIC);
    bytes[4..12].copy_from_slice(&retained.info.position.0.to_be_bytes());
    bytes[12..44].copy_from_slice(&retained.info.id.0);
    bytes[44] = match retained.info.kind {
        InventoryKind::Record => 1,
        InventoryKind::Receipt => 2,
    };
    bytes[45..77].copy_from_slice(&retained.info.record.0);
    bytes[77..85].copy_from_slice(
        &u64::try_from(retained.info.bytes)
            .map_err(|_| Fault::Capacity)?
            .to_be_bytes(),
    );
    if let Some(operation) = retained.output {
        bytes[85] = 1;
        bytes[86..].copy_from_slice(&operation.0);
    }
    Ok(bytes)
}

pub(super) fn decode_retained(bytes: &[u8]) -> Result<Retained, Fault> {
    if bytes.len() != INFO_BYTES || &bytes[..4] != INFO_MAGIC {
        return Err(Fault::Corrupt);
    }
    let output = match bytes[85] {
        0 if bytes[86..].iter().all(|byte| *byte == 0) => None,
        1 => Some(OperationId(
            bytes[86..].try_into().map_err(|_| Fault::Corrupt)?,
        )),
        _ => return Err(Fault::Corrupt),
    };
    Ok(Retained {
        info: EnvelopeInfo {
            position: InventoryCursor(counter(&bytes[4..12])?),
            id: EnvelopeId(bytes[12..44].try_into().map_err(|_| Fault::Corrupt)?),
            kind: match bytes[44] {
                1 => InventoryKind::Record,
                2 => InventoryKind::Receipt,
                _ => return Err(Fault::Corrupt),
            },
            record: RecordId(bytes[45..77].try_into().map_err(|_| Fault::Corrupt)?),
            bytes: usize::try_from(counter(&bytes[77..85])?).map_err(|_| Fault::Corrupt)?,
        },
        output,
    })
}
