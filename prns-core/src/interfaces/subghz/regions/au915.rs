use super::super::configuration::{manual_defaults, sealed};
use super::super::{
    AutoLoRaAvailability, FrequencyRange, RegionalDutyCycle, RegionalSubGPolicy, RegionalSubGSpec,
    RegulatoryRegion, SubGRegion, TxPower,
};

pub struct Au915;

pub const SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Regulated(RegulatoryRegion::Au915),
    "AU915",
    FrequencyRange::from_ordered_hz(915_000_000, 928_000_000),
    manual_defaults(921_500_000, 22),
    TxPower::new(22),
    RegionalDutyCycle::NoModeledLimit,
    AutoLoRaAvailability::Unavailable,
);

impl sealed::RegionalSubGPolicy for Au915 {}

impl RegionalSubGPolicy for Au915 {
    const SPEC: RegionalSubGSpec = SPEC;
}
