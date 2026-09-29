use super::super::configuration::{manual_defaults, sealed};
use super::super::{
    AutoLoRaAvailability, FrequencyRange, RegionalDutyCycle, RegionalSubGPolicy, RegionalSubGSpec,
    RegulatoryRegion, SubGRegion, TxPower,
};

pub struct In865;

pub const SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Regulated(RegulatoryRegion::In865),
    "IN865",
    FrequencyRange::from_ordered_hz(865_000_000, 867_000_000),
    manual_defaults(866_000_000, 22),
    TxPower::new(22),
    RegionalDutyCycle::NoModeledLimit,
    AutoLoRaAvailability::Unavailable,
);

impl sealed::RegionalSubGPolicy for In865 {}

impl RegionalSubGPolicy for In865 {
    const SPEC: RegionalSubGSpec = SPEC;
}
