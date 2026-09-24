use embedded_storage_async::nor_flash::NorFlash;
use personal_rns::interfaces::lora::{
    CodingRate, Frequency, LoraBandwidth, Modulation, PreambleSymbols, RadioProfile,
    SpreadingFactor, TxPower,
};
use personal_rns::interfaces::lora::{LoRaConfiguration, LoRaConfigurationState};
use personal_rns::interfaces::subghz::regions::us915::Us915;
use personal_rns::interfaces::subghz::{
    RegulatoryRegion, ResolvedSubGMode, SubGConfiguration, SubGConfigurationState, SubGMode,
    SubGRegion,
};

const MAGIC: [u8; 4] = *b"HSLP";
const LEGACY_SCHEMA_VERSION: u16 = 1;
const SCHEMA_VERSION: u16 = 2;
const LEGACY_PROFILE_KIND: u8 = 1;
const LEGACY_DEFAULT_KIND: u8 = 2;
const CONFIGURATION_KIND: u8 = 3;
const UNCONFIGURED_KIND: u8 = 4;
const GHZ24_CONFIGURATION_KIND: u8 = 5;
const MODE_AUTO_LORA: u8 = 1;
const MODE_MANUAL_LORA: u8 = 2;
const MODE_TURBO: u8 = 3;
const COMMIT_WORD: u32 = 0x5449_4D43;
const RECORD_LEN: usize = 48;
const PROFILE_PAYLOAD_LEN: usize = 12;
const MANUAL_CONFIGURATION_PAYLOAD_LEN: usize = PROFILE_PAYLOAD_LEN + 1;
const MODE_CONFIGURATION_PAYLOAD_LEN: usize = 2;
const CHECKSUM_OFFSET: usize = 20;
const COMMIT_OFFSET: usize = 28;
const PAYLOAD_OFFSET: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubGConfigurationLoadNotice {
    Migrated,
    Recovered,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedSubGConfiguration {
    pub state: SubGConfigurationState,
    pub notice: Option<SubGConfigurationLoadNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedLoRaConfiguration {
    pub state: LoRaConfigurationState,
    pub notice: Option<SubGConfigurationLoadNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubGConfigurationStoreError<E> {
    UnsupportedConfiguration,
    Flash {
        operation: SubGConfigurationFlashOperation,
        error: E,
    },
    InvalidLayout,
    VerificationFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubGConfigurationFlashOperation {
    Read,
    Erase,
    WriteRecord,
    WriteCommit,
    Verify,
    Reconcile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubGConfigurationCommitOutcome<E> {
    Committed,
    NotCommitted(SubGConfigurationStoreError<E>),
    Indeterminate(SubGConfigurationStoreError<E>),
}

pub struct SubGConfigurationStore<F> {
    flash: F,
    pages: [u32; 2],
}

impl<F> SubGConfigurationStore<F>
where
    F: NorFlash,
{
    #[must_use]
    pub const fn new(flash: F, pages: [u32; 2]) -> Self {
        Self { flash, pages }
    }

    pub async fn load(
        &mut self,
    ) -> Result<LoadedSubGConfiguration, SubGConfigurationStoreError<F::Error>> {
        let loaded = self.load_radio().await?;
        let state = match loaded.state {
            LoRaConfigurationState::Unconfigured => SubGConfigurationState::Unconfigured,
            LoRaConfigurationState::Configured(LoRaConfiguration::SubG(configuration)) => {
                SubGConfigurationState::Configured(configuration)
            }
            #[cfg(feature = "lora-2g4")]
            LoRaConfigurationState::Configured(LoRaConfiguration::Ghz24(_)) => {
                return Err(SubGConfigurationStoreError::UnsupportedConfiguration)
            }
        };
        Ok(LoadedSubGConfiguration {
            state,
            notice: loaded.notice,
        })
    }

    pub async fn save_radio(
        &mut self,
        state: LoRaConfigurationState,
    ) -> SubGConfigurationCommitOutcome<F::Error> {
        match state {
            LoRaConfigurationState::Unconfigured => self.clear().await,
            LoRaConfigurationState::Configured(LoRaConfiguration::SubG(configuration)) => {
                self.save(configuration).await
            }
            #[cfg(feature = "lora-2g4")]
            LoRaConfigurationState::Configured(LoRaConfiguration::Ghz24(profile)) => {
                self.commit(StoredValue::Ghz24(profile)).await
            }
        }
    }

    pub async fn load_radio(
        &mut self,
    ) -> Result<LoadedLoRaConfiguration, SubGConfigurationStoreError<F::Error>> {
        self.validate_layout()?;
        let slots = self
            .read_slots(SubGConfigurationFlashOperation::Read)
            .await?;
        let Some(active) = select_active(&slots) else {
            let notice = slots
                .iter()
                .any(|slot| !matches!(slot, Slot::Erased))
                .then_some(SubGConfigurationLoadNotice::Reset);
            return Ok(LoadedLoRaConfiguration {
                state: LoRaConfigurationState::Unconfigured,
                notice,
            });
        };
        let Some(record) = slots[active].record() else {
            return Ok(LoadedLoRaConfiguration {
                state: LoRaConfigurationState::Unconfigured,
                notice: Some(SubGConfigurationLoadNotice::Reset),
            });
        };
        let recovered = match slots[1 - active] {
            Slot::Invalid(Some(generation)) => generation_is_newer(generation, record.generation),
            Slot::Invalid(None) => true,
            Slot::Erased | Slot::Valid(_) => false,
        };
        let notice = if recovered {
            Some(SubGConfigurationLoadNotice::Recovered)
        } else if record.legacy {
            Some(SubGConfigurationLoadNotice::Migrated)
        } else {
            None
        };
        Ok(LoadedLoRaConfiguration {
            state: record
                .value
                .state()
                .ok_or(SubGConfigurationStoreError::UnsupportedConfiguration)?,
            notice,
        })
    }

    pub async fn save(
        &mut self,
        configuration: SubGConfiguration,
    ) -> SubGConfigurationCommitOutcome<F::Error> {
        self.commit(StoredValue::Configuration(configuration)).await
    }

    pub async fn clear(&mut self) -> SubGConfigurationCommitOutcome<F::Error> {
        self.commit(StoredValue::Unconfigured).await
    }

    pub fn into_flash(self) -> F {
        self.flash
    }

    async fn commit(&mut self, value: StoredValue) -> SubGConfigurationCommitOutcome<F::Error> {
        if let Err(error) = self.validate_layout() {
            return SubGConfigurationCommitOutcome::NotCommitted(error);
        }
        let slots = match self.read_slots(SubGConfigurationFlashOperation::Read).await {
            Ok(slots) => slots,
            Err(error) => return SubGConfigurationCommitOutcome::NotCommitted(error),
        };
        let active = select_active(&slots);
        let previous = active.and_then(|index| slots[index].record());
        let target = active.map_or(0, |index| 1 - index);
        let generation = previous.map_or(0, |record| record.generation.wrapping_add(1));
        let record = encode_record(generation, value);
        let expected_record = StoredRecord {
            generation,
            value,
            legacy: false,
        };
        let page = self.pages[target];
        if let Err(error) = self.flash.erase(page, page + F::ERASE_SIZE as u32).await {
            return SubGConfigurationCommitOutcome::NotCommitted(
                SubGConfigurationStoreError::Flash {
                    operation: SubGConfigurationFlashOperation::Erase,
                    error,
                },
            );
        }
        if let Err(error) = self.flash.write(page, &record[..COMMIT_OFFSET]).await {
            return SubGConfigurationCommitOutcome::NotCommitted(
                SubGConfigurationStoreError::Flash {
                    operation: SubGConfigurationFlashOperation::WriteRecord,
                    error,
                },
            );
        }
        if let Err(error) = self
            .flash
            .write(
                page + (COMMIT_OFFSET + 4) as u32,
                &record[COMMIT_OFFSET + 4..],
            )
            .await
        {
            return SubGConfigurationCommitOutcome::NotCommitted(
                SubGConfigurationStoreError::Flash {
                    operation: SubGConfigurationFlashOperation::WriteRecord,
                    error,
                },
            );
        }
        if let Err(error) = self
            .flash
            .write(page + COMMIT_OFFSET as u32, &COMMIT_WORD.to_le_bytes())
            .await
        {
            return self
                .reconcile_commit(
                    expected_record,
                    previous,
                    SubGConfigurationStoreError::Flash {
                        operation: SubGConfigurationFlashOperation::WriteCommit,
                        error,
                    },
                )
                .await;
        }

        let mut verified = [0u8; RECORD_LEN];
        if let Err(error) = self.flash.read(page, &mut verified).await {
            return self
                .reconcile_commit(
                    expected_record,
                    previous,
                    SubGConfigurationStoreError::Flash {
                        operation: SubGConfigurationFlashOperation::Verify,
                        error,
                    },
                )
                .await;
        }
        let mut expected = record;
        expected[COMMIT_OFFSET..COMMIT_OFFSET + 4].copy_from_slice(&COMMIT_WORD.to_le_bytes());
        if verified != expected {
            return self
                .reconcile_commit(
                    expected_record,
                    previous,
                    SubGConfigurationStoreError::VerificationFailed,
                )
                .await;
        }
        let Some(decoded) = decode_record(&verified) else {
            return self
                .reconcile_commit(
                    expected_record,
                    previous,
                    SubGConfigurationStoreError::VerificationFailed,
                )
                .await;
        };
        if decoded != expected_record {
            return self
                .reconcile_commit(
                    expected_record,
                    previous,
                    SubGConfigurationStoreError::VerificationFailed,
                )
                .await;
        }
        SubGConfigurationCommitOutcome::Committed
    }

    async fn reconcile_commit(
        &mut self,
        expected: StoredRecord,
        previous: Option<StoredRecord>,
        failure: SubGConfigurationStoreError<F::Error>,
    ) -> SubGConfigurationCommitOutcome<F::Error> {
        let slots = match self
            .read_slots(SubGConfigurationFlashOperation::Reconcile)
            .await
        {
            Ok(slots) => slots,
            Err(error) => return SubGConfigurationCommitOutcome::Indeterminate(error),
        };
        let active = select_active(&slots).and_then(|index| slots[index].record());
        if active == Some(expected) {
            return SubGConfigurationCommitOutcome::Committed;
        }
        if active == previous {
            return SubGConfigurationCommitOutcome::NotCommitted(failure);
        }
        SubGConfigurationCommitOutcome::Indeterminate(failure)
    }

    async fn read_slots(
        &mut self,
        operation: SubGConfigurationFlashOperation,
    ) -> Result<[Slot; 2], SubGConfigurationStoreError<F::Error>> {
        let mut slots = [Slot::Erased; 2];
        for (index, page) in self.pages.into_iter().enumerate() {
            let mut bytes = [0u8; RECORD_LEN];
            self.flash
                .read(page, &mut bytes)
                .await
                .map_err(|error| SubGConfigurationStoreError::Flash { operation, error })?;
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

    fn validate_layout(&self) -> Result<(), SubGConfigurationStoreError<F::Error>> {
        if F::ERASE_SIZE == 0
            || F::READ_SIZE == 0
            || F::WRITE_SIZE == 0
            || F::ERASE_SIZE > u32::MAX as usize
            || RECORD_LEN > F::ERASE_SIZE
            || !RECORD_LEN.is_multiple_of(F::READ_SIZE)
            || !COMMIT_OFFSET.is_multiple_of(F::WRITE_SIZE)
            || !4usize.is_multiple_of(F::WRITE_SIZE)
            || !(COMMIT_OFFSET + 4).is_multiple_of(F::WRITE_SIZE)
            || !(RECORD_LEN - COMMIT_OFFSET - 4).is_multiple_of(F::WRITE_SIZE)
            || self.pages[0] == self.pages[1]
        {
            return Err(SubGConfigurationStoreError::InvalidLayout);
        }
        let [first_page, second_page] = self.pages;
        let Some(first_end) = (first_page as usize).checked_add(F::ERASE_SIZE) else {
            return Err(SubGConfigurationStoreError::InvalidLayout);
        };
        let Some(second_end) = (second_page as usize).checked_add(F::ERASE_SIZE) else {
            return Err(SubGConfigurationStoreError::InvalidLayout);
        };
        for (page, end) in [(first_page, first_end), (second_page, second_end)] {
            if !(page as usize).is_multiple_of(F::ERASE_SIZE)
                || !(page as usize).is_multiple_of(F::READ_SIZE)
                || !(page as usize).is_multiple_of(F::WRITE_SIZE)
                || page.checked_add(F::ERASE_SIZE as u32).is_none()
                || end > self.flash.capacity()
            {
                return Err(SubGConfigurationStoreError::InvalidLayout);
            }
        }
        let first_start = first_page as usize;
        let second_start = second_page as usize;
        if first_start < second_end && second_start < first_end {
            return Err(SubGConfigurationStoreError::InvalidLayout);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoredValue {
    Configuration(SubGConfiguration),
    #[cfg(feature = "lora-2g4")]
    Ghz24(personal_rns::interfaces::lora::Ghz24Profile),
    #[cfg(not(feature = "lora-2g4"))]
    UnsupportedGhz24([u8; PROFILE_PAYLOAD_LEN]),
    Unconfigured,
}

impl StoredValue {
    const fn state(self) -> Option<LoRaConfigurationState> {
        match self {
            Self::Configuration(configuration) => Some(LoRaConfigurationState::Configured(
                LoRaConfiguration::SubG(configuration),
            )),
            Self::Unconfigured => Some(LoRaConfigurationState::Unconfigured),
            #[cfg(feature = "lora-2g4")]
            Self::Ghz24(profile) => Some(LoRaConfigurationState::Configured(
                LoRaConfiguration::Ghz24(profile),
            )),
            #[cfg(not(feature = "lora-2g4"))]
            Self::UnsupportedGhz24(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoredRecord {
    generation: u64,
    value: StoredValue,
    legacy: bool,
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
        StoredValue::Configuration(_) => CONFIGURATION_KIND,
        StoredValue::Unconfigured => UNCONFIGURED_KIND,
        #[cfg(feature = "lora-2g4")]
        StoredValue::Ghz24(_) => GHZ24_CONFIGURATION_KIND,
        #[cfg(not(feature = "lora-2g4"))]
        StoredValue::UnsupportedGhz24(_) => GHZ24_CONFIGURATION_KIND,
    };
    bytes[7] = 0;
    bytes[8..16].copy_from_slice(&generation.to_le_bytes());
    let payload_len = match value {
        StoredValue::Configuration(configuration) => {
            encode_configuration(configuration, &mut bytes[PAYLOAD_OFFSET..])
        }
        StoredValue::Unconfigured => 0,
        #[cfg(not(feature = "lora-2g4"))]
        StoredValue::UnsupportedGhz24(payload) => {
            bytes[PAYLOAD_OFFSET..PAYLOAD_OFFSET + PROFILE_PAYLOAD_LEN].copy_from_slice(&payload);
            PROFILE_PAYLOAD_LEN
        }
        #[cfg(feature = "lora-2g4")]
        StoredValue::Ghz24(profile) => {
            encode_parameters(
                profile.frequency(),
                profile.modulation(),
                profile.tx_power(),
                profile.preamble(),
                0,
                &mut bytes[PAYLOAD_OFFSET..PAYLOAD_OFFSET + PROFILE_PAYLOAD_LEN],
            );
            PROFILE_PAYLOAD_LEN
        }
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
    let schema = u16::from_le_bytes(bytes[4..6].try_into().ok()?);
    if bytes[..4] != MAGIC
        || !matches!(schema, LEGACY_SCHEMA_VERSION | SCHEMA_VERSION)
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
    let payload = &bytes[PAYLOAD_OFFSET..];
    let (value, legacy) = match (schema, bytes[6], payload_len) {
        (LEGACY_SCHEMA_VERSION, LEGACY_PROFILE_KIND, PROFILE_PAYLOAD_LEN) => {
            let profile = decode_profile(&payload[..PROFILE_PAYLOAD_LEN])?;
            (
                StoredValue::Configuration(manual_configuration(profile)),
                true,
            )
        }
        (LEGACY_SCHEMA_VERSION, LEGACY_DEFAULT_KIND, 0) => {
            (StoredValue::Configuration(Us915::auto_lora()), true)
        }
        (SCHEMA_VERSION, CONFIGURATION_KIND, length) if length <= RECORD_LEN - PAYLOAD_OFFSET => (
            StoredValue::Configuration(decode_configuration(&payload[..length])?),
            false,
        ),
        #[cfg(feature = "lora-2g4")]
        (SCHEMA_VERSION, GHZ24_CONFIGURATION_KIND, PROFILE_PAYLOAD_LEN) => (
            StoredValue::Ghz24(decode_ghz24(&payload[..PROFILE_PAYLOAD_LEN])?),
            false,
        ),
        #[cfg(not(feature = "lora-2g4"))]
        (SCHEMA_VERSION, GHZ24_CONFIGURATION_KIND, PROFILE_PAYLOAD_LEN) => (
            StoredValue::UnsupportedGhz24(payload[..PROFILE_PAYLOAD_LEN].try_into().ok()?),
            false,
        ),
        (SCHEMA_VERSION, UNCONFIGURED_KIND, 0) => (StoredValue::Unconfigured, false),
        _ => return None,
    };
    Some(StoredRecord {
        generation,
        value,
        legacy,
    })
}

fn generation_hint(bytes: &[u8; RECORD_LEN]) -> Option<u64> {
    (bytes[..4] == MAGIC).then(|| {
        u64::from_le_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ])
    })
}

fn encode_configuration(configuration: SubGConfiguration, out: &mut [u8]) -> usize {
    match configuration.resolve() {
        ResolvedSubGMode::LoRa(profile) if configuration.mode() == SubGMode::ManualLoRa => {
            out[0] = MODE_MANUAL_LORA;
            encode_profile(profile, &mut out[1..MANUAL_CONFIGURATION_PAYLOAD_LEN]);
            MANUAL_CONFIGURATION_PAYLOAD_LEN
        }
        ResolvedSubGMode::LoRa(_) => {
            out[0] = MODE_AUTO_LORA;
            out[1] = region_code(configuration.region());
            MODE_CONFIGURATION_PAYLOAD_LEN
        }
    }
}

fn decode_configuration(bytes: &[u8]) -> Option<SubGConfiguration> {
    match (bytes.first().copied()?, bytes.len()) {
        (MODE_AUTO_LORA, MODE_CONFIGURATION_PAYLOAD_LEN) => {
            SubGConfiguration::auto_lora_for(decode_region(bytes[1])?).ok()
        }
        (MODE_MANUAL_LORA, MANUAL_CONFIGURATION_PAYLOAD_LEN) => {
            Some(manual_configuration(decode_profile(&bytes[1..])?))
        }
        (MODE_TURBO, MODE_CONFIGURATION_PAYLOAD_LEN) => None,
        _ => None,
    }
}

fn manual_configuration(profile: RadioProfile) -> SubGConfiguration {
    SubGConfiguration::manual_lora(profile)
}

fn encode_profile(profile: RadioProfile, out: &mut [u8]) {
    encode_parameters(
        profile.frequency(),
        profile.modulation(),
        profile.tx_power(),
        profile.preamble(),
        region_code(profile.region()),
        out,
    )
}

fn encode_parameters(
    frequency: Frequency,
    modulation: Modulation,
    power: TxPower,
    preamble: PreambleSymbols,
    region: u8,
    out: &mut [u8],
) {
    out[..4].copy_from_slice(&frequency.hz().to_le_bytes());
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = modulation;
    out[4] = spreading_factor as u8;
    out[5] = match bandwidth {
        LoraBandwidth::Bw125kHz => 1,
        LoraBandwidth::Bw250kHz => 2,
        LoraBandwidth::Bw500kHz => 3,
        #[cfg(feature = "lora-2g4")]
        LoraBandwidth::Bw203kHz => 4,
        #[cfg(feature = "lora-2g4")]
        LoraBandwidth::Bw406kHz => 5,
        #[cfg(feature = "lora-2g4")]
        LoraBandwidth::Bw812kHz => 6,
    };
    out[6] = coding_rate as u8;
    out[7] = power.dbm().to_le_bytes()[0];
    out[8..10].copy_from_slice(&preamble.count().to_le_bytes());
    out[10] = region;
    out[11] = 0;
}

fn decode_profile(bytes: &[u8]) -> Option<RadioProfile> {
    let (frequency, modulation, power, preamble) = decode_parameters(bytes)?;
    RadioProfile::new(
        decode_region(*bytes.get(10)?)?,
        frequency,
        modulation,
        power,
        preamble,
    )
    .ok()
}

#[cfg(feature = "lora-2g4")]
fn decode_ghz24(bytes: &[u8]) -> Option<personal_rns::interfaces::lora::Ghz24Profile> {
    let (frequency, modulation, power, preamble) = decode_parameters(bytes)?;
    if *bytes.get(10)? != 0 {
        return None;
    }
    personal_rns::interfaces::lora::Ghz24Profile::new(frequency, modulation, power, preamble).ok()
}

fn decode_parameters(bytes: &[u8]) -> Option<(Frequency, Modulation, TxPower, PreambleSymbols)> {
    if bytes.len() != PROFILE_PAYLOAD_LEN || bytes[11] != 0 {
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
        #[cfg(feature = "lora-2g4")]
        4 => LoraBandwidth::Bw203kHz,
        #[cfg(feature = "lora-2g4")]
        5 => LoraBandwidth::Bw406kHz,
        #[cfg(feature = "lora-2g4")]
        6 => LoraBandwidth::Bw812kHz,
        _ => return None,
    };
    let coding_rate = match bytes[6] {
        5 => CodingRate::Cr45,
        6 => CodingRate::Cr46,
        7 => CodingRate::Cr47,
        8 => CodingRate::Cr48,
        _ => return None,
    };
    Some((
        Frequency::new(u32::from_le_bytes(bytes[..4].try_into().ok()?)),
        Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        },
        TxPower::new(i8::from_le_bytes([bytes[7]])),
        PreambleSymbols::new(u16::from_le_bytes(bytes[8..10].try_into().ok()?)),
    ))
}

const fn region_code(region: SubGRegion) -> u8 {
    match region {
        SubGRegion::Regulated(RegulatoryRegion::Us915) => 1,
        SubGRegion::Regulated(RegulatoryRegion::Au915) => 2,
        SubGRegion::Regulated(RegulatoryRegion::Eu433) => 3,
        SubGRegion::Regulated(RegulatoryRegion::Eu865) => 4,
        SubGRegion::Regulated(RegulatoryRegion::Eu868) => 5,
        SubGRegion::Regulated(RegulatoryRegion::Eu869) => 6,
        SubGRegion::Regulated(RegulatoryRegion::As923) => 7,
        SubGRegion::Regulated(RegulatoryRegion::In865) => 8,
        SubGRegion::Regulated(RegulatoryRegion::Cn470) => 9,
        SubGRegion::Regulated(RegulatoryRegion::Kr920) => 10,
        SubGRegion::Regulated(RegulatoryRegion::Jp920) => 11,
        SubGRegion::Custom => 12,
    }
}

const fn decode_region(value: u8) -> Option<SubGRegion> {
    let region = match value {
        1 => SubGRegion::Regulated(RegulatoryRegion::Us915),
        2 => SubGRegion::Regulated(RegulatoryRegion::Au915),
        3 => SubGRegion::Regulated(RegulatoryRegion::Eu433),
        4 => SubGRegion::Regulated(RegulatoryRegion::Eu865),
        5 => SubGRegion::Regulated(RegulatoryRegion::Eu868),
        6 => SubGRegion::Regulated(RegulatoryRegion::Eu869),
        7 => SubGRegion::Regulated(RegulatoryRegion::As923),
        8 => SubGRegion::Regulated(RegulatoryRegion::In865),
        9 => SubGRegion::Regulated(RegulatoryRegion::Cn470),
        10 => SubGRegion::Regulated(RegulatoryRegion::Kr920),
        11 => SubGRegion::Regulated(RegulatoryRegion::Jp920),
        12 => SubGRegion::Custom,
        _ => return None,
    };
    Some(region)
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
    use personal_rns::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE;
    use std::boxed::Box;
    use std::task::Waker;
    use std::vec::Vec;

    const CAPACITY: usize = 2 * 4096;
    const PAGES: [u32; 2] = [0, 4096];

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FakeError {
        Bounds,
        PowerCut,
    }

    impl NorFlashError for FakeError {
        fn kind(&self) -> NorFlashErrorKind {
            NorFlashErrorKind::Other
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Fault {
        ReadBefore(usize),
        ReadCorrupt(usize),
        EraseBefore(usize),
        WriteBefore(usize),
        WriteAfter(usize),
        WritePartial(usize, usize),
    }

    struct FakeFlash<const READ: usize, const WRITE: usize, const ERASE: usize, const CAP: usize> {
        bytes: Box<[u8; CAP]>,
        faults: Vec<Fault>,
        reads: usize,
        erases: usize,
        writes: usize,
    }

    impl<const READ: usize, const WRITE: usize, const ERASE: usize, const CAP: usize>
        FakeFlash<READ, WRITE, ERASE, CAP>
    {
        fn erased() -> Self {
            Self {
                bytes: Box::new([0xFF; CAP]),
                faults: Vec::new(),
                reads: 0,
                erases: 0,
                writes: 0,
            }
        }

        fn from_bytes(bytes: Box<[u8; CAP]>) -> Self {
            Self {
                bytes,
                faults: Vec::new(),
                reads: 0,
                erases: 0,
                writes: 0,
            }
        }

        fn with_fault(mut self, fault: Fault) -> Self {
            self.faults.push(fault);
            self
        }

        fn take_fault(&mut self, fault: Fault) -> bool {
            let Some(index) = self.faults.iter().position(|candidate| *candidate == fault) else {
                return false;
            };
            self.faults.remove(index);
            true
        }

        fn write_bytes(&mut self, offset: u32, bytes: &[u8]) -> Result<(), FakeError> {
            if WRITE == 0
                || !(offset as usize).is_multiple_of(WRITE)
                || !bytes.len().is_multiple_of(WRITE)
            {
                return Err(FakeError::Bounds);
            }
            let start = offset as usize;
            let end = start.checked_add(bytes.len()).ok_or(FakeError::Bounds)?;
            let destination = self.bytes.get_mut(start..end).ok_or(FakeError::Bounds)?;
            for (current, requested) in destination.iter_mut().zip(bytes) {
                *current &= *requested;
            }
            Ok(())
        }
    }

    impl<const READ: usize, const WRITE: usize, const ERASE: usize, const CAP: usize> ErrorType
        for FakeFlash<READ, WRITE, ERASE, CAP>
    {
        type Error = FakeError;
    }

    impl<const READ: usize, const WRITE: usize, const ERASE: usize, const CAP: usize> ReadNorFlash
        for FakeFlash<READ, WRITE, ERASE, CAP>
    {
        const READ_SIZE: usize = READ;

        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            let call = self.reads;
            self.reads += 1;
            if self.take_fault(Fault::ReadBefore(call)) {
                return Err(FakeError::PowerCut);
            }
            if READ == 0
                || !(offset as usize).is_multiple_of(READ)
                || !bytes.len().is_multiple_of(READ)
            {
                return Err(FakeError::Bounds);
            }
            let start = offset as usize;
            let end = start.checked_add(bytes.len()).ok_or(FakeError::Bounds)?;
            bytes.copy_from_slice(self.bytes.get(start..end).ok_or(FakeError::Bounds)?);
            if self.take_fault(Fault::ReadCorrupt(call)) && !bytes.is_empty() {
                bytes[0] ^= 1;
            }
            Ok(())
        }

        fn capacity(&self) -> usize {
            CAP
        }
    }

    impl<const READ: usize, const WRITE: usize, const ERASE: usize, const CAP: usize> NorFlash
        for FakeFlash<READ, WRITE, ERASE, CAP>
    {
        const WRITE_SIZE: usize = WRITE;
        const ERASE_SIZE: usize = ERASE;

        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            let call = self.writes;
            self.writes += 1;
            if self.take_fault(Fault::WriteBefore(call)) {
                return Err(FakeError::PowerCut);
            }
            if let Some(index) = self.faults.iter().position(
                |fault| matches!(fault, Fault::WritePartial(candidate, _) if *candidate == call),
            ) {
                let Fault::WritePartial(_, written) = self.faults.remove(index) else {
                    unreachable!()
                };
                let written = written.min(bytes.len());
                self.write_bytes(offset, &bytes[..written])?;
                return Err(FakeError::PowerCut);
            }
            self.write_bytes(offset, bytes)?;
            if self.take_fault(Fault::WriteAfter(call)) {
                return Err(FakeError::PowerCut);
            }
            Ok(())
        }

        async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            let call = self.erases;
            self.erases += 1;
            if self.take_fault(Fault::EraseBefore(call)) {
                return Err(FakeError::PowerCut);
            }
            if ERASE == 0
                || !(from as usize).is_multiple_of(ERASE)
                || !(to as usize).is_multiple_of(ERASE)
            {
                return Err(FakeError::Bounds);
            }
            self.bytes
                .get_mut(from as usize..to as usize)
                .ok_or(FakeError::Bounds)?
                .fill(0xFF);
            Ok(())
        }
    }

    type ByteFlash = FakeFlash<1, 1, 4096, CAPACITY>;

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = Box::pin(future);
        let mut context = Context::from_waker(Waker::noop());
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn committed(mut record: [u8; RECORD_LEN]) -> [u8; RECORD_LEN] {
        record[COMMIT_OFFSET..COMMIT_OFFSET + 4].copy_from_slice(&COMMIT_WORD.to_le_bytes());
        record
    }

    fn legacy_record(generation: u64, value: Option<RadioProfile>) -> [u8; RECORD_LEN] {
        let mut bytes = [0xFF; RECORD_LEN];
        bytes[..4].copy_from_slice(&MAGIC);
        bytes[4..6].copy_from_slice(&LEGACY_SCHEMA_VERSION.to_le_bytes());
        bytes[6] = if value.is_some() {
            LEGACY_PROFILE_KIND
        } else {
            LEGACY_DEFAULT_KIND
        };
        bytes[7] = 0;
        bytes[8..16].copy_from_slice(&generation.to_le_bytes());
        let payload_len = if let Some(profile) = value {
            encode_profile(
                profile,
                &mut bytes[PAYLOAD_OFFSET..PAYLOAD_OFFSET + PROFILE_PAYLOAD_LEN],
            );
            PROFILE_PAYLOAD_LEN
        } else {
            0
        };
        bytes[16..18].copy_from_slice(&(payload_len as u16).to_le_bytes());
        bytes[18..20].fill(0);
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].fill(0);
        bytes[24..28].fill(0);
        bytes[COMMIT_OFFSET..COMMIT_OFFSET + 4].fill(0);
        let checksum = crc32(&bytes);
        bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&checksum.to_le_bytes());
        committed(bytes)
    }

    fn expect_committed(outcome: SubGConfigurationCommitOutcome<FakeError>) {
        assert_eq!(outcome, SubGConfigurationCommitOutcome::Committed);
    }

    fn manual_configuration_for(region: SubGRegion) -> SubGConfiguration {
        SubGConfiguration::manual_lora_for(region, region.manual_lora_defaults()).unwrap()
    }

    fn record_with_mode(mode: u8, region: u8) -> [u8; RECORD_LEN] {
        let mut record = encode_record(0, StoredValue::Configuration(Us915::auto_lora()));
        record[PAYLOAD_OFFSET] = mode;
        record[PAYLOAD_OFFSET + 1] = region;
        record[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].fill(0);
        record[COMMIT_OFFSET..COMMIT_OFFSET + 4].fill(0);
        let checksum = crc32(&record);
        record[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&checksum.to_le_bytes());
        committed(record)
    }

    #[test]
    fn mode_and_region_decoders_reject_unknown_tags_and_inexact_lengths() {
        assert_eq!(decode_configuration(&[]), None);
        for tag in 0..=u8::MAX {
            assert_eq!(decode_region(tag).is_some(), (1..=12).contains(&tag));
            for length in 1..=MANUAL_CONFIGURATION_PAYLOAD_LEN + 1 {
                let mut bytes = std::vec![0; length];
                bytes[0] = tag;
                if length > 1 {
                    bytes[1] = 1;
                }
                assert_eq!(
                    decode_configuration(&bytes).is_some(),
                    tag == MODE_AUTO_LORA && length == MODE_CONFIGURATION_PAYLOAD_LEN
                );
            }
            assert_eq!(
                decode_configuration(&[MODE_AUTO_LORA, tag]).is_some(),
                tag == 1
            );
        }
    }

    #[test]
    fn journal_checksum_matches_the_ieee_crc32_check_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn legacy_profile_codec_preserves_each_sub_ghz_bandwidth() {
        for bandwidth in [
            LoraBandwidth::Bw125kHz,
            LoraBandwidth::Bw250kHz,
            LoraBandwidth::Bw500kHz,
        ] {
            let profile = US915_AUTO_LORA_PROFILE
                .with_modulation(Modulation::Lora {
                    spreading_factor: SpreadingFactor::Sf7,
                    bandwidth,
                    coding_rate: CodingRate::Cr45,
                })
                .unwrap();
            let mut bytes = [0; PROFILE_PAYLOAD_LEN];
            encode_profile(profile, &mut bytes);
            assert_eq!(decode_profile(&bytes), Some(profile));
        }
    }

    #[test]
    fn authenticated_but_invalid_record_headers_and_lengths_are_rejected() {
        for (offset, value) in [
            (0, 0),
            (4, 0xff),
            (6, 0xff),
            (7, 1),
            (16, 0xff),
            (16, 17),
            (18, 1),
            (24, 1),
        ] {
            let mut record = encode_record(9, StoredValue::Configuration(Us915::auto_lora()));
            record[offset] = value;
            record[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].fill(0);
            record[COMMIT_OFFSET..COMMIT_OFFSET + 4].fill(0);
            let checksum = crc32(&record);
            record[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&checksum.to_le_bytes());
            assert_eq!(
                decode_record(&committed(record)),
                None,
                "header offset {offset}"
            );
        }
    }

    #[test]
    fn an_invalid_older_slot_does_not_report_recovery_of_a_newer_valid_record() {
        for corrupt_generation in [7, 9] {
            let mut flash = ByteFlash::erased();
            flash.bytes[..RECORD_LEN].copy_from_slice(&committed(encode_record(
                8,
                StoredValue::Configuration(Us915::auto_lora()),
            )));
            let mut invalid =
                committed(encode_record(corrupt_generation, StoredValue::Unconfigured));
            invalid[CHECKSUM_OFFSET] ^= 1;
            flash.bytes[4096..4096 + RECORD_LEN].copy_from_slice(&invalid);
            let mut store = SubGConfigurationStore::new(flash, PAGES);
            let loaded = block_on(store.load_radio()).unwrap();
            assert_eq!(
                loaded.notice,
                if corrupt_generation == 9 {
                    Some(SubGConfigurationLoadNotice::Recovered)
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn reconciliation_with_an_unexpected_valid_generation_stays_indeterminate() {
        let mut flash = ByteFlash::erased();
        flash.bytes[4096..4096 + RECORD_LEN]
            .copy_from_slice(&committed(encode_record(9, StoredValue::Unconfigured)));
        let mut store = SubGConfigurationStore::new(flash, PAGES);
        assert_eq!(
            block_on(store.reconcile_commit(
                StoredRecord {
                    generation: 10,
                    value: StoredValue::Unconfigured,
                    legacy: false
                },
                Some(StoredRecord {
                    generation: 8,
                    value: StoredValue::Unconfigured,
                    legacy: false
                }),
                SubGConfigurationStoreError::VerificationFailed,
            )),
            SubGConfigurationCommitOutcome::Indeterminate(
                SubGConfigurationStoreError::VerificationFailed
            )
        );
    }

    #[test]
    fn erased_flash_is_unconfigured() {
        let mut store = SubGConfigurationStore::new(ByteFlash::erased(), PAGES);
        assert_eq!(
            block_on(store.load()).unwrap(),
            LoadedSubGConfiguration {
                state: SubGConfigurationState::Unconfigured,
                notice: None,
            }
        );
    }

    #[test]
    fn configured_and_cleared_states_round_trip() {
        let mut store = SubGConfigurationStore::new(ByteFlash::erased(), PAGES);
        let configuration = Us915::auto_lora();
        expect_committed(block_on(store.save(configuration)));
        assert_eq!(
            block_on(store.load()).unwrap().state,
            SubGConfigurationState::Configured(configuration)
        );
        expect_committed(block_on(store.clear()));
        assert_eq!(
            block_on(store.load()).unwrap().state,
            SubGConfigurationState::Unconfigured
        );
    }

    #[test]
    fn remote_profile_changes_and_clears_survive_a_reboot() {
        use crate::apply_remote_subg_configuration;
        let mut store = SubGConfigurationStore::new(ByteFlash::erased(), PAGES);
        let mut active = SubGConfigurationState::Unconfigured;
        for requested in [
            SubGConfigurationState::Configured(manual_configuration_for(SubGRegion::Custom)),
            SubGConfigurationState::Configured(Us915::auto_lora()),
            SubGConfigurationState::Unconfigured,
        ] {
            let mut applied = Vec::new();
            assert_eq!(
                block_on(apply_remote_subg_configuration(
                    async |state| {
                        applied.push(state);
                        Ok(())
                    },
                    &mut store,
                    &mut active,
                    requested,
                )),
                Ok(())
            );
            let mut rebooted = SubGConfigurationStore::new(
                ByteFlash::from_bytes(store.flash.bytes.clone()),
                PAGES,
            );
            assert_eq!(
                (active, block_on(rebooted.load()).unwrap().state, applied),
                (requested, requested, std::vec![requested])
            );
        }
    }

    #[test]
    fn rejected_remote_profile_never_changes_flash_or_active_state() {
        use personal_rns::runtime::RemoteControlHostCommandError as Error;
        let mut store = SubGConfigurationStore::new(ByteFlash::erased(), PAGES);
        let mut active = SubGConfigurationState::Unconfigured;
        let result = block_on(crate::apply_remote_subg_configuration(
            async |_| Err(Error::ApplyFailed),
            &mut store,
            &mut active,
            SubGConfigurationState::Configured(Us915::auto_lora()),
        ));
        assert_eq!(
            (result, active, store.flash.writes, store.flash.erases),
            (
                Err(Error::ApplyFailed),
                SubGConfigurationState::Unconfigured,
                0,
                0
            )
        );
    }

    #[test]
    fn failed_remote_profile_save_restores_radio_and_reboot_state() {
        use personal_rns::runtime::RemoteControlHostCommandError as Error;
        let requested = SubGConfigurationState::Configured(Us915::auto_lora());
        for flash in [
            ByteFlash::erased().with_fault(Fault::WriteBefore(0)),
            ByteFlash::erased()
                .with_fault(Fault::WriteAfter(2))
                .with_fault(Fault::ReadBefore(2)),
        ] {
            let mut store = SubGConfigurationStore::new(flash, PAGES);
            let mut active = SubGConfigurationState::Unconfigured;
            let mut applied = Vec::new();
            let result = block_on(crate::apply_remote_subg_configuration(
                async |state| {
                    applied.push(state);
                    Ok(())
                },
                &mut store,
                &mut active,
                requested,
            ));
            let mut rebooted =
                SubGConfigurationStore::new(ByteFlash::from_bytes(store.flash.bytes), PAGES);
            assert_eq!(
                (
                    result,
                    active,
                    block_on(rebooted.load()).unwrap().state,
                    applied
                ),
                (
                    Err(Error::PersistenceFailed),
                    SubGConfigurationState::Unconfigured,
                    SubGConfigurationState::Unconfigured,
                    std::vec![requested, SubGConfigurationState::Unconfigured]
                )
            );
        }
    }

    #[test]
    fn unconfirmed_flash_rollback_is_reported_instead_of_claiming_a_restored_profile() {
        use personal_rns::runtime::RemoteControlHostCommandError as Error;
        let flash = ByteFlash::erased()
            .with_fault(Fault::WriteAfter(2))
            .with_fault(Fault::ReadBefore(2))
            .with_fault(Fault::WriteBefore(3));
        let mut store = SubGConfigurationStore::new(flash, PAGES);
        let mut active = SubGConfigurationState::Unconfigured;
        let requested = SubGConfigurationState::Configured(Us915::auto_lora());
        let result = block_on(crate::apply_remote_subg_configuration(
            async |_| Ok(()),
            &mut store,
            &mut active,
            requested,
        ));
        let mut rebooted =
            SubGConfigurationStore::new(ByteFlash::from_bytes(store.flash.bytes), PAGES);
        assert_eq!(
            (result, active, block_on(rebooted.load()).unwrap().state),
            (
                Err(Error::RollbackFailed),
                SubGConfigurationState::Unconfigured,
                requested
            )
        );
    }

    #[test]
    fn rejected_radio_rollback_preserves_the_actual_running_state() {
        use personal_rns::runtime::RemoteControlHostCommandError as Error;
        let mut store = SubGConfigurationStore::new(
            ByteFlash::erased().with_fault(Fault::WriteBefore(0)),
            PAGES,
        );
        let mut active = SubGConfigurationState::Unconfigured;
        let requested = SubGConfigurationState::Configured(Us915::auto_lora());
        let result = block_on(crate::apply_remote_subg_configuration(
            async |state| {
                if state == requested {
                    Ok(())
                } else {
                    Err(Error::ApplyFailed)
                }
            },
            &mut store,
            &mut active,
            requested,
        ));
        assert_eq!((result, active), (Err(Error::RollbackFailed), requested));
    }

    #[test]
    fn retrying_the_running_profile_after_failed_rollback_confirms_durability() {
        use personal_rns::runtime::RemoteControlHostCommandError as Error;
        let mut store = SubGConfigurationStore::new(
            ByteFlash::erased().with_fault(Fault::WriteBefore(0)),
            PAGES,
        );
        let mut active = SubGConfigurationState::Unconfigured;
        let requested = SubGConfigurationState::Configured(Us915::auto_lora());
        assert_eq!(
            block_on(crate::apply_remote_subg_configuration(
                async |state| if state == requested {
                    Ok(())
                } else {
                    Err(Error::ApplyFailed)
                },
                &mut store,
                &mut active,
                requested,
            )),
            Err(Error::RollbackFailed)
        );
        assert_eq!(active, requested);
        assert_eq!(
            block_on(crate::apply_remote_subg_configuration(
                async |_| panic!("the radio already has the requested profile"),
                &mut store,
                &mut active,
                requested,
            )),
            Ok(())
        );
        let mut rebooted =
            SubGConfigurationStore::new(ByteFlash::from_bytes(store.flash.bytes), PAGES);
        assert_eq!(block_on(rebooted.load()).unwrap().state, requested);
    }

    #[test]
    fn every_representable_configuration_round_trips() {
        let mut configurations = std::vec![Us915::auto_lora()];
        for region in RegulatoryRegion::ALL {
            configurations.push(manual_configuration_for(SubGRegion::Regulated(region)));
        }
        configurations.push(manual_configuration_for(SubGRegion::Custom));

        for configuration in configurations {
            let mut store = SubGConfigurationStore::new(ByteFlash::erased(), PAGES);
            expect_committed(block_on(store.save(configuration)));
            assert_eq!(
                block_on(store.load()).unwrap().state,
                SubGConfigurationState::Configured(configuration)
            );
        }
    }

    #[test]
    fn legacy_explicit_profile_becomes_manual_and_legacy_default_becomes_auto() {
        let mut profile_flash = ByteFlash::erased();
        profile_flash.bytes[..RECORD_LEN]
            .copy_from_slice(&legacy_record(1, Some(US915_AUTO_LORA_PROFILE)));
        let mut profile_store = SubGConfigurationStore::new(profile_flash, PAGES);
        let migrated = block_on(profile_store.load()).unwrap();
        assert_eq!(migrated.notice, Some(SubGConfigurationLoadNotice::Migrated));
        let SubGConfigurationState::Configured(configuration) = migrated.state else {
            panic!("expected migrated configuration")
        };
        assert_eq!(configuration.mode(), SubGMode::ManualLoRa);

        let mut default_flash = ByteFlash::erased();
        default_flash.bytes[..RECORD_LEN].copy_from_slice(&legacy_record(1, None));
        let mut default_store = SubGConfigurationStore::new(default_flash, PAGES);
        let migrated = block_on(default_store.load()).unwrap();
        let SubGConfigurationState::Configured(configuration) = migrated.state else {
            panic!("expected migrated configuration")
        };
        assert_eq!(configuration, Us915::auto_lora());
    }

    #[test]
    fn interrupted_record_write_preserves_the_last_committed_configuration() {
        let mut initial = SubGConfigurationStore::new(ByteFlash::erased(), PAGES);
        expect_committed(block_on(initial.save(Us915::auto_lora())));
        let flash =
            ByteFlash::from_bytes(initial.into_flash().bytes).with_fault(Fault::WritePartial(1, 4));
        let mut interrupted = SubGConfigurationStore::new(flash, PAGES);
        assert_eq!(
            block_on(interrupted.clear()),
            SubGConfigurationCommitOutcome::NotCommitted(SubGConfigurationStoreError::Flash {
                operation: SubGConfigurationFlashOperation::WriteRecord,
                error: FakeError::PowerCut,
            })
        );
        let flash = interrupted.into_flash();
        let mut rebooted = SubGConfigurationStore::new(ByteFlash::from_bytes(flash.bytes), PAGES);
        assert_eq!(
            block_on(rebooted.load()).unwrap().state,
            SubGConfigurationState::Configured(Us915::auto_lora())
        );
    }

    #[test]
    fn a_commit_write_that_landed_before_error_reconciles_as_committed() {
        let flash = ByteFlash::erased().with_fault(Fault::WriteAfter(2));
        let mut store = SubGConfigurationStore::new(flash, PAGES);
        expect_committed(block_on(store.save(Us915::auto_lora())));
        assert_eq!(
            block_on(store.load()).unwrap().state,
            SubGConfigurationState::Configured(Us915::auto_lora())
        );
    }

    #[test]
    fn a_commit_write_that_did_not_land_reconciles_as_not_committed() {
        let flash = ByteFlash::erased().with_fault(Fault::WriteBefore(2));
        let mut store = SubGConfigurationStore::new(flash, PAGES);
        assert_eq!(
            block_on(store.save(Us915::auto_lora())),
            SubGConfigurationCommitOutcome::NotCommitted(SubGConfigurationStoreError::Flash {
                operation: SubGConfigurationFlashOperation::WriteCommit,
                error: FakeError::PowerCut,
            })
        );
    }

    #[test]
    fn matching_readback_completes_without_an_unnecessary_reconciliation_read() {
        let flash = ByteFlash::erased().with_fault(Fault::ReadBefore(3));
        let mut store = SubGConfigurationStore::new(flash, PAGES);
        expect_committed(block_on(store.save(Us915::auto_lora())));
        assert_eq!(store.flash.reads, 3);
    }

    #[test]
    fn failed_or_corrupt_verification_reconciles_a_committed_record() {
        for fault in [Fault::ReadBefore(2), Fault::ReadCorrupt(2)] {
            let flash = ByteFlash::erased().with_fault(fault);
            let mut store = SubGConfigurationStore::new(flash, PAGES);
            expect_committed(block_on(store.save(Us915::auto_lora())));
        }
    }

    #[test]
    fn reconciliation_read_failure_is_indeterminate() {
        let flash = ByteFlash::erased()
            .with_fault(Fault::WriteAfter(2))
            .with_fault(Fault::ReadBefore(2));
        let mut store = SubGConfigurationStore::new(flash, PAGES);
        assert_eq!(
            block_on(store.save(Us915::auto_lora())),
            SubGConfigurationCommitOutcome::Indeterminate(SubGConfigurationStoreError::Flash {
                operation: SubGConfigurationFlashOperation::Reconcile,
                error: FakeError::PowerCut,
            })
        );
    }

    #[test]
    fn precommit_failures_are_definitively_not_committed() {
        for (fault, operation) in [
            (Fault::ReadBefore(0), SubGConfigurationFlashOperation::Read),
            (
                Fault::EraseBefore(0),
                SubGConfigurationFlashOperation::Erase,
            ),
            (
                Fault::WriteBefore(0),
                SubGConfigurationFlashOperation::WriteRecord,
            ),
        ] {
            let flash = ByteFlash::erased().with_fault(fault);
            let mut store = SubGConfigurationStore::new(flash, PAGES);
            assert_eq!(
                block_on(store.save(Us915::auto_lora())),
                SubGConfigurationCommitOutcome::NotCommitted(SubGConfigurationStoreError::Flash {
                    operation,
                    error: FakeError::PowerCut,
                })
            );
        }
    }

    #[test]
    fn unsupported_or_region_mismatched_auto_modes_reset_on_load() {
        for record in [
            record_with_mode(MODE_TURBO, 1),
            record_with_mode(MODE_AUTO_LORA, 2),
        ] {
            let mut flash = ByteFlash::erased();
            flash.bytes[..RECORD_LEN].copy_from_slice(&record);
            let mut store = SubGConfigurationStore::new(flash, PAGES);
            assert_eq!(
                block_on(store.load()).unwrap(),
                LoadedSubGConfiguration {
                    state: SubGConfigurationState::Unconfigured,
                    notice: Some(SubGConfigurationLoadNotice::Reset),
                }
            );
        }
    }

    #[test]
    fn a_corrupt_newer_slot_recovers_the_previous_configuration() {
        let mut flash = ByteFlash::erased();
        let previous = committed(encode_record(
            7,
            StoredValue::Configuration(Us915::auto_lora()),
        ));
        let mut corrupt = committed(encode_record(8, StoredValue::Unconfigured));
        corrupt[PAYLOAD_OFFSET] ^= 1;
        flash.bytes[..RECORD_LEN].copy_from_slice(&previous);
        flash.bytes[4096..4096 + RECORD_LEN].copy_from_slice(&corrupt);
        let mut store = SubGConfigurationStore::new(flash, PAGES);
        assert_eq!(
            block_on(store.load()).unwrap(),
            LoadedSubGConfiguration {
                state: SubGConfigurationState::Configured(Us915::auto_lora()),
                notice: Some(SubGConfigurationLoadNotice::Recovered),
            }
        );
    }

    #[test]
    fn generation_selection_handles_wraparound() {
        let before_wrap = StoredRecord {
            generation: u64::MAX,
            value: StoredValue::Configuration(Us915::auto_lora()),
            legacy: false,
        };
        let after_wrap = StoredRecord {
            generation: 0,
            value: StoredValue::Unconfigured,
            legacy: false,
        };
        assert_eq!(
            select_active(&[Slot::Valid(before_wrap), Slot::Valid(after_wrap)]),
            Some(1)
        );
        assert!(!generation_is_newer(1u64 << 63, 0));
    }

    #[test]
    fn supported_flash_alignment_shapes_round_trip() {
        type WordFlash = FakeFlash<4, 4, 4096, CAPACITY>;
        type WideReadFlash = FakeFlash<8, 4, 4096, CAPACITY>;
        let mut word = SubGConfigurationStore::new(WordFlash::erased(), PAGES);
        assert_eq!(
            block_on(word.save(Us915::auto_lora())),
            SubGConfigurationCommitOutcome::Committed
        );
        let mut wide_read = SubGConfigurationStore::new(WideReadFlash::erased(), PAGES);
        assert_eq!(
            block_on(wide_read.save(Us915::auto_lora())),
            SubGConfigurationCommitOutcome::Committed
        );
    }

    #[test]
    fn flash_layout_validation_covers_independent_granularities_and_exact_page_size() {
        fn valid<const READ: usize, const WRITE: usize, const ERASE: usize>(
            pages: [u32; 2],
        ) -> bool {
            SubGConfigurationStore::new(FakeFlash::<READ, WRITE, ERASE, 8192>::erased(), pages)
                .validate_layout()
                .is_ok()
        }
        assert!(valid::<1, 1, 48>([0, 48]));
        assert!(valid::<1, 4, 48>([0, 96]));
        assert!(valid::<1, 4, 48>([96, 0]));
        assert!(valid::<3, 4, 48>([0, 48]));
        assert!(valid::<48, 4, 48>([48, 0]));
        assert!(valid::<1, 4, 4096>([4096, 0]));
        assert!(!valid::<0, 4, 4096>(PAGES));
        assert!(!valid::<1, 0, 4096>(PAGES));
        assert!(!valid::<1, 4, 0>(PAGES));
        assert!(!valid::<5, 4, 4096>(PAGES));
        assert!(!valid::<1, 3, 4096>(PAGES));
        assert!(!valid::<1, 4, 32>([0, 32]));
        assert!(!valid::<1, 4, 4096>([0, 4095]));
        assert!(!valid::<3, 4, 4096>(PAGES));
        assert!(!valid::<1, 4, 4097>([0, 4097]));
        assert!(!valid::<1, 4, 49>([0, 49]));
        assert!(!valid::<3, 1, 49>([0, 49]));
        assert!(!valid::<1, 4, 4096>([0, 8192]));
        assert!(!valid::<1, 4, 4096>([0, 0]));
        #[cfg(target_pointer_width = "64")]
        assert!(!valid::<1, 4, { u32::MAX as usize + 1 }>(PAGES));
    }

    #[cfg(feature = "lora-2g4")]
    #[test]
    fn high_frequency_storage_codec_covers_every_modulation_and_rejects_unknown_fields() {
        use personal_rns::interfaces::lora::{Ghz24Profile, GHZ24_BALANCED_PROFILE};
        for (sf, sf_wire) in [
            (SpreadingFactor::Sf5, 5),
            (SpreadingFactor::Sf6, 6),
            (SpreadingFactor::Sf7, 7),
            (SpreadingFactor::Sf8, 8),
            (SpreadingFactor::Sf9, 9),
            (SpreadingFactor::Sf10, 10),
            (SpreadingFactor::Sf11, 11),
            (SpreadingFactor::Sf12, 12),
        ] {
            for (bw, bw_wire) in [
                (LoraBandwidth::Bw203kHz, 4),
                (LoraBandwidth::Bw406kHz, 5),
                (LoraBandwidth::Bw812kHz, 6),
            ] {
                for (cr, cr_wire) in [
                    (CodingRate::Cr45, 5),
                    (CodingRate::Cr46, 6),
                    (CodingRate::Cr47, 7),
                    (CodingRate::Cr48, 8),
                ] {
                    let profile = Ghz24Profile::new(
                        GHZ24_BALANCED_PROFILE.frequency(),
                        Modulation::Lora {
                            spreading_factor: sf,
                            bandwidth: bw,
                            coding_rate: cr,
                        },
                        TxPower::new(11),
                        PreambleSymbols::new(18),
                    )
                    .unwrap();
                    let mut bytes = [0; PROFILE_PAYLOAD_LEN];
                    encode_parameters(
                        profile.frequency(),
                        profile.modulation(),
                        profile.tx_power(),
                        profile.preamble(),
                        0,
                        &mut bytes,
                    );
                    assert_eq!(&bytes[4..7], &[sf_wire, bw_wire, cr_wire]);
                    assert_eq!(decode_ghz24(&bytes), Some(profile));
                    for index in [4, 5, 6, 10, 11] {
                        let mut invalid = bytes;
                        invalid[index] = 0xff;
                        assert_eq!(decode_ghz24(&invalid), None);
                    }
                    for len in 0..PROFILE_PAYLOAD_LEN {
                        assert_eq!(decode_ghz24(&bytes[..len]), None);
                    }
                }
            }
        }
    }

    #[test]
    fn incompatible_flash_shapes_and_page_layouts_fail_closed() {
        type WideWriteFlash = FakeFlash<1, 8, 4096, CAPACITY>;
        type SmallEraseFlash = FakeFlash<1, 1, 32, 64>;
        type MisalignedPageFlash = FakeFlash<4, 4, 4098, 8196>;
        let mut wide_write = SubGConfigurationStore::new(WideWriteFlash::erased(), PAGES);
        assert_eq!(
            block_on(wide_write.save(Us915::auto_lora())),
            SubGConfigurationCommitOutcome::NotCommitted(
                SubGConfigurationStoreError::InvalidLayout
            )
        );
        let mut small_erase = SubGConfigurationStore::new(SmallEraseFlash::erased(), [0, 32]);
        assert_eq!(
            block_on(small_erase.save(Us915::auto_lora())),
            SubGConfigurationCommitOutcome::NotCommitted(
                SubGConfigurationStoreError::InvalidLayout
            )
        );
        let mut overlapping = SubGConfigurationStore::new(ByteFlash::erased(), [0, 0]);
        assert_eq!(
            block_on(overlapping.save(Us915::auto_lora())),
            SubGConfigurationCommitOutcome::NotCommitted(
                SubGConfigurationStoreError::InvalidLayout
            )
        );
        let mut misaligned = SubGConfigurationStore::new(MisalignedPageFlash::erased(), [0, 4098]);
        assert_eq!(
            block_on(misaligned.save(Us915::auto_lora())),
            SubGConfigurationCommitOutcome::NotCommitted(
                SubGConfigurationStoreError::InvalidLayout
            )
        );
    }
    #[cfg(feature = "lora-2g4")]
    #[test]
    fn both_bands_and_clear_share_the_existing_journal() {
        use personal_rns::interfaces::lora::GHZ24_BALANCED_PROFILE;
        let mut store = SubGConfigurationStore::new(ByteFlash::erased(), PAGES);
        for state in [
            LoRaConfigurationState::Configured(LoRaConfiguration::SubG(Us915::auto_lora())),
            LoRaConfigurationState::Configured(LoRaConfiguration::Ghz24(GHZ24_BALANCED_PROFILE)),
            LoRaConfigurationState::Unconfigured,
        ] {
            assert_eq!(
                block_on(store.save_radio(state)),
                SubGConfigurationCommitOutcome::Committed
            );
            assert_eq!(block_on(store.load_radio()).unwrap().state, state);
        }
        let mut profile = GHZ24_BALANCED_PROFILE;
        for bandwidth in [
            LoraBandwidth::Bw203kHz,
            LoraBandwidth::Bw406kHz,
            LoraBandwidth::Bw812kHz,
        ] {
            profile = profile
                .with_modulation(Modulation::Lora {
                    spreading_factor: SpreadingFactor::Sf7,
                    bandwidth,
                    coding_rate: CodingRate::Cr45,
                })
                .unwrap();
            let state = LoRaConfigurationState::Configured(LoRaConfiguration::Ghz24(profile));
            assert_eq!(
                block_on(store.save_radio(state)),
                SubGConfigurationCommitOutcome::Committed
            );
            assert_eq!(block_on(store.load_radio()).unwrap().state, state);
            assert_eq!(
                block_on(store.load()),
                Err(SubGConfigurationStoreError::UnsupportedConfiguration)
            );
        }
    }

    #[cfg(not(feature = "lora-2g4"))]
    #[test]
    fn unsupported_committed_band_does_not_resurrect_an_older_subg_channel() {
        let mut store = SubGConfigurationStore::new(ByteFlash::erased(), PAGES);
        assert_eq!(
            block_on(store.save(Us915::auto_lora())),
            SubGConfigurationCommitOutcome::Committed
        );
        assert_eq!(
            block_on(store.commit(StoredValue::UnsupportedGhz24([0; PROFILE_PAYLOAD_LEN]))),
            SubGConfigurationCommitOutcome::Committed
        );
        assert_eq!(
            block_on(store.load_radio()),
            Err(SubGConfigurationStoreError::UnsupportedConfiguration)
        );
        assert_eq!(
            block_on(store.clear()),
            SubGConfigurationCommitOutcome::Committed
        );
        assert_eq!(
            block_on(store.load_radio()).unwrap().state,
            LoRaConfigurationState::Unconfigured
        );
    }

    struct ScriptedRadio {
        operations: Vec<personal_rns::interfaces::lora::configuration::RadioConfigurationOperation>,
        failures: Vec<usize>,
    }
    impl personal_rns::interfaces::lora::configuration::ConfigurationRadio for ScriptedRadio {
        async fn perform(
            &mut self,
            operation: personal_rns::interfaces::lora::configuration::RadioConfigurationOperation,
        ) -> personal_rns::interfaces::lora::configuration::ConfigurationCompletion {
            use personal_rns::interfaces::lora::configuration::ConfigurationCompletion;
            let index = self.operations.len();
            self.operations.push(operation);
            if self.failures.contains(&index) {
                ConfigurationCompletion::Failed
            } else {
                ConfigurationCompletion::Succeeded
            }
        }
    }

    #[test]
    fn configuration_service_settles_every_radio_fault_and_recovers_storage() {
        use crate::{LoRaConfigurationResult, LoRaConfigurationService};
        use personal_rns::interfaces::lora::configuration::*;
        let previous = LoRaConfigurationState::Unconfigured;
        let requested =
            LoRaConfigurationState::Configured(LoRaConfiguration::SubG(Us915::auto_lora()));
        for failures in [
            Vec::new(),
            std::vec![0],
            std::vec![1],
            std::vec![2],
            std::vec![1, 2],
            std::vec![2, 3],
            std::vec![2, 4],
        ] {
            let mut store = SubGConfigurationStore::new(ByteFlash::erased(), PAGES);
            let mut radio = ScriptedRadio {
                operations: Vec::new(),
                failures,
            };
            let mut service = LoRaConfigurationService::new(previous);
            let result = block_on(service.apply(requested, &mut radio, &mut store));
            if result == LoRaConfigurationResult::Saved {
                assert_eq!(
                    radio.operations,
                    [
                        RadioConfigurationOperation::Quiesce,
                        RadioConfigurationOperation::Stage(requested),
                        RadioConfigurationOperation::Publish
                    ]
                );
                assert_eq!(
                    service.hardware(),
                    HardwareConfiguration::Confirmed(requested)
                );
                assert_eq!(
                    service.durable(),
                    DurableConfiguration::Confirmed(requested)
                );
                assert_eq!(block_on(store.load_radio()).unwrap().state, requested);
                let count = radio.operations.len();
                assert_eq!(
                    block_on(service.apply(requested, &mut radio, &mut store)),
                    LoRaConfigurationResult::Saved
                );
                assert_eq!(radio.operations.len(), count);
            } else {
                assert_eq!(service.durable(), DurableConfiguration::Confirmed(previous));
                assert_eq!(block_on(store.load_radio()).unwrap().state, previous);
                if service.hardware() == HardwareConfiguration::Unknown {
                    assert_eq!(
                        radio.operations.last(),
                        Some(&RadioConfigurationOperation::Hold)
                    );
                    assert_eq!(
                        block_on(service.apply(requested, &mut radio, &mut store)),
                        LoRaConfigurationResult::Refused(ConfigurationRefusal::RecoveryRequired)
                    );
                } else {
                    assert_eq!(
                        radio.operations.last(),
                        Some(&RadioConfigurationOperation::Resume)
                    );
                }
            }
        }
    }

    #[test]
    fn configuration_service_reconciles_indeterminate_flash_before_resuming() {
        use crate::{LoRaConfigurationResult, LoRaConfigurationService};
        use personal_rns::interfaces::lora::configuration::*;
        let previous = LoRaConfigurationState::Unconfigured;
        let requested =
            LoRaConfigurationState::Configured(LoRaConfiguration::SubG(Us915::auto_lora()));
        for flash in [
            ByteFlash::erased().with_fault(Fault::WriteBefore(0)),
            ByteFlash::erased()
                .with_fault(Fault::WriteAfter(2))
                .with_fault(Fault::ReadBefore(2)),
            ByteFlash::erased()
                .with_fault(Fault::WriteAfter(2))
                .with_fault(Fault::ReadBefore(2))
                .with_fault(Fault::EraseBefore(1)),
        ] {
            let mut store = SubGConfigurationStore::new(flash, PAGES);
            let mut radio = ScriptedRadio {
                operations: Vec::new(),
                failures: Vec::new(),
            };
            let mut service = LoRaConfigurationService::new(previous);
            let result = block_on(service.apply(requested, &mut radio, &mut store));
            assert_ne!(result, LoRaConfigurationResult::Saved);
            assert!(!radio
                .operations
                .contains(&RadioConfigurationOperation::Publish));
            if service.durable() == DurableConfiguration::Unknown {
                assert_eq!(
                    radio.operations.last(),
                    Some(&RadioConfigurationOperation::Hold)
                );
            } else {
                assert_eq!(block_on(store.load_radio()).unwrap().state, previous);
                assert_eq!(
                    radio.operations.last(),
                    Some(&RadioConfigurationOperation::Resume)
                );
            }
        }
    }
    #[cfg(feature = "embedded")]
    #[test]
    fn admitted_radio_configuration_survives_requester_disconnect_and_releases_its_slot() {
        use crate::{HopspotCommandMailbox, LoRaConfigurationResult, LoRaConfigurationService};
        use personal_rns::interfaces::InterfaceId;
        use personal_rns::remote_control::{
            RemoteControlLoRaProfile, RemoteControlRadioConfiguration, RemoteControlRadioOutcome,
        };
        use personal_rns::runtime::{
            RemoteControlHostCommand, RemoteControlHostCommandError, RemoteControlHostResponse,
        };
        let mailbox = Box::leak(Box::new(HopspotCommandMailbox::<1>::new()));
        let id = InterfaceId::new([0x42; 8]);
        let profile = LoRaConfiguration::manual(US915_AUTO_LORA_PROFILE.into());
        let configuration = RemoteControlRadioConfiguration::Profile(
            RemoteControlLoRaProfile::from_band_profile(profile.profile()).unwrap(),
        );
        let mut caller = Box::pin(
            mailbox
                .handle()
                .execute(RemoteControlHostCommand::ConfigureRadio { id, configuration }),
        );
        let mut context = Context::from_waker(Waker::noop());
        assert!(caller.as_mut().poll(&mut context).is_pending());
        let admitted = block_on(mailbox.receive());
        let (token, command) = admitted.into_parts();
        drop(caller);
        assert_eq!(
            block_on(
                mailbox
                    .handle()
                    .execute(RemoteControlHostCommand::InspectRadio { id })
            ),
            Err(RemoteControlHostCommandError::Busy)
        );
        let RemoteControlHostCommand::ConfigureRadio {
            id: admitted_id,
            configuration: admitted_configuration,
        } = command
        else {
            panic!("wrong admitted command");
        };
        assert_eq!(admitted_id, id);
        assert_eq!(admitted_configuration, configuration);
        let mut service = LoRaConfigurationService::new(LoRaConfigurationState::Unconfigured);
        let mut store = SubGConfigurationStore::new(ByteFlash::erased(), PAGES);
        let mut radio = ScriptedRadio {
            operations: Vec::new(),
            failures: Vec::new(),
        };
        let requested = LoRaConfigurationState::Configured(profile);
        assert_eq!(
            block_on(service.apply(requested, &mut radio, &mut store)),
            LoRaConfigurationResult::Saved
        );
        mailbox.complete(
            token,
            Ok(RemoteControlHostResponse::ConfigureRadio(
                RemoteControlRadioOutcome::Saved,
            )),
        );
        assert_eq!(block_on(store.load_radio()).unwrap().state, requested);
        let mut next = Box::pin(
            mailbox
                .handle()
                .execute(RemoteControlHostCommand::InspectRadio { id }),
        );
        assert!(next.as_mut().poll(&mut context).is_pending());
        let (token, _) = block_on(mailbox.receive()).into_parts();
        mailbox.complete(token, Err(RemoteControlHostCommandError::ApplyFailed));
        assert_eq!(
            block_on(next),
            Err(RemoteControlHostCommandError::ApplyFailed)
        );
    }
}
