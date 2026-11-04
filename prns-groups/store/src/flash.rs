//! Append-only NOR journal for an exclusively owned, pre-erased partition.
//!
//! Each fixed-size slot publishes one transaction and receipt with a final write.
//! There is no compaction or implicit erase: full storage rejects further commits.
//! A torn body with an erased publication marker is retired. A partial publication
//! marker or invalid published record fails closed, including during recovery.
//! CRC detects accidental corruption; it is not authentication or rollback defense.
//! Deleted and superseded secrets remain physically present until external erase.

use alloc::vec::Vec;
use core::fmt;
use embedded_storage::nor_flash::NorFlash;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    AtomicBackend, CommitOutcome, CommitReceipt, Mutation, OperationId, RecoveryOutcome, Revision,
};

/// The final, separately programmed block at the end of every slot.
pub const COMMIT_MARKER_BYTES: usize = 256;
const BLOCK: usize = COMMIT_MARKER_BYTES;
const HEADER: usize = 64;
const ENTRY: usize = 12;
const MAGIC: &[u8; 8] = b"PRNSGR01";
const COMMITTED: [u8; BLOCK] = [0x3c; BLOCK];

/// Caller-owned flash range. No other owner may read/write through this backend
/// concurrently, or erase any part of this range while receipts remain relevant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Partition {
    pub offset: u32,
    pub length: u32,
}

/// Durable encoding limits; these must remain unchanged when reopening a journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashLimits {
    pub slot_bytes: u32,
    pub max_changes: usize,
    pub max_key_bytes: usize,
    pub max_value_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashUsage {
    pub committed_slots: usize,
    pub retired_slots: usize,
    pub total_slots: usize,
    /// Slots preceding this offset will never be overwritten, including holes.
    pub next_slot: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FlashError<E> {
    Io(E),
    InvalidGeometry,
    AllocationFailed,
    Full,
    TransactionTooLarge,
    KeyTooLarge,
    ValueTooLarge,
    TooManyChanges,
    CounterExhausted,
    /// Published state or a publication marker cannot be trusted. Never interpret
    /// this as an empty store or as proof that an operation did not commit.
    Corrupt {
        slot: usize,
    },
}

impl<E: fmt::Debug> fmt::Display for FlashError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "group flash error: {self:?}")
    }
}

impl<E: fmt::Debug> core::error::Error for FlashError<E> {}

#[derive(Clone, Copy)]
struct Record {
    slot: usize,
    receipt: CommitReceipt,
    payload_bytes: usize,
    changes: usize,
}

/// Metadata-only RAM index: one entry per committed slot, bounded by partition
/// length / slot size. Values are read directly from flash without state cloning.
/// The driver must complete writes durably before returning success and enforce
/// normal NOR semantics. Exclusive physical partition ownership is mandatory.
pub struct FlashBackend<F: NorFlash> {
    flash: F,
    partition: Partition,
    limits: FlashLimits,
    records: Vec<Record>,
    usage: FlashUsage,
    dirty: bool,
}

impl<F: NorFlash> FlashBackend<F> {
    /// Mount a journal, accepting completely erased storage as revision zero.
    /// The application must separately validate its partition table/version.
    /// Never silently erase/reinitialize on an error from this function.
    pub fn mount(
        flash: F,
        partition: Partition,
        limits: FlashLimits,
    ) -> Result<Self, FlashError<F::Error>> {
        let slot = limits.slot_bytes as usize;
        let start = partition.offset as usize;
        let len = partition.length as usize;
        if F::READ_SIZE == 0
            || F::WRITE_SIZE == 0
            || F::ERASE_SIZE == 0
            || !BLOCK.is_multiple_of(F::READ_SIZE)
            || !BLOCK.is_multiple_of(F::WRITE_SIZE)
            || !start.is_multiple_of(F::ERASE_SIZE)
            || !start.is_multiple_of(BLOCK)
            || slot < F::ERASE_SIZE
            || !slot.is_multiple_of(F::ERASE_SIZE)
            || !slot.is_multiple_of(BLOCK)
            || slot < HEADER + 4 + BLOCK
            || len == 0
            || !len.is_multiple_of(slot)
            || start
                .checked_add(len)
                .is_none_or(|end| end > flash.capacity() || end > u32::MAX as usize)
            || limits.max_changes > u32::MAX as usize
            || limits.max_key_bytes > u32::MAX as usize
            || limits.max_value_bytes > u32::MAX as usize
        {
            return Err(FlashError::InvalidGeometry);
        }
        let total_slots = len / slot;
        let mut records = Vec::new();
        records
            .try_reserve_exact(total_slots)
            .map_err(|_| FlashError::AllocationFailed)?;
        let mut backend = Self {
            flash,
            partition,
            limits,
            records,
            usage: FlashUsage {
                committed_slots: 0,
                retired_slots: 0,
                total_slots,
                next_slot: 0,
            },
            dirty: true,
        };
        backend.scan()?;
        Ok(backend)
    }

