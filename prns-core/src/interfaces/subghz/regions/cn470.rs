use super::super::configuration::{manual_defaults, sealed};
use super::super::{
    AutoLoRaAvailability, FrequencyRange, RegionalDutyCycle, RegionalSubGPolicy, RegionalSubGSpec,
    RegulatoryRegion, SubGRegion, TxPower,
};

pub struct Cn470;

pub const SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Regulated(RegulatoryRegion::Cn470),
    "CN470",
    FrequencyRange::from_ordered_hz(470_000_000, 510_000_000),
    manual_defaults(490_000_000, 19),
    TxPower::new(19),
    RegionalDutyCycle::NoModeledLimit,
    AutoLoRaAvailability::Unavailable,
);

impl sealed::RegionalSubGPolicy for Cn470 {}

impl RegionalSubGPolicy for Cn470 {
    const SPEC: RegionalSubGSpec = SPEC;
}
