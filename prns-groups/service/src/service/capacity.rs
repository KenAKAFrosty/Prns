//! Logical capacity owed to the first authenticated receipt from every target.
//! These counters reserve service quotas, not bytes in the caller's physical
//! medium. Backend/MLS resource bounds must still cover the chosen service policy.

use super::*;

pub(super) const RESERVED: u8 = b'R';
const VERSION: u8 = 1;

impl<B: AtomicBackend + Send> GroupService<'_, B> {
    /// MLS result capacity is shared by every group using this adapter. Count
    /// other groups' obligations from the same transaction snapshot. A missing
    /// counter must be reconstructed before another group can spend capacity
    /// that may already be owed to it.
    pub(super) fn adapter_reserved_receipts(
        &self,
        tx: &ApplicationTransaction<'_, B>,
    ) -> ServiceResult<u64, B> {
        let mut reserved = self.reserved_receipts(tx)?;
        for group in tx.groups()? {
            if group == self.group() {
                continue;
            }
            let Some(bytes) = tx.read(&key(RESERVED, &group.0))? else {
                if tx.read(&key(b'g', &group.0))?.is_some() {
                    return Err(Fault::Capacity.into());
                }
                // A newly created MLS group need not yet have any service data.
                if tx.read(&key(b'n', &group.0))?.is_some()
                    || tx.read(&key(b'c', &group.0))?.is_some()
                {
                    return Err(Fault::Corrupt.into());
                }
                continue;
            };
            if bytes.len() != 9 || bytes[0] != VERSION {
                return Err(Fault::Corrupt.into());
            }
            reserved = reserved
                .checked_add(counter(&bytes[1..])?)
                .ok_or(Fault::Capacity)?;
        }
        Ok(reserved)
    }

    pub(super) fn reserved_receipts(
        &self,
        tx: &ApplicationTransaction<'_, B>,
    ) -> ServiceResult<u64, B> {
        let bytes = tx
            .read(&key(RESERVED, &self.descriptor.group.0))?
            .ok_or(Fault::Corrupt)?;
        if bytes.len() != 9 || bytes[0] != VERSION {
            return Err(Fault::Corrupt.into());
        }
        Ok(counter(&bytes[1..])?)
    }

    pub(super) fn put_reserved(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        count: u64,
    ) -> ServiceResult<(), B> {
        let mut bytes = [VERSION; 9];
        bytes[1..].copy_from_slice(&count.to_be_bytes());
        tx.put(&key(RESERVED, &self.descriptor.group.0), &bytes)?;
        Ok(())
    }

    /// Initialize a missing reservation counter or recompute it after an
    /// authorization-era change from authoritative records and receipt flags.
    /// The update shares the next operation's transaction; failure changes nothing.
    pub(super) fn initialize_reservations(
        &self,
        tx: &ApplicationTransaction<'_, B>,
    ) -> ServiceResult<(), B> {
        let era_key = key(b'E', &self.descriptor.group.0);
        let era_matches = !self.managed
            || tx
                .read(&era_key)?
                .is_some_and(|value| value.as_slice() == self.descriptor.control.era.0 .0);
        if era_matches && tx.read(&key(RESERVED, &self.descriptor.group.0))?.is_some() {
            self.reserved_receipts(tx)?;
            return Ok(());
        }
        let positions = self.tx_positions(tx, index::CHANGES, b'c', self.limits.max_changes)?;
        let mut reserved = 0u64;
        for position in positions {
            let bytes = tx
                .read(&feed_key(self.descriptor.group, ChangeCursor(position)))?
                .ok_or(Fault::Corrupt)?;
            let change = decode_change(&bytes)?;
            if change.cursor.0 != position {
                return Err(Fault::Corrupt.into());
            }
            let id = match change.kind {
                ChangeKind::Published(id) | ChangeKind::Received(id) => id,
                _ => continue,
            };
            let bytes = tx
                .read(&record_key(b'r', self.descriptor.group, id))?
                .ok_or(Fault::Corrupt)?;
            let Packet::Record(record) = codec::decode(&bytes, self.limits)? else {
                return Err(Fault::Corrupt.into());
            };
            if record.metadata.record != id || record.metadata.group != self.descriptor.group {
                return Err(Fault::Corrupt.into());
            }
            if self.managed && record.metadata.original_control.era != self.descriptor.control.era {
                continue;
            }
            let flags = tx
                .read(&record_key(b'p', self.descriptor.group, id))?
                .ok_or(Fault::Corrupt)?;
            validate_flags(&flags, record.targets.len())?;
            let pending = flags.iter().filter(|flag| **flag == 0).count() as u64;
            reserved = reserved.checked_add(pending).ok_or(Fault::Capacity)?;
        }
        self.put_reserved(tx, reserved)?;
        if self.managed {
            tx.put(&era_key, &self.descriptor.control.era.0 .0)?;
        }
        Ok(())
    }

    pub(super) fn reserve_receipts(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        targets: usize,
    ) -> ServiceResult<(), B> {
        let count = self
            .reserved_receipts(tx)?
            .checked_add(u64::try_from(targets).map_err(|_| Fault::Capacity)?)
            .ok_or(Fault::Capacity)?;
        self.put_reserved(tx, count)
    }

    pub(super) fn release_receipt(
        &self,
        tx: &ApplicationTransaction<'_, B>,
    ) -> ServiceResult<(), B> {
        let count = self
            .reserved_receipts(tx)?
            .checked_sub(1)
            .ok_or(Fault::Corrupt)?;
        self.put_reserved(tx, count)
    }
}