    pub fn into_inner(self) -> F {
        self.flash
    }

    /// Last successful scan/commit accounting. After an I/O error call `recover`
    /// before relying on this information; it is not a durability receipt.
    pub fn usage(&self) -> FlashUsage {
        self.usage
    }

    fn revision(&self) -> Revision {
        self.records
            .last()
            .map_or(Revision(0), |record| record.receipt.revision)
    }

    fn offset(&self, slot: usize) -> u32 {
        // Mount checks the complete partition fits both usize and u32.
        self.partition.offset + (slot * self.limits.slot_bytes as usize) as u32
    }

    fn refresh(&mut self) -> Result<(), FlashError<F::Error>> {
        if self.dirty {
            self.scan()?;
        }
        Ok(())
    }

    fn scan(&mut self) -> Result<(), FlashError<F::Error>> {
        self.dirty = true;
        self.records.clear();
        self.usage.committed_slots = 0;
        self.usage.retired_slots = 0;
        self.usage.next_slot = 0;
        for slot in 0..self.usage.total_slots {
            let start = self.offset(slot);
            let marker_offset = start + self.limits.slot_bytes - BLOCK as u32;
            let mut marker = Zeroizing::new([0u8; BLOCK]);
            self.flash
                .read(marker_offset, marker.as_mut())
                .map_err(FlashError::Io)?;
            if *marker == COMMITTED {
                let record = self.validate(slot)?;
                if Some(record.receipt.revision) != self.revision().next()
                    || self
                        .records
                        .iter()
                        .any(|prior| prior.receipt.operation_id == record.receipt.operation_id)
                {
                    return Err(FlashError::Corrupt { slot });
                }
                self.records.push(record);
                self.usage.committed_slots += 1;
                self.usage.next_slot = slot + 1;
            } else if marker.iter().any(|byte| *byte != 0xff) {
                // Partial programming and corruption of a previously published
                // marker cannot be distinguished. Fail closed in both cases.
                return Err(FlashError::Corrupt { slot });
            } else if !self.erased(start, self.limits.slot_bytes as usize - BLOCK)? {
                self.usage.retired_slots += 1;
                self.usage.next_slot = slot + 1;
            }
        }
        self.dirty = false;
        Ok(())
    }

