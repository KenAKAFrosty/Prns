use embedded_storage_async::nor_flash::NorFlash;
use personal_rns::interfaces::lora::{
    CodingRate, Frequency, LoraBandwidth, Modulation, PreambleSymbols, RadioProfile, Region,
    SpreadingFactor, TxPower,
};

const MAGIC: [u8; 4] = *b"HSLP";
const SCHEMA_VERSION: u16 = 1;
const PROFILE_KIND: u8 = 1;
const DEFAULT_KIND: u8 = 2;
const COMMIT_WORD: u32 = 0x5449_4D43;
const RECORD_LEN: usize = 48;
const BLE_GROUP_OFFSET: usize = 64;
const BLE_GROUP_RECORD_LEN: usize = 32;
const BLE_GROUP_MAGIC: [u8; 4] = *b"HSBG";
const BLE_GROUP_VERSION: u16 = 1;
const BLE_GROUP_NAME_MAX: usize = 16;
const BLE_GROUP_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789-_";
const PROFILE_PAYLOAD_LEN: usize = 12;
const CHECKSUM_OFFSET: usize = 20;
const COMMIT_OFFSET: usize = 28;
const PAYLOAD_OFFSET: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioProfileLoadNotice {
    Recovered,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedRadioProfile {
    pub profile: RadioProfile,
    pub follows_default: bool,
    pub notice: Option<RadioProfileLoadNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioProfileStoreError<E> {
    Flash(E),
    InvalidLayout,
    InvalidProfile,
    VerificationFailed,
}

pub struct RadioProfileStore<F> {
    flash: F,
    pages: [u32; 2],
}

impl<F> RadioProfileStore<F>
where
    F: NorFlash,
{
    #[must_use]
    pub const fn new(flash: F, pages: [u32; 2]) -> Self {
        Self { flash, pages }
    }

    pub async fn load(
        &mut self,
        default: RadioProfile,
    ) -> Result<LoadedRadioProfile, RadioProfileStoreError<F::Error>> {
        self.validate_layout()?;
        let slots = self.read_slots().await?;
        let Some(active) = select_active(&slots) else {
            let notice = slots
                .iter()
                .any(|slot| !matches!(slot, Slot::Erased))
                .then_some(RadioProfileLoadNotice::Reset);
            return Ok(LoadedRadioProfile {
                profile: default,
                follows_default: true,
                notice,
            });
        };
        let Some(record) = slots[active].record() else {
            return Ok(LoadedRadioProfile {
                profile: default,
                follows_default: true,
                notice: Some(RadioProfileLoadNotice::Reset),
            });
        };
        let recovered = match slots[1 - active] {
            Slot::Invalid(Some(generation)) => generation_is_newer(generation, record.generation),
            Slot::Invalid(None) => true,
            Slot::Erased | Slot::Valid(_) => false,
        };
        let (profile, follows_default) = match record.value {
            StoredValue::Profile(profile) => (profile, false),
            StoredValue::Default => (default, true),
        };
        Ok(LoadedRadioProfile {
            profile,
            follows_default,
            notice: recovered.then_some(RadioProfileLoadNotice::Recovered),
        })
    }

    pub async fn save(
        &mut self,
        profile: RadioProfile,
        ble_group: Option<&[u8]>,
    ) -> Result<(), RadioProfileStoreError<F::Error>> {
        if profile.validate().is_err() {
            return Err(RadioProfileStoreError::InvalidProfile);
        }
        if let Some(name) = ble_group {
            if !ble_group_name_valid(name) {
                return Err(RadioProfileStoreError::InvalidProfile);
            }
            self.commit_with_ble_group(StoredValue::Profile(profile), Some(name))
                .await
        } else {
            self.commit(StoredValue::Profile(profile)).await
        }
    }

    pub async fn reset(&mut self) -> Result<(), RadioProfileStoreError<F::Error>> {
        self.commit(StoredValue::Default).await
    }

    pub async fn load_ble_discovery_group(&mut self) -> Option<([u8; BLE_GROUP_NAME_MAX], u8)> {
        let slots = self.read_slots().await.ok()?;
        let active = select_active(&slots)?;
        self.read_ble_group(self.pages[active]).await.ok().flatten()
    }

    pub async fn save_ble_discovery_group(
        &mut self,
        name: &[u8],
    ) -> Result<(), RadioProfileStoreError<F::Error>> {
        if !ble_group_name_valid(name) {
            return Err(RadioProfileStoreError::InvalidProfile);
        }
        let slots = self.read_slots().await?;
        let value = select_active(&slots)
            .and_then(|index| slots[index].record())
            .map_or(StoredValue::Default, |record| record.value);
        self.commit_with_ble_group(value, Some(name)).await
    }

    pub fn into_flash(self) -> F {
        self.flash
    }

    async fn commit(&mut self, value: StoredValue) -> Result<(), RadioProfileStoreError<F::Error>> {
        let group = self.load_ble_discovery_group().await;
        let group_ref = group.as_ref().map(|(bytes, len)| &bytes[..*len as usize]);
        self.commit_with_ble_group(value, group_ref).await
    }

    async fn commit_with_ble_group(
        &mut self,
        value: StoredValue,
        ble_group: Option<&[u8]>,
    ) -> Result<(), RadioProfileStoreError<F::Error>> {
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
            .map_err(RadioProfileStoreError::Flash)?;
        self.flash
            .write(page, &record[..COMMIT_OFFSET])
            .await
            .map_err(RadioProfileStoreError::Flash)?;
        self.flash
            .write(
                page + (COMMIT_OFFSET + 4) as u32,
                &record[COMMIT_OFFSET + 4..],
            )
            .await
            .map_err(RadioProfileStoreError::Flash)?;
        self.flash
            .write(page + COMMIT_OFFSET as u32, &COMMIT_WORD.to_le_bytes())
            .await
            .map_err(RadioProfileStoreError::Flash)?;

        let mut verified = [0u8; RECORD_LEN];
        self.flash
            .read(page, &mut verified)
            .await
            .map_err(RadioProfileStoreError::Flash)?;
        let mut expected = record;
        expected[COMMIT_OFFSET..COMMIT_OFFSET + 4].copy_from_slice(&COMMIT_WORD.to_le_bytes());
        if verified != expected {
            return Err(RadioProfileStoreError::VerificationFailed);
        }
        let Some(decoded) = decode_record(&verified) else {
            return Err(RadioProfileStoreError::VerificationFailed);
        };
        if decoded != (StoredRecord { generation, value }) {
            return Err(RadioProfileStoreError::VerificationFailed);
        }
        if let Some(name) = ble_group {
            self.write_ble_group(page, name).await?;
        }
        Ok(())
    }

    async fn read_ble_group(
        &mut self,
        page: u32,
    ) -> Result<Option<([u8; BLE_GROUP_NAME_MAX], u8)>, RadioProfileStoreError<F::Error>> {
        let mut bytes = [0u8; BLE_GROUP_RECORD_LEN];
        self.flash
            .read(page + BLE_GROUP_OFFSET as u32, &mut bytes)
            .await
            .map_err(RadioProfileStoreError::Flash)?;
        Ok(decode_ble_group(&bytes))
    }

    async fn write_ble_group(
        &mut self,
        page: u32,
        name: &[u8],
    ) -> Result<(), RadioProfileStoreError<F::Error>> {
        let record = encode_ble_group(name).ok_or(RadioProfileStoreError::InvalidProfile)?;
        self.flash
            .write(page + BLE_GROUP_OFFSET as u32, &record)
            .await
            .map_err(RadioProfileStoreError::Flash)?;
        let mut verified = [0u8; BLE_GROUP_RECORD_LEN];
        self.flash
            .read(page + BLE_GROUP_OFFSET as u32, &mut verified)
            .await
            .map_err(RadioProfileStoreError::Flash)?;
        if verified != record {
            return Err(RadioProfileStoreError::VerificationFailed);
        }
        Ok(())
    }

    async fn read_slots(&mut self) -> Result<[Slot; 2], RadioProfileStoreError<F::Error>> {
        let mut slots = [Slot::Erased; 2];
        for (index, page) in self.pages.into_iter().enumerate() {
            let mut bytes = [0u8; RECORD_LEN];
            self.flash
                .read(page, &mut bytes)
                .await
                .map_err(RadioProfileStoreError::Flash)?;
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

    fn validate_layout(&self) -> Result<(), RadioProfileStoreError<F::Error>> {
        if F::ERASE_SIZE == 0
            || F::READ_SIZE == 0
            || F::WRITE_SIZE == 0
            || !RECORD_LEN.is_multiple_of(F::READ_SIZE)
            || !COMMIT_OFFSET.is_multiple_of(F::WRITE_SIZE)
            || !4usize.is_multiple_of(F::WRITE_SIZE)
            || !(COMMIT_OFFSET + 4).is_multiple_of(F::WRITE_SIZE)
            || !(RECORD_LEN - COMMIT_OFFSET - 4).is_multiple_of(F::WRITE_SIZE)
            || !BLE_GROUP_OFFSET.is_multiple_of(F::WRITE_SIZE)
            || !BLE_GROUP_RECORD_LEN.is_multiple_of(F::WRITE_SIZE)
            || BLE_GROUP_OFFSET + BLE_GROUP_RECORD_LEN > F::ERASE_SIZE
            || self.pages[0] == self.pages[1]
        {
            return Err(RadioProfileStoreError::InvalidLayout);
        }
        for page in self.pages {
            let end = page as usize + F::ERASE_SIZE;
            if !(page as usize).is_multiple_of(F::ERASE_SIZE) || end > self.flash.capacity() {
                return Err(RadioProfileStoreError::InvalidLayout);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoredValue {
    Profile(RadioProfile),
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

fn encode_record(generation: u64, value: StoredValue) -> [u8; RECORD_LEN] {
    let mut bytes = [0xFF; RECORD_LEN];
    bytes[..4].copy_from_slice(&MAGIC);
    bytes[4..6].copy_from_slice(&SCHEMA_VERSION.to_le_bytes());
    bytes[6] = match value {
        StoredValue::Profile(_) => PROFILE_KIND,
        StoredValue::Default => DEFAULT_KIND,
    };
    bytes[7] = 0;
    bytes[8..16].copy_from_slice(&generation.to_le_bytes());
    let payload_len = match value {
        StoredValue::Profile(profile) => {
            encode_profile(
                profile,
                &mut bytes[PAYLOAD_OFFSET..PAYLOAD_OFFSET + PROFILE_PAYLOAD_LEN],
            );
            PROFILE_PAYLOAD_LEN
        }
        StoredValue::Default => 0,
    };
    bytes[16..18].copy_from_slice(&(payload_len as u16).to_le_bytes());
    bytes[18..20].fill(0);
    bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].fill(0);
    bytes[24..28].fill(0);
    bytes[COMMIT_OFFSET..COMMIT_OFFSET + 4].fill(0);
    let checksum = crc32(&bytes);
    bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&checksum.to_le_bytes());
    bytes[COMMIT_OFFSET..COMMIT_OFFSET + 4].fill(0xFF);
    bytes
}

fn decode_record(bytes: &[u8; RECORD_LEN]) -> Option<StoredRecord> {
    if bytes[..4] != MAGIC
        || u16::from_le_bytes(bytes[4..6].try_into().ok()?) != SCHEMA_VERSION
        || bytes[7] != 0
        || bytes[18..20] != [0, 0]
        || bytes[24..28] != [0, 0, 0, 0]
        || u32::from_le_bytes(bytes[COMMIT_OFFSET..COMMIT_OFFSET + 4].try_into().ok()?)
            != COMMIT_WORD
    {
        return None;
    }
    let found = u32::from_le_bytes(
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4]
            .try_into()
            .ok()?,
    );
    let mut canonical = *bytes;
    canonical[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].fill(0);
    canonical[COMMIT_OFFSET..COMMIT_OFFSET + 4].fill(0);
    if crc32(&canonical) != found {
        return None;
    }
    let generation = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let payload_len = u16::from_le_bytes(bytes[16..18].try_into().ok()?) as usize;
    let value = match (bytes[6], payload_len) {
        (PROFILE_KIND, PROFILE_PAYLOAD_LEN) => StoredValue::Profile(decode_profile(
            &bytes[PAYLOAD_OFFSET..PAYLOAD_OFFSET + PROFILE_PAYLOAD_LEN],
        )?),
        (DEFAULT_KIND, 0) => StoredValue::Default,
        _ => return None,
    };
    Some(StoredRecord { generation, value })
}

fn generation_hint(bytes: &[u8; RECORD_LEN]) -> Option<u64> {
    (bytes[..4] == MAGIC).then(|| {
        u64::from_le_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ])
    })
}

fn ble_group_name_valid(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= BLE_GROUP_NAME_MAX
        && name.iter().all(|byte| BLE_GROUP_CHARSET.contains(byte))
}

fn encode_ble_group(name: &[u8]) -> Option<[u8; BLE_GROUP_RECORD_LEN]> {
    if !ble_group_name_valid(name) {
        return None;
    }
    let mut bytes = [0xFF; BLE_GROUP_RECORD_LEN];
    bytes[..4].copy_from_slice(&BLE_GROUP_MAGIC);
    bytes[4..6].copy_from_slice(&BLE_GROUP_VERSION.to_le_bytes());
    bytes[6] = name.len() as u8;
    bytes[7] = 0;
    bytes[8..8 + name.len()].copy_from_slice(name);
    bytes[24..28].fill(0);
    let checksum = crc32(&bytes[..24]);
    bytes[24..28].copy_from_slice(&checksum.to_le_bytes());
    bytes[28..32].copy_from_slice(&COMMIT_WORD.to_le_bytes());
    Some(bytes)
}

fn decode_ble_group(bytes: &[u8; BLE_GROUP_RECORD_LEN]) -> Option<([u8; BLE_GROUP_NAME_MAX], u8)> {
    if bytes.iter().all(|byte| *byte == 0xFF) {
        return None;
    }
    if bytes[..4] != BLE_GROUP_MAGIC
        || u16::from_le_bytes(bytes[4..6].try_into().ok()?) != BLE_GROUP_VERSION
        || bytes[7] != 0
        || u32::from_le_bytes(bytes[28..32].try_into().ok()?) != COMMIT_WORD
    {
        return None;
    }
    let len = bytes[6] as usize;
    if len == 0 || len > BLE_GROUP_NAME_MAX {
        return None;
    }
    let mut checksum_input = *bytes;
    checksum_input[24..28].fill(0);
    let found = u32::from_le_bytes(bytes[24..28].try_into().ok()?);
    if crc32(&checksum_input[..24]) != found {
        return None;
    }
    let name = &bytes[8..8 + len];
    if !ble_group_name_valid(name) {
        return None;
    }
    let mut stored = [0u8; BLE_GROUP_NAME_MAX];
    stored[..len].copy_from_slice(name);
    Some((stored, len as u8))
}

fn encode_profile(profile: RadioProfile, out: &mut [u8]) {
    out[..4].copy_from_slice(&profile.frequency.hz().to_le_bytes());
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation;
    out[4] = match spreading_factor {
        SpreadingFactor::Sf5 => 5,
        SpreadingFactor::Sf6 => 6,
        SpreadingFactor::Sf7 => 7,
        SpreadingFactor::Sf8 => 8,
        SpreadingFactor::Sf9 => 9,
        SpreadingFactor::Sf10 => 10,
        SpreadingFactor::Sf11 => 11,
        SpreadingFactor::Sf12 => 12,
    };
    out[5] = match bandwidth {
        LoraBandwidth::Bw125kHz => 1,
        LoraBandwidth::Bw250kHz => 2,
        LoraBandwidth::Bw500kHz => 3,
    };
    out[6] = match coding_rate {
        CodingRate::Cr45 => 5,
        CodingRate::Cr46 => 6,
        CodingRate::Cr47 => 7,
        CodingRate::Cr48 => 8,
    };
    out[7] = profile.tx_power.dbm().to_le_bytes()[0];
    out[8..10].copy_from_slice(&profile.preamble.count().to_le_bytes());
    out[10] = region_code(profile.region);
    out[11] = 0;
}

fn decode_profile(bytes: &[u8]) -> Option<RadioProfile> {
    if bytes[11] != 0 {
        return None;
    }
    let spreading_factor = match bytes[4] {
        5 => SpreadingFactor::Sf5,
        6 => SpreadingFactor::Sf6,
        7 => SpreadingFactor::Sf7,
        8 => SpreadingFactor::Sf8,
        9 => SpreadingFactor::Sf9,
        10 => SpreadingFactor::Sf10,
        11 => SpreadingFactor::Sf11,
        12 => SpreadingFactor::Sf12,
        _ => return None,
    };
    let bandwidth = match bytes[5] {
        1 => LoraBandwidth::Bw125kHz,
        2 => LoraBandwidth::Bw250kHz,
        3 => LoraBandwidth::Bw500kHz,
        _ => return None,
    };
    let coding_rate = match bytes[6] {
        5 => CodingRate::Cr45,
        6 => CodingRate::Cr46,
        7 => CodingRate::Cr47,
        8 => CodingRate::Cr48,
        _ => return None,
    };
    let profile = RadioProfile {
        frequency: Frequency::new(u32::from_le_bytes(bytes[..4].try_into().ok()?)),
        modulation: Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        },
        tx_power: TxPower::new(i8::from_le_bytes([bytes[7]])),
        preamble: PreambleSymbols::new(u16::from_le_bytes(bytes[8..10].try_into().ok()?)),
        region: decode_region(bytes[10])?,
    };
    profile.validate().ok().map(|()| profile)
}

const fn region_code(region: Region) -> u8 {
    match region {
        Region::Us915 => 1,
        Region::Au915 => 2,
        Region::Eu433 => 3,
        Region::Eu865 => 4,
        Region::Eu868 => 5,
        Region::Eu869 => 6,
        Region::As923 => 7,
        Region::In865 => 8,
        Region::Cn470 => 9,
        Region::Kr920 => 10,
        Region::Jp920 => 11,
        Region::Unlimited => 12,
    }
}

const fn decode_region(value: u8) -> Option<Region> {
    match value {
        1 => Some(Region::Us915),
        2 => Some(Region::Au915),
        3 => Some(Region::Eu433),
        4 => Some(Region::Eu865),
        5 => Some(Region::Eu868),
        6 => Some(Region::Eu869),
        7 => Some(Region::As923),
        8 => Some(Region::In865),
        9 => Some(Region::Cn470),
        10 => Some(Region::Kr920),
        11 => Some(Region::Jp920),
        12 => Some(Region::Unlimited),
        _ => None,
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let low_bit_set = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & low_bit_set);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::Future;
    use core::task::{Context, Poll};
    use embedded_storage_async::nor_flash::{
        ErrorType, NorFlashError, NorFlashErrorKind, ReadNorFlash,
    };
    use personal_rns::interfaces::lora::DEFAULT_915_PROFILE;
    use std::boxed::Box;
    use std::task::Waker;

    const CAPACITY: usize = 2 * 4096;
    const PAGES: [u32; 2] = [0, 4096];

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FakeError {
        Bounds,
        Alignment,
        PowerCut,
    }

    impl NorFlashError for FakeError {
        fn kind(&self) -> NorFlashErrorKind {
            match self {
                Self::Bounds => NorFlashErrorKind::OutOfBounds,
                Self::Alignment => NorFlashErrorKind::NotAligned,
                Self::PowerCut => NorFlashErrorKind::Other,
            }
        }
    }

    #[derive(Clone)]
    struct FakeFlash {
        bytes: [u8; CAPACITY],
        fail_mutation: Option<usize>,
        mutations: usize,
        corrupt_after_commit: bool,
    }

    impl FakeFlash {
        fn erased() -> Self {
            Self {
                bytes: [0xFF; CAPACITY],
                fail_mutation: None,
                mutations: 0,
                corrupt_after_commit: false,
            }
        }

        fn from_bytes(bytes: [u8; CAPACITY]) -> Self {
            Self {
                bytes,
                ..Self::erased()
            }
        }

        fn mutate(&mut self) -> Result<(), FakeError> {
            if self.fail_mutation == Some(self.mutations) {
                return Err(FakeError::PowerCut);
            }
            self.mutations += 1;
            Ok(())
        }

        fn range(
            &self,
            offset: u32,
            len: usize,
            alignment: usize,
        ) -> Result<core::ops::Range<usize>, FakeError> {
            if !(offset as usize).is_multiple_of(alignment) || !len.is_multiple_of(alignment) {
                return Err(FakeError::Alignment);
            }
            let start = offset as usize;
            let end = start.checked_add(len).ok_or(FakeError::Bounds)?;
            if end > self.bytes.len() {
                return Err(FakeError::Bounds);
            }
            Ok(start..end)
        }
    }

    impl ErrorType for FakeFlash {
        type Error = FakeError;
    }

    impl ReadNorFlash for FakeFlash {
        const READ_SIZE: usize = 4;

        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            let range = self.range(offset, bytes.len(), Self::READ_SIZE)?;
            bytes.copy_from_slice(&self.bytes[range]);
            Ok(())
        }

        fn capacity(&self) -> usize {
            self.bytes.len()
        }
    }

    impl NorFlash for FakeFlash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = 4096;

        async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            self.mutate()?;
            let range = self.range(from, (to - from) as usize, Self::ERASE_SIZE)?;
            self.bytes[range].fill(0xFF);
            Ok(())
        }

        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            self.mutate()?;
            let range = self.range(offset, bytes.len(), Self::WRITE_SIZE)?;
            for (destination, source) in self.bytes[range.clone()].iter_mut().zip(bytes) {
                *destination &= *source;
            }
            if self.corrupt_after_commit && offset as usize % Self::ERASE_SIZE == COMMIT_OFFSET {
                let page = offset as usize - COMMIT_OFFSET;
                self.bytes[page + CHECKSUM_OFFSET] ^= 1;
            }
            Ok(())
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn changed_profile() -> RadioProfile {
        RadioProfile {
            frequency: Frequency::new(916_000_000),
            tx_power: TxPower::new(20),
            ..DEFAULT_915_PROFILE
        }
    }

    fn committed_record(generation: u64, value: StoredValue) -> [u8; RECORD_LEN] {
        let mut bytes = encode_record(generation, value);
        bytes[COMMIT_OFFSET..COMMIT_OFFSET + 4].copy_from_slice(&COMMIT_WORD.to_le_bytes());
        bytes
    }

    fn reseal(bytes: &mut [u8; RECORD_LEN]) {
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].fill(0);
        bytes[COMMIT_OFFSET..COMMIT_OFFSET + 4].fill(0);
        let checksum = crc32(bytes);
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&checksum.to_le_bytes());
        bytes[COMMIT_OFFSET..COMMIT_OFFSET + 4].copy_from_slice(&COMMIT_WORD.to_le_bytes());
    }

    #[test]
    fn explicit_profile_codec_covers_every_supported_field_value() {
        let spreading_factors = [
            SpreadingFactor::Sf5,
            SpreadingFactor::Sf6,
            SpreadingFactor::Sf7,
            SpreadingFactor::Sf8,
            SpreadingFactor::Sf9,
            SpreadingFactor::Sf10,
            SpreadingFactor::Sf11,
            SpreadingFactor::Sf12,
        ];
        let bandwidths = [
            LoraBandwidth::Bw125kHz,
            LoraBandwidth::Bw250kHz,
            LoraBandwidth::Bw500kHz,
        ];
        let coding_rates = [
            CodingRate::Cr45,
            CodingRate::Cr46,
            CodingRate::Cr47,
            CodingRate::Cr48,
        ];
        for region in Region::ALL {
            for spreading_factor in spreading_factors {
                for bandwidth in bandwidths {
                    for coding_rate in coding_rates {
                        let profile = RadioProfile {
                            frequency: region.default_frequency(),
                            modulation: Modulation::Lora {
                                spreading_factor,
                                bandwidth,
                                coding_rate,
                            },
                            tx_power: TxPower::new(region.max_tx_power().dbm().min(7)),
                            preamble: PreambleSymbols::new(37),
                            region,
                        };
                        let record = committed_record(42, StoredValue::Profile(profile));
                        assert_eq!(
                            decode_record(&record),
                            Some(StoredRecord {
                                generation: 42,
                                value: StoredValue::Profile(profile),
                            })
                        );
                    }
                }
            }
        }

        for tx_power in [-9, -1, 0, 1, 22] {
            let profile = RadioProfile {
                tx_power: TxPower::new(tx_power),
                ..DEFAULT_915_PROFILE
            };
            let record = committed_record(43, StoredValue::Profile(profile));
            assert_eq!(
                decode_record(&record),
                Some(StoredRecord {
                    generation: 43,
                    value: StoredValue::Profile(profile),
                })
            );
        }
    }

    #[test]
    fn codec_refuses_unknown_schema_codes_corruption_and_invalid_profiles() {
        let valid = committed_record(1, StoredValue::Profile(DEFAULT_915_PROFILE));
        let mut unknown_schema = valid;
        unknown_schema[4..6].copy_from_slice(&2u16.to_le_bytes());
        reseal(&mut unknown_schema);
        assert_eq!(decode_record(&unknown_schema), None);

        for offset in [
            6,
            PAYLOAD_OFFSET + 4,
            PAYLOAD_OFFSET + 5,
            PAYLOAD_OFFSET + 6,
            PAYLOAD_OFFSET + 10,
        ] {
            let mut unknown_code = valid;
            unknown_code[offset] = 0xFE;
            reseal(&mut unknown_code);
            assert_eq!(decode_record(&unknown_code), None);
        }

        let mut corrupted = valid;
        corrupted[PAYLOAD_OFFSET] ^= 1;
        assert_eq!(decode_record(&corrupted), None);

        let mut outside_region = DEFAULT_915_PROFILE;
        outside_region.frequency = Frequency::new(800_000_000);
        let invalid = committed_record(2, StoredValue::Profile(outside_region));
        assert_eq!(decode_record(&invalid), None);
    }

    #[test]
    fn fresh_save_reset_and_restore_have_distinct_semantics() {
        let mut store = RadioProfileStore::new(FakeFlash::erased(), PAGES);
        let fresh = block_on(store.load(DEFAULT_915_PROFILE)).unwrap();
        assert_eq!(
            fresh,
            LoadedRadioProfile {
                profile: DEFAULT_915_PROFILE,
                follows_default: true,
                notice: None,
            }
        );

        let profile = changed_profile();
        block_on(store.save(profile, None)).unwrap();
        assert_eq!(
            block_on(store.load(DEFAULT_915_PROFILE)).unwrap(),
            LoadedRadioProfile {
                profile,
                follows_default: false,
                notice: None,
            }
        );

        block_on(store.reset()).unwrap();
        let future_default = RadioProfile {
            frequency: Frequency::new(917_000_000),
            ..DEFAULT_915_PROFILE
        };
        assert_eq!(
            block_on(store.load(future_default)).unwrap(),
            LoadedRadioProfile {
                profile: future_default,
                follows_default: true,
                notice: None,
            }
        );
    }

    #[test]
    fn ble_discovery_group_survives_lora_save_and_reboot() {
        let mut store = RadioProfileStore::new(FakeFlash::erased(), PAGES);
        block_on(store.save_ble_discovery_group(b"mt-leg-a")).unwrap();
        let (bytes, len) = block_on(store.load_ble_discovery_group()).unwrap();
        assert_eq!(&bytes[..len as usize], b"mt-leg-a");

        block_on(store.save(changed_profile(), None)).unwrap();
        let (bytes, len) = block_on(store.load_ble_discovery_group()).unwrap();
        assert_eq!(&bytes[..len as usize], b"mt-leg-a");

        let flash = store.into_flash();
        let mut rebooted = RadioProfileStore::new(FakeFlash::from_bytes(flash.bytes), PAGES);
        let (bytes, len) = block_on(rebooted.load_ble_discovery_group()).unwrap();
        assert_eq!(&bytes[..len as usize], b"mt-leg-a");
        assert_eq!(
            block_on(rebooted.load(DEFAULT_915_PROFILE))
                .unwrap()
                .profile,
            changed_profile()
        );
    }

    #[test]
    fn saving_a_profile_can_replace_the_ble_discovery_group() {
        let mut store = RadioProfileStore::new(FakeFlash::erased(), PAGES);
        block_on(store.save_ble_discovery_group(b"mt-leg-a")).unwrap();
        block_on(store.save(changed_profile(), Some(b"mt-leg-b"))).unwrap();
        let (bytes, len) = block_on(store.load_ble_discovery_group()).unwrap();
        assert_eq!(&bytes[..len as usize], b"mt-leg-b");
        assert_eq!(
            block_on(store.load(DEFAULT_915_PROFILE)).unwrap().profile,
            changed_profile()
        );
    }

    #[test]
    fn every_interrupted_mutation_preserves_the_previous_committed_profile() {
        let mut initial = RadioProfileStore::new(FakeFlash::erased(), PAGES);
        block_on(initial.save(DEFAULT_915_PROFILE, None)).unwrap();
        let bytes = initial.into_flash().bytes;

        for fail_mutation in 0..4 {
            let mut flash = FakeFlash::from_bytes(bytes);
            flash.fail_mutation = Some(fail_mutation);
            let mut interrupted = RadioProfileStore::new(flash, PAGES);
            assert!(block_on(interrupted.save(changed_profile(), None)).is_err());
            let flash = interrupted.into_flash();
            let mut rebooted = RadioProfileStore::new(FakeFlash::from_bytes(flash.bytes), PAGES);
            assert_eq!(
                block_on(rebooted.load(DEFAULT_915_PROFILE))
                    .unwrap()
                    .profile,
                DEFAULT_915_PROFILE
            );
        }
    }

    #[test]
    fn every_interrupted_mutation_preserves_the_previous_reset_marker() {
        let future_default = RadioProfile {
            frequency: Frequency::new(917_000_000),
            ..DEFAULT_915_PROFILE
        };
        let mut initial = RadioProfileStore::new(FakeFlash::erased(), PAGES);
        block_on(initial.reset()).unwrap();
        let bytes = initial.into_flash().bytes;

        for fail_mutation in 0..4 {
            let mut flash = FakeFlash::from_bytes(bytes);
            flash.fail_mutation = Some(fail_mutation);
            let mut interrupted = RadioProfileStore::new(flash, PAGES);
            assert!(block_on(interrupted.save(changed_profile(), None)).is_err());
            let flash = interrupted.into_flash();
            let mut rebooted = RadioProfileStore::new(FakeFlash::from_bytes(flash.bytes), PAGES);
            let loaded = block_on(rebooted.load(future_default)).unwrap();
            assert_eq!(loaded.profile, future_default);
            assert!(loaded.follows_default);
            assert!(
                loaded.notice.is_none() || loaded.notice == Some(RadioProfileLoadNotice::Recovered)
            );
        }
    }

    #[test]
    fn failed_readback_verification_preserves_the_previous_profile() {
        let mut initial = RadioProfileStore::new(FakeFlash::erased(), PAGES);
        block_on(initial.save(DEFAULT_915_PROFILE, None)).unwrap();
        let mut flash = initial.into_flash();
        flash.corrupt_after_commit = true;
        let mut store = RadioProfileStore::new(flash, PAGES);
        assert_eq!(
            block_on(store.save(changed_profile(), None)),
            Err(RadioProfileStoreError::VerificationFailed)
        );
        let mut rebooted = RadioProfileStore::new(store.into_flash(), PAGES);
        let loaded = block_on(rebooted.load(DEFAULT_915_PROFILE)).unwrap();
        assert_eq!(loaded.profile, DEFAULT_915_PROFILE);
        assert_eq!(loaded.notice, Some(RadioProfileLoadNotice::Recovered));
    }

    #[test]
    fn corrupt_newest_falls_back_and_two_corrupt_slots_reset() {
        let mut store = RadioProfileStore::new(FakeFlash::erased(), PAGES);
        block_on(store.save(DEFAULT_915_PROFILE, None)).unwrap();
        block_on(store.save(changed_profile(), None)).unwrap();
        let mut flash = store.into_flash();
        flash.bytes[4096 + CHECKSUM_OFFSET] ^= 1;
        let mut recovered = RadioProfileStore::new(flash, PAGES);
        let loaded = block_on(recovered.load(DEFAULT_915_PROFILE)).unwrap();
        assert_eq!(loaded.profile, DEFAULT_915_PROFILE);
        assert_eq!(loaded.notice, Some(RadioProfileLoadNotice::Recovered));

        let mut flash = recovered.into_flash();
        flash.bytes[CHECKSUM_OFFSET] ^= 1;
        let mut reset = RadioProfileStore::new(flash, PAGES);
        let loaded = block_on(reset.load(DEFAULT_915_PROFILE)).unwrap();
        assert_eq!(loaded.profile, DEFAULT_915_PROFILE);
        assert_eq!(loaded.notice, Some(RadioProfileLoadNotice::Reset));
    }

    #[test]
    fn unreadable_newest_generation_still_reports_recovery() {
        let mut store = RadioProfileStore::new(FakeFlash::erased(), PAGES);
        block_on(store.save(DEFAULT_915_PROFILE, None)).unwrap();
        block_on(store.save(changed_profile(), None)).unwrap();
        let mut flash = store.into_flash();
        flash.bytes[4096] ^= 1;

        let mut rebooted = RadioProfileStore::new(flash, PAGES);
        let loaded = block_on(rebooted.load(DEFAULT_915_PROFILE)).unwrap();
        assert_eq!(loaded.profile, DEFAULT_915_PROFILE);
        assert_eq!(loaded.notice, Some(RadioProfileLoadNotice::Recovered));
    }

    #[test]
    fn generation_selection_handles_wraparound() {
        let older = Slot::Valid(StoredRecord {
            generation: u64::MAX,
            value: StoredValue::Default,
        });
        let wrapped = Slot::Valid(StoredRecord {
            generation: 0,
            value: StoredValue::Profile(DEFAULT_915_PROFILE),
        });
        assert_eq!(select_active(&[older, wrapped]), Some(1));
    }
}
