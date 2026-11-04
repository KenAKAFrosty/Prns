//! Versioned live positions. Heads remain monotonic; capacity counts retained
//! entries, not the number of operations performed since enrollment.
use super::*;

pub(super) const CHANGES: u8 = b'L';
pub(super) const ENVELOPES: u8 = b'V';

pub(super) fn decode_positions(bytes: &[u8], head: u64, bound: usize) -> Result<Vec<u64>, Fault> {
    if bytes.first() != Some(&1) || !(bytes.len() - 1).is_multiple_of(8) {
        return Err(Fault::Corrupt);
    }
    let count = (bytes.len() - 1) / 8;
    if count > bound {
        return Err(Fault::Corrupt);
    }
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(count)
        .map_err(|_| Fault::Allocation)?;
    for bytes in bytes[1..].as_chunks::<8>().0 {
        let position = counter(bytes)?;
        if position == 0 || position > head || positions.last().is_some_and(|p| *p >= position) {
            return Err(Fault::Corrupt);
        }
        positions.push(position);
    }
    Ok(positions)
}

pub(super) fn encode_positions(positions: &[u64]) -> Result<Vec<u8>, Fault> {
    let size = positions
        .len()
        .checked_mul(8)
        .and_then(|n| n.checked_add(1))
        .ok_or(Fault::Capacity)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| Fault::Allocation)?;
    bytes.push(1);
    for position in positions {
        bytes.extend_from_slice(&position.to_be_bytes());
    }
    Ok(bytes)
}

fn legacy_positions(head: u64, bound: usize) -> Result<Vec<u64>, Fault> {
    let count = usize::try_from(head).map_err(|_| Fault::Corrupt)?;
    if count > bound {
        return Err(Fault::Corrupt);
    }
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(count)
        .map_err(|_| Fault::Allocation)?;
    positions.extend(1..=head);
    Ok(positions)
}

impl<B: AtomicBackend + Send> GroupService<'_, B> {
    pub(super) fn positions(
        &self,
        tag: u8,
        head_tag: u8,
        bound: usize,
    ) -> ServiceResult<Vec<u64>, B> {
        let head = self
            .read(&key(head_tag, &self.group().0))?
            .map(|b| counter(&b))
            .transpose()?
            .unwrap_or(0);
        Ok(match self.read(&key(tag, &self.group().0))? {
            Some(bytes) => decode_positions(&bytes, head, bound)?,
            None => legacy_positions(head, bound)?,
        })
    }

    pub(super) fn tx_positions(
        &self,
        tx: &ApplicationTransaction<'_, B>,
        tag: u8,
        head_tag: u8,
        bound: usize,
    ) -> ServiceResult<Vec<u64>, B> {
        let head = tx
            .read(&key(head_tag, &self.group().0))?
            .map(|b| counter(&b))
            .transpose()?
            .unwrap_or(0);
        Ok(match tx.read(&key(tag, &self.group().0))? {
            Some(bytes) => decode_positions(&bytes, head, bound)?,
            None => legacy_positions(head, bound)?,
        })
    }

    pub(super) fn initialize_indexes(
        &self,
        tx: &ApplicationTransaction<'_, B>,
    ) -> ServiceResult<(), B> {
        for (tag, head, bound) in [
            (CHANGES, b'c', self.limits.max_changes),
            (ENVELOPES, b'H', self.limits.max_envelopes),
        ] {
            let positions = self.tx_positions(tx, tag, head, bound)?;
            if tx.read(&key(tag, &self.group().0))?.is_none() {
                tx.put(&key(tag, &self.group().0), &encode_positions(&positions)?)?;
            }
        }
        Ok(())
    }
}
