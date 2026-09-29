use super::super::configuration::{manual_defaults, sealed};
use super::super::{
    AutoLoRaAvailability, FrequencyRange, RegionalDutyCycle, RegionalSubGPolicy, RegionalSubGSpec,
    RegulatoryRegion, SubGRegion, TxPower,
};

pub struct As923;

pub const SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Regulated(RegulatoryRegion::As923),
    "AS923",
    FrequencyRange::from_ordered_hz(920_000_000, 925_000_000),
    manual_defaults(922_500_000, 16),
    TxPower::new(16),
    RegionalDutyCycle::NoModeledLimit,
    AutoLoRaAvailability::Unavailable,
);

impl sealed::RegionalSubGPolicy for As923 {}

impl RegionalSubGPolicy for As923 {
    const SPEC: RegionalSubGSpec = SPEC;
}
