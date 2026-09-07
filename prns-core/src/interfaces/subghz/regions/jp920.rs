use super::super::configuration::{manual_defaults, sealed};
use super::super::{
    AutoLoRaAvailability, FrequencyRange, RegionalDutyCycle, RegionalSubGPolicy, RegionalSubGSpec,
    RegulatoryRegion, SubGRegion, TxPower,
};

pub struct Jp920;

pub const SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Regulated(RegulatoryRegion::Jp920),
    "JP920",
    FrequencyRange::from_ordered_hz(920_800_000, 927_800_000),
    manual_defaults(922_000_000, 16),
    TxPower::new(16),
    RegionalDutyCycle::NoModeledLimit,
    AutoLoRaAvailability::Unavailable,
);

impl sealed::RegionalSubGPolicy for Jp920 {}

impl RegionalSubGPolicy for Jp920 {
    const SPEC: RegionalSubGSpec = SPEC;
}
