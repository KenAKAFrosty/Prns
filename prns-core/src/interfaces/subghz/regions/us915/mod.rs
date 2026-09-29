use crate::interfaces::lora::{
    CodingRate, LoraBandwidth, Modulation, PreambleSymbols, RadioProfile, SpreadingFactor,
};
use crate::interfaces::subghz::configuration::{manual_defaults, sealed};
use crate::interfaces::subghz::{
    AutoLoRaAvailability, Frequency, FrequencyRange, ManualLoRaParameters, RegionalDutyCycle,
    RegionalSubGPolicy, RegionalSubGSpec, RegulatoryRegion, SubGConfiguration, SubGRegion, TxPower,
};

pub mod frequency_hopping;
pub mod turbo;

pub use frequency_hopping::{
    AntennaGainDeciDb, ChannelOccupancyLimit, ChannelOccupancyLimitError, ConductedPowerDbm,
    ConductedPowerError, FrameDwell, HopSetError, MeasuredTwentyDbBandwidth,
    MeasuredTwentyDbBandwidthError, Us915HopSet, Us915HoppingModel, Us915HoppingModelError,
    Us915PowerBudget, Us915PowerBudgetError, Us915PowerClass, Us915PowerInputs,
};

pub struct Us915;

const US915_AUTO_LORA_PARAMETERS: ManualLoRaParameters = ManualLoRaParameters::new(
    Frequency::new(921_500_000),
    Modulation::Lora {
        spreading_factor: SpreadingFactor::Sf7,
        bandwidth: LoraBandwidth::Bw500kHz,
        coding_rate: CodingRate::Cr45,
    },
    TxPower::new(22),
    PreambleSymbols::new(18),
);

pub const SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Regulated(RegulatoryRegion::Us915),
    "US915",
    FrequencyRange::from_ordered_hz(902_000_000, 928_000_000),
    manual_defaults(921_500_000, 22),
    TxPower::new(22),
    RegionalDutyCycle::NoModeledLimit,
    AutoLoRaAvailability::Available(US915_AUTO_LORA_PARAMETERS),
);

impl sealed::RegionalSubGPolicy for Us915 {}

impl RegionalSubGPolicy for Us915 {
    const SPEC: RegionalSubGSpec = SPEC;
}

impl Us915 {
    pub const fn auto_lora() -> SubGConfiguration {
        SubGConfiguration::auto_lora(US915_AUTO_LORA_PROFILE)
    }
}

pub const US915_AUTO_LORA_PROFILE: RadioProfile = match RadioProfile::new(
    SubGRegion::Regulated(RegulatoryRegion::Us915),
    US915_AUTO_LORA_PARAMETERS.frequency(),
    US915_AUTO_LORA_PARAMETERS.modulation(),
    US915_AUTO_LORA_PARAMETERS.tx_power(),
    US915_AUTO_LORA_PARAMETERS.preamble(),
) {
    Ok(profile) => profile,
    Err(_) => panic!("invalid US915 Auto LoRa profile"),
};
