//! Read-only durable obligations beneath content profiles and transport policy.
use super::*;

/// Facts from this group's bounded, retained record/change indexes. Active
/// counts are conservative: observing a wall clock never clears an obligation.
/// Only authenticated receipt flags, removal by maintenance, or an authorization
/// barrier/terminal MLS fence can remove the counted network work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DurableDependencies {
    pub retired: bool,
    pub pending_authored_records: u64,
    pub pending_authored_targets: u64,
    /// Received or locally targeted records still needing receipt emission.
    pub pending_receipts: u64,
    /// Passive retained plaintext records, including old authorization eras.
    pub retained_records: u64,
    /// Passive indexed result changes, not every backend receipt or MLS output.
    /// Duplicate receives can retain a result without appending a change.
    pub retained_result_changes: u64,
}

impl<B: AtomicBackend + Send> GroupService<'_, B> {
    /// Inspect committed facts without maintenance, writes, clocks or traffic.
    /// The exclusive owner serializes this with mutation and binds any broader
    /// dependency report to its domain revision. Error means incomplete evidence.
    /// This does not assert that a native runtime, carrier or key owner drained.
    pub fn dependencies(&self) -> ServiceResult<DurableDependencies, B> {
        self.check_configuration()?;
        let retired = self
            .adapter
            .is_retired(self.group())
            .map_err(Error::Security)?;
        let positions = self.positions(index::CHANGES, b'c', self.limits.max_changes)?;
        let mut result = DurableDependencies {
            retired,
            retained_result_changes: positions.len() as u64,
            ..DurableDependencies::default()
        };
        let mut records = Vec::new();
        records
            .try_reserve_exact(self.limits.max_records.min(positions.len()))
            .map_err(|_| Fault::Allocation)?;
        for position in positions {
            let bytes = self
                .read(&feed_key(self.group(), ChangeCursor(position)))?
                .ok_or(Fault::Corrupt)?;
            let change = decode_change(&bytes)?;
            if change.cursor.0 != position {
                return Err(Fault::Corrupt.into());
            }
            let (id, authored) = match change.kind {
                ChangeKind::Published(id) => (id, true),
                ChangeKind::Received(id) => (id, false),
                _ => continue,
            };
            if records.len() >= self.limits.max_records || records.contains(&id) {
                return Err(Fault::Corrupt.into());
            }
            records.push(id);
            let record = self.record(id)?.ok_or(Fault::Corrupt)?;
            if authored && record.metadata.author != self.local {
                return Err(Fault::Corrupt.into());
            }
            let flags = self
                .read(&record_key(b'p', self.group(), id))?
                .ok_or(Fault::Corrupt)?;
            validate_flags(&flags, record.targets.len())?;
            let active = !retired
                && (!self.managed
                    || record.metadata.original_control.era == self.descriptor.control.era);
            if authored && active {
                let pending = flags.iter().filter(|flag| **flag == 0).count() as u64;
                if pending != 0 {
                    result.pending_authored_records = result
                        .pending_authored_records
                        .checked_add(1)
                        .ok_or(Fault::Corrupt)?;
                    result.pending_authored_targets = result
                        .pending_authored_targets
                        .checked_add(pending)
                        .ok_or(Fault::Corrupt)?;
                }
            }
            let receipt = self.read(&record_key(b'q', self.group(), id))?;
            match (
                record
                    .targets
                    .iter()
                    .position(|target| *target == self.local),
                receipt.as_ref().map(|bytes| bytes.as_slice()),
            ) {
                (None, None) => (),
                (Some(_), Some(bytes)) if bytes == [0] => {
                    if active {
                        result.pending_receipts = result
                            .pending_receipts
                            .checked_add(1)
                            .ok_or(Fault::Corrupt)?;
                    }
                }
                (Some(target), Some(bytes)) if bytes.len() == 33 && bytes[0] == 1 => {
                    let operation = OperationId(bytes[1..].try_into().map_err(|_| Fault::Corrupt)?);
                    let fact = self.read(&key(b'o', &operation.0))?.ok_or(Fault::Corrupt)?;
                    if flags[target] != 1
                        || !matches!(decode_fact(&fact)?.result,ResultFact::ReceiptQueued(record) if record==id)
                    {
                        return Err(Fault::Corrupt.into());
                    }
                }
                _ => return Err(Fault::Corrupt.into()),
            }
        }
        result.retained_records = records.len() as u64;
        let recorded = self
            .read(&key(b'n', &self.group().0))?
            .map(|bytes| counter(&bytes))
            .transpose()?
            .unwrap_or(0);
        if recorded != result.retained_records {
            return Err(Fault::Corrupt.into());
        }
        Ok(result)
    }
}