    fn erased(&mut self, start: u32, bytes: usize) -> Result<bool, FlashError<F::Error>> {
        let mut block = Zeroizing::new([0u8; BLOCK]);
        for offset in (0..bytes).step_by(BLOCK) {
            self.flash
                .read(start + offset as u32, block.as_mut())
                .map_err(FlashError::Io)?;
            if block.iter().any(|byte| *byte != 0xff) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn read_bytes(
        &mut self,
        mut offset: u32,
        mut out: &mut [u8],
    ) -> Result<(), FlashError<F::Error>> {
        let mut block = Zeroizing::new([0u8; BLOCK]);
        while !out.is_empty() {
            let within = offset as usize % BLOCK;
            self.flash
                .read(offset - within as u32, block.as_mut())
                .map_err(FlashError::Io)?;
            let count = out.len().min(BLOCK - within);
            out[..count].copy_from_slice(&block[within..within + count]);
            offset += count as u32;
            out = &mut out[count..];
        }
        Ok(())
    }

    fn validate(&mut self, slot: usize) -> Result<Record, FlashError<F::Error>> {
        let start = self.offset(slot);
        let mut marker = [0u8; BLOCK];
        self.flash
            .read(start + self.limits.slot_bytes - BLOCK as u32, &mut marker)
            .map_err(FlashError::Io)?;
        if marker != COMMITTED {
            return Err(FlashError::Corrupt { slot });
        }
        let mut header = [0u8; HEADER];
        self.read_bytes(start, &mut header)?;
        let payload_bytes = word(&header[12..16]) as usize;
        let changes = word(&header[56..60]) as usize;
        let body_bytes = HEADER
            .checked_add(payload_bytes)
            .and_then(|n| n.checked_add(4));
        if &header[..8] != MAGIC
            || word(&header[8..12]) != 1
            || word(&header[60..]) != checksum(&header[..60])
            || body_bytes.is_none_or(|n| n > self.limits.slot_bytes as usize - BLOCK)
            || changes > self.limits.max_changes
        {
            return Err(FlashError::Corrupt { slot });
        }
        let mut operation = [0u8; 32];
        operation.copy_from_slice(&header[24..56]);
        let mut revision = [0u8; 8];
        revision.copy_from_slice(&header[16..24]);
        let record = Record {
            slot,
            receipt: CommitReceipt {
                operation_id: OperationId(operation),
                revision: Revision(u64::from_le_bytes(revision)),
            },
            payload_bytes,
            changes,
        };
        let mut crc = Crc::new();
        crc.update(&header);
        let mut block = Zeroizing::new([0u8; BLOCK]);
        let mut cursor = 0;
        while cursor < payload_bytes {
            let count = BLOCK.min(payload_bytes - cursor);
            self.read_bytes(start + HEADER as u32 + cursor as u32, &mut block[..count])?;
            crc.update(&block[..count]);
            cursor += count;
        }
        let mut stored_crc = [0u8; 4];
        self.read_bytes(
            start + HEADER as u32 + payload_bytes as u32,
            &mut stored_crc,
        )?;
        if crc.finish() != word(&stored_crc) {
            return Err(FlashError::Corrupt { slot });
        }
        // Verify the entire structure even if no caller reads a particular key.
        let mut cursor = 0;
        for _ in 0..changes {
            let (_, _, _, next) = self.entry(record, cursor)?;
            cursor = next;
        }
        if cursor != payload_bytes {
            return Err(FlashError::Corrupt { slot });
        }
        Ok(record)
    }

    fn entry(
        &mut self,
        record: Record,
        cursor: usize,
    ) -> Result<(bool, usize, usize, usize), FlashError<F::Error>> {
        let fail = || FlashError::Corrupt { slot: record.slot };
        if cursor
            .checked_add(ENTRY)
            .is_none_or(|n| n > record.payload_bytes)
        {
            return Err(fail());
        }
        let mut entry = [0u8; ENTRY];
        self.read_bytes(
            self.offset(record.slot) + (HEADER + cursor) as u32,
            &mut entry,
        )?;
        let key_bytes = word(&entry[4..8]) as usize;
        let value_bytes = word(&entry[8..12]) as usize;
        let next = cursor
            .checked_add(ENTRY)
            .and_then(|n| n.checked_add(key_bytes))
            .and_then(|n| n.checked_add(value_bytes))
            .ok_or_else(fail)?;
        if entry[0] > 1
            || entry[1..4] != [0, 0, 0]
            || (entry[0] == 0 && value_bytes != 0)
            || key_bytes > self.limits.max_key_bytes
            || value_bytes > self.limits.max_value_bytes
            || next > record.payload_bytes
        {
            return Err(fail());
        }
        Ok((entry[0] == 1, key_bytes, value_bytes, next))
    }

    fn key_matches(&mut self, offset: u32, key: &[u8]) -> Result<bool, FlashError<F::Error>> {
        let mut block = Zeroizing::new([0u8; BLOCK]);
        for (index, chunk) in key.chunks(BLOCK).enumerate() {
            self.read_bytes(offset + (index * BLOCK) as u32, &mut block[..chunk.len()])?;
            if &block[..chunk.len()] != chunk {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn encode_header(
        &self,
        revision: Revision,
        operation: OperationId,
        changes: &[Mutation],
    ) -> Result<[u8; HEADER], FlashError<F::Error>> {
        if changes.len() > self.limits.max_changes {
            return Err(FlashError::TooManyChanges);
        }
        let mut payload_bytes = 0usize;
        for change in changes {
            if change.key().len() > self.limits.max_key_bytes {
                return Err(FlashError::KeyTooLarge);
            }
            let value_bytes = change.value().map_or(0, <[u8]>::len);
            if value_bytes > self.limits.max_value_bytes {
                return Err(FlashError::ValueTooLarge);
            }
            payload_bytes = payload_bytes
                .checked_add(ENTRY)
                .and_then(|n| n.checked_add(change.key().len()))
                .and_then(|n| n.checked_add(value_bytes))
                .ok_or(FlashError::TransactionTooLarge)?;
        }
        if payload_bytes
            .checked_add(HEADER + 4)
            .is_none_or(|n| n > self.limits.slot_bytes as usize - BLOCK)
        {
            return Err(FlashError::TransactionTooLarge);
        }
        let mut header = [0u8; HEADER];
        header[..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&1u32.to_le_bytes());
        header[12..16].copy_from_slice(&(payload_bytes as u32).to_le_bytes());
        header[16..24].copy_from_slice(&revision.0.to_le_bytes());
        header[24..56].copy_from_slice(&operation.0);
        header[56..60].copy_from_slice(&(changes.len() as u32).to_le_bytes());
        let crc = checksum(&header[..60]);
        header[60..].copy_from_slice(&crc.to_le_bytes());
        Ok(header)
    }

    fn append(
        &mut self,
        slot: usize,
        header: &[u8; HEADER],
        changes: &[Mutation],
    ) -> Result<(), FlashError<F::Error>> {
        let start = self.offset(slot);
        let mut writer = Writer::new(&mut self.flash, start);
        let mut crc = Crc::new();
        crc.update(header);
        writer.bytes(header)?;
        for change in changes {
            let mut entry = [0u8; ENTRY];
            entry[0] = u8::from(change.value().is_some());
            entry[4..8].copy_from_slice(&(change.key().len() as u32).to_le_bytes());
            entry[8..12]
                .copy_from_slice(&(change.value().map_or(0, <[u8]>::len) as u32).to_le_bytes());
            for bytes in [
                entry.as_slice(),
                change.key(),
                change.value().unwrap_or(&[]),
            ] {
                crc.update(bytes);
                writer.bytes(bytes)?;
            }
        }
        writer.bytes(&crc.finish().to_le_bytes())?;
        writer.flush()?;
        drop(writer);
        // Only this last write publishes all mutations and the receipt together.
        self.flash
            .write(start + self.limits.slot_bytes - BLOCK as u32, &COMMITTED)
            .map_err(FlashError::Io)
    }
}

impl<F: NorFlash> AtomicBackend for FlashBackend<F> {
    type Error = FlashError<F::Error>;

    fn head(&mut self) -> Result<Revision, Self::Error> {
        self.refresh()?;
        Ok(self.revision())
    }

    fn read(&mut self, key: &[u8], max_bytes: usize) -> Result<Option<Vec<u8>>, Self::Error> {
        self.refresh()?;
        if key.len() > self.limits.max_key_bytes {
            return Err(FlashError::KeyTooLarge);
        }
        let result = (|| {
            for index in (0..self.records.len()).rev() {
                let record = self.validate(self.records[index].slot)?;
                let mut cursor = 0;
                let mut found = None;
                for _ in 0..record.changes {
                    let (put, key_bytes, value_bytes, next) = self.entry(record, cursor)?;
                    let offset = self.offset(record.slot) + (HEADER + cursor + ENTRY) as u32;
                    if key.len() == key_bytes && self.key_matches(offset, key)? {
                        found = Some((put, offset + key_bytes as u32, value_bytes));
                    }
                    cursor = next;
                }
                if let Some((put, offset, bytes)) = found {
                    if !put {
                        return Ok(None);
                    }
                    if bytes > max_bytes {
                        return Err(FlashError::ValueTooLarge);
                    }
                    let mut value = Vec::new();
                    value
                        .try_reserve_exact(bytes)
                        .map_err(|_| FlashError::AllocationFailed)?;
                    value.resize(bytes, 0);
                    if let Err(error) = self.read_bytes(offset, &mut value) {
                        value.zeroize();
                        return Err(error);
                    }
                    return Ok(Some(value));
                }
            }
            Ok(None)
        })();
        if result.is_err() {
            // A detected integrity/read failure must be revalidated before any
            // subsequent use of the cached revision and receipt index.
            self.dirty = true;
        }
        result
    }

    fn commit(
        &mut self,
        expected: Revision,
        operation: OperationId,
        changes: &[Mutation],
    ) -> Result<CommitOutcome, Self::Error> {
        self.refresh()?;
        if let Some(record) = self
            .records
            .iter()
            .find(|record| record.receipt.operation_id == operation)
        {
            return Ok(CommitOutcome::Committed(record.receipt));
        }
        if expected != self.revision() {
            return Ok(CommitOutcome::NotCommitted {
                revision: self.revision(),
            });
        }
        let revision = expected.next().ok_or(FlashError::CounterExhausted)?;
        let header = self.encode_header(revision, operation, changes)?;
        let slot = self.usage.next_slot;
        if slot == self.usage.total_slots {
            return Err(FlashError::Full);
        }
        self.dirty = true;
        self.append(slot, &header, changes)?;
        // Read back before reporting durability. Any failure keeps recovery fenced.
        let mut marker = [0u8; BLOCK];
        self.flash
            .read(
                self.offset(slot) + self.limits.slot_bytes - BLOCK as u32,
                &mut marker,
            )
            .map_err(FlashError::Io)?;
        let record = self.validate(slot)?;
        let receipt = CommitReceipt {
            operation_id: operation,
            revision,
        };
        if marker != COMMITTED || record.receipt != receipt {
            return Err(FlashError::Corrupt { slot });
        }
        self.records.push(record);
        self.usage.committed_slots += 1;
        self.usage.next_slot = slot + 1;
        self.dirty = false;
        Ok(CommitOutcome::Committed(receipt))
    }

    fn recover(&mut self, operation: OperationId) -> Result<RecoveryOutcome, Self::Error> {
        self.scan()?;
        Ok(
            match self
                .records
                .iter()
                .find(|record| record.receipt.operation_id == operation)
            {
                Some(record) => RecoveryOutcome::Committed(record.receipt),
                None => RecoveryOutcome::NotCommitted {
                    revision: self.revision(),
                },
            },
        )
    }
}

struct Writer<'a, F: NorFlash> {
    flash: &'a mut F,
    offset: u32,
    block: Zeroizing<[u8; BLOCK]>,
    used: usize,
}

impl<'a, F: NorFlash> Writer<'a, F> {
    fn new(flash: &'a mut F, offset: u32) -> Self {
        Self {
            flash,
            offset,
            block: Zeroizing::new([0xff; BLOCK]),
            used: 0,
        }
    }

    fn bytes(&mut self, mut bytes: &[u8]) -> Result<(), FlashError<F::Error>> {
        while !bytes.is_empty() {
            let count = bytes.len().min(BLOCK - self.used);
            self.block[self.used..self.used + count].copy_from_slice(&bytes[..count]);
            self.used += count;
            bytes = &bytes[count..];
            if self.used == BLOCK {
                self.flush()?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), FlashError<F::Error>> {
        if self.used != 0 {
            self.flash
                .write(self.offset, self.block.as_ref())
                .map_err(FlashError::Io)?;
            self.offset += BLOCK as u32;
            self.block.fill(0xff);
            self.used = 0;
        }
        Ok(())
    }
}

fn word(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

struct Crc(u32);
impl Crc {
    fn new() -> Self {
        Self(u32::MAX)
    }
    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u32::from(*byte);
            for _ in 0..8 {
                self.0 = (self.0 >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(self.0 & 1)));
            }
        }
    }
    fn finish(self) -> u32 {
        !self.0
    }
}
fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(bytes);
    crc.finish()
}

#[cfg(test)]
mod tests;
