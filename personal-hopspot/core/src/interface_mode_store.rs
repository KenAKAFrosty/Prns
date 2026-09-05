use embedded_storage_async::nor_flash::NorFlash;

use personal_rns::interfaces::InterfaceMode;

use crate::interface_mode::{
    AnnouncesToInternal, InterfaceModeSelection, InterfaceModeSlot, InterfaceModeTable,
    INTERFACE_MODE_SLOT_COUNT,
};

const MAGIC: [u8; 4] = *b"HSIM";
const SCHEMA_VERSION: u16 = 1;
const TABLE_KIND: u8 = 1;
const DEFAULT_KIND: u8 = 2;
const COMMIT_WORD: u32 = 0x5449_4D43;
const RECORD_LEN: usize = 48;
const CHECKSUM_OFFSET: usize = 20;
const COMMIT_OFFSET: usize = 28;
const PAYLOAD_OFFSET: usize = 32;
const SLOT_BYTES: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceModeLoadNotice {
    Recovered,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedInterfaceModes {
    pub table: InterfaceModeTable,
    pub follows_default: bool,
    pub notice: Option<InterfaceModeLoadNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceModeStoreError<E> {
    Flash(E),
    InvalidLayout,
    VerificationFailed,
}

pub struct InterfaceModeStore<F> {
    flash: F,
    pages: [u32; 2],
}

impl<F> InterfaceModeStore<F>
where
    F: NorFlash,
{
    #[must_use]
    pub const fn new(flash: F, pages: [u32; 2]) -> Self {
        Self { flash, pages }
    }

    pub async fn load(
        &mut self,
    ) -> Result<LoadedInterfaceModes, InterfaceModeStoreError<F::Error>> {
        self.validate_layout()?;
        let slots = self.read_slots().await?;
        let Some(active) = select_active(&slots) else {
            let notice = slots
                .iter()
                .any(|slot| !matches!(slot, Slot::Erased))
                .then_some(InterfaceModeLoadNotice::Reset);
            return Ok(LoadedInterfaceModes {
                table: InterfaceModeTable::DEFAULT,
                follows_default: true,
                notice,
            });
        };
        let Some(record) = slots[active].record() else {
            return Ok(LoadedInterfaceModes {
                table: InterfaceModeTable::DEFAULT,
                follows_default: true,
                notice: Some(InterfaceModeLoadNotice::Reset),
            });
        };
        let recovered = match slots[1 - active] {
            Slot::Invalid(Some(generation)) => generation_is_newer(generation, record.generation),
            Slot::Invalid(None) => true,
            Slot::Erased | Slot::Valid(_) => false,
        };
        let (table, follows_default) = match record.value {
            StoredValue::Table(table) => (table, false),
            StoredValue::Default => (InterfaceModeTable::DEFAULT, true),
        };
        Ok(LoadedInterfaceModes {
            table,
            follows_default,
            notice: recovered.then_some(InterfaceModeLoadNotice::Recovered),
        })
    }

    pub async fn save(
        &mut self,
        table: InterfaceModeTable,
    ) -> Result<(), InterfaceModeStoreError<F::Error>> {
        self.commit(StoredValue::Table(table)).await
    }

    pub async fn reset(&mut self) -> Result<(), InterfaceModeStoreError<F::Error>> {
        self.commit(StoredValue::Default).await
    }

    pub fn into_flash(self) -> F {
        self.flash
    }

    async fn commit(
        &mut self,
        value: StoredValue,
    ) -> Result<(), InterfaceModeStoreError<F::Error>> {
        self.validate_layout()?;
        let slots = self.read_slots().await?;
        let active = select_active(&slots);
        let target = active.map_or(0, |index| 1 - index);
        let generation = active
            .and_then(|index| slots[index].record())
            .map_or(0, |record| record.generation.wrapping_add(1));
        let record = encode_record(generation, value);
        let page = self.pages[target];
        self.flash
            .erase(page, page + F::ERASE_SIZE as u32)
            .await
            .map_err(InterfaceModeStoreError::Flash)?;
        self.flash
            .write(page, &record[..COMMIT_OFFSET])
            .await
            .map_err(InterfaceModeStoreError::Flash)?;
        self.flash
            .write(
                page + (COMMIT_OFFSET + 4) as u32,
                &record[COMMIT_OFFSET + 4..],
            )
            .await
            .map_err(InterfaceModeStoreError::Flash)?;
        self.flash
            .write(page + COMMIT_OFFSET as u32, &COMMIT_WORD.to_le_bytes())
            .await
            .map_err(InterfaceModeStoreError::Flash)?;

        let mut verified = [0u8; RECORD_LEN];
        self.flash
            .read(page, &mut verified)
            .await
            .map_err(InterfaceModeStoreError::Flash)?;
        let mut expected = record;
        expected[COMMIT_OFFSET..COMMIT_OFFSET + 4].copy_from_slice(&COMMIT_WORD.to_le_bytes());
        if verified != expected {
            return Err(InterfaceModeStoreError::VerificationFailed);
        }
        Ok(())
    }

    async fn read_slots(&mut self) -> Result<[Slot; 2], InterfaceModeStoreError<F::Error>> {
        let mut slots = [Slot::Erased; 2];
        for (index, page) in self.pages.into_iter().enumerate() {
            let mut bytes = [0u8; RECORD_LEN];
            self.flash
                .read(page, &mut bytes)
                .await
                .map_err(InterfaceModeStoreError::Flash)?;
            slots[index] = if bytes.iter().all(|byte| *byte == 0xFF) {
                Slot::Erased
            } else if let Some(record) = decode_record(&bytes) {
                Slot::Valid(record)
            } else {
                Slot::Invalid(generation_hint(&bytes))
            };
        }
        Ok(slots)
    }

    fn validate_layout(&self) -> Result<(), InterfaceModeStoreError<F::Error>> {
        if F::ERASE_SIZE == 0
            || F::READ_SIZE == 0
            || F::WRITE_SIZE == 0
            || !RECORD_LEN.is_multiple_of(F::READ_SIZE)
            || !COMMIT_OFFSET.is_multiple_of(F::WRITE_SIZE)
            || !4usize.is_multiple_of(F::WRITE_SIZE)
            || !(COMMIT_OFFSET + 4).is_multiple_of(F::WRITE_SIZE)
            || !(RECORD_LEN - COMMIT_OFFSET - 4).is_multiple_of(F::WRITE_SIZE)
            || self.pages[0] == self.pages[1]
        {
            return Err(InterfaceModeStoreError::InvalidLayout);
        }
        for page in self.pages {
            let end = page as usize + F::ERASE_SIZE;
            if !(page as usize).is_multiple_of(F::ERASE_SIZE) || end > self.flash.capacity() {
                return Err(InterfaceModeStoreError::InvalidLayout);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoredValue {
    Table(InterfaceModeTable),
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoredRecord {
    generation: u64,
    value: StoredValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Erased,
    Valid(StoredRecord),
    Invalid(Option<u64>),
}

impl Slot {
    const fn record(self) -> Option<StoredRecord> {
        match self {
            Self::Valid(record) => Some(record),
            Self::Erased | Self::Invalid(_) => None,
        }
    }
}

fn select_active(slots: &[Slot; 2]) -> Option<usize> {
    match (slots[0].record(), slots[1].record()) {
        (Some(first), Some(second)) => Some(
            if generation_is_newer(second.generation, first.generation) {
                1
            } else {
                0
            },
        ),
        (Some(_), None) => Some(0),
        (None, Some(_)) => Some(1),
        (None, None) => None,
    }
}

const fn generation_is_newer(candidate: u64, current: u64) -> bool {
    let delta = candidate.wrapping_sub(current);
    delta != 0 && delta < (1u64 << 63)
}

fn encode_mode(mode: InterfaceMode) -> u8 {
    match mode {
        InterfaceMode::Full => 0,
        InterfaceMode::PointToPoint => 1,
        InterfaceMode::AccessPoint => 2,
        InterfaceMode::Roaming => 3,
        InterfaceMode::Boundary => 4,
        InterfaceMode::Gateway => 5,
        InterfaceMode::Internal => 6,
    }
}

fn decode_mode(value: u8) -> Option<InterfaceMode> {
    Some(match value {
        0 => InterfaceMode::Full,
        1 => InterfaceMode::PointToPoint,
        2 => InterfaceMode::AccessPoint,
        3 => InterfaceMode::Roaming,
        4 => InterfaceMode::Boundary,
        5 => InterfaceMode::Gateway,
        6 => InterfaceMode::Internal,
        _ => return None,
    })
}

fn encode_record(generation: u64, value: StoredValue) -> [u8; RECORD_LEN] {
    let mut bytes = [0xFF; RECORD_LEN];
    bytes[..4].copy_from_slice(&MAGIC);
    bytes[4..6].copy_from_slice(&SCHEMA_VERSION.to_le_bytes());
    bytes[6] = match value {
        StoredValue::Table(_) => TABLE_KIND,
        StoredValue::Default => DEFAULT_KIND,
    };
    bytes[7] = 0;
    bytes[8..16].copy_from_slice(&generation.to_le_bytes());
    match value {
        StoredValue::Table(table) => {
            for (index, slot) in InterfaceModeSlot::ALL.into_iter().enumerate() {
                let selection = table.get(slot);
                let offset = PAYLOAD_OFFSET + index * SLOT_BYTES;
                bytes[offset] = encode_mode(selection.mode);
                bytes[offset + 1] = u8::from(selection.announces_to_internal.allowed());
            }
        }
        StoredValue::Default => {}
    }
    let checksum = checksum(&bytes);
    bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&checksum.to_le_bytes());
    bytes
}

fn decode_record(bytes: &[u8; RECORD_LEN]) -> Option<StoredRecord> {
    if bytes[..4] != MAGIC || u16::from_le_bytes([bytes[4], bytes[5]]) != SCHEMA_VERSION {
        return None;
    }
    let kind = bytes[6];
    let generation = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let stored_checksum = u32::from_le_bytes(
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4]
            .try_into()
            .ok()?,
    );
    let commit = u32::from_le_bytes(bytes[COMMIT_OFFSET..COMMIT_OFFSET + 4].try_into().ok()?);
    if commit != COMMIT_WORD {
        return None;
    }
    let mut probe = *bytes;
    probe[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].fill(0xFF);
    if checksum(&probe) != stored_checksum {
        return None;
    }
    let value = match kind {
        TABLE_KIND => {
            let mut table = InterfaceModeTable::DEFAULT;
            for index in 0..INTERFACE_MODE_SLOT_COUNT {
                let offset = PAYLOAD_OFFSET + index * SLOT_BYTES;
                let mode = decode_mode(bytes[offset])?;
                let announces = AnnouncesToInternal::from_allowed(bytes[offset + 1] != 0);
                let slot = InterfaceModeSlot::from_index(index)?;
                table.set(
                    slot,
                    InterfaceModeSelection {
                        mode,
                        announces_to_internal: announces,
                    },
                );
            }
            StoredValue::Table(table)
        }
        DEFAULT_KIND => StoredValue::Default,
        _ => return None,
    };
    Some(StoredRecord { generation, value })
}

fn generation_hint(bytes: &[u8; RECORD_LEN]) -> Option<u64> {
    if bytes[..4] != MAGIC {
        return None;
    }
    Some(u64::from_le_bytes(bytes[8..16].try_into().ok()?))
}

fn checksum(bytes: &[u8; RECORD_LEN]) -> u32 {
    let mut sum = 0u32;
    for (index, byte) in bytes.iter().enumerate() {
        if (CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4).contains(&index)
            || (COMMIT_OFFSET..COMMIT_OFFSET + 4).contains(&index)
        {
            continue;
        }
        sum = sum.wrapping_add(u32::from(*byte).wrapping_mul((index as u32).wrapping_add(1)));
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::Future;
    use core::task::{Context, Poll};
    use embedded_storage_async::nor_flash::ReadNorFlash;
    use std::task::Waker;

    const PAGES: [u32; 2] = [0, 4096];

    struct RamFlash {
        bytes: [u8; 8192],
    }

    impl RamFlash {
        fn new() -> Self {
            Self {
                bytes: [0xFF; 8192],
            }
        }
    }

    impl embedded_storage_async::nor_flash::ErrorType for RamFlash {
        type Error = core::convert::Infallible;
    }

    impl ReadNorFlash for RamFlash {
        const READ_SIZE: usize = 1;

        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            let start = offset as usize;
            bytes.copy_from_slice(&self.bytes[start..start + bytes.len()]);
            Ok(())
        }

        fn capacity(&self) -> usize {
            self.bytes.len()
        }
    }

    impl NorFlash for RamFlash {
        const WRITE_SIZE: usize = 1;
        const ERASE_SIZE: usize = 4096;

        async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            self.bytes[from as usize..to as usize].fill(0xFF);
            Ok(())
        }

        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            let start = offset as usize;
            self.bytes[start..start + bytes.len()].copy_from_slice(bytes);
            Ok(())
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut future = core::pin::pin!(future);
        let mut context = Context::from_waker(waker);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => {}
            }
        }
    }

    #[test]
    fn round_trips_a_boundary_selection() {
        let mut store = InterfaceModeStore::new(RamFlash::new(), PAGES);
        let mut table = InterfaceModeTable::DEFAULT;
        table.set(
            InterfaceModeSlot::LoRa,
            InterfaceModeSelection {
                mode: InterfaceMode::Boundary,
                announces_to_internal: AnnouncesToInternal::Allowed,
            },
        );
        block_on(store.save(table)).expect("save");
        let loaded = block_on(store.load()).expect("load");
        assert_eq!(
            loaded.table.get(InterfaceModeSlot::LoRa).mode,
            InterfaceMode::Boundary
        );
        assert!(loaded
            .table
            .get(InterfaceModeSlot::LoRa)
            .announces_to_internal
            .allowed());
        assert!(!loaded.follows_default);
    }
}
