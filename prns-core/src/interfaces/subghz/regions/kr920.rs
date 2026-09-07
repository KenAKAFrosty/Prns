use super::super::configuration::{manual_defaults, sealed};
use super::super::{
    AutoLoRaAvailability, FrequencyRange, RegionalDutyCycle, RegionalSubGPolicy, RegionalSubGSpec,
    RegulatoryRegion, SubGRegion, TxPower,
};

pub struct Kr920;

pub const SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Regulated(RegulatoryRegion::Kr920),
    "KR920",
    FrequencyRange::from_ordered_hz(920_000_000, 923_000_000),
    manual_defaults(921_500_000, 14),
    TxPower::new(14),
    RegionalDutyCycle::NoModeledLimit,
    AutoLoRaAvailability::Unavailable,
);

impl sealed::RegionalSubGPolicy for Kr920 {}

impl RegionalSubGPolicy for Kr920 {
    const SPEC: RegionalSubGSpec = SPEC;
}
