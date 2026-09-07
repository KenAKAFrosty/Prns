use super::super::configuration::{manual_defaults, sealed, ten_percent_duty_cycle};
use super::super::{
    AutoLoRaAvailability, FrequencyRange, RegionalSubGPolicy, RegionalSubGSpec, RegulatoryRegion,
    SubGRegion, TxPower,
};

pub struct Eu869;

pub const SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Regulated(RegulatoryRegion::Eu869),
    "EU869",
    FrequencyRange::from_ordered_hz(869_400_000, 869_650_000),
    manual_defaults(869_525_000, 22),
    TxPower::new(22),
    ten_percent_duty_cycle(),
    AutoLoRaAvailability::Unavailable,
);

impl sealed::RegionalSubGPolicy for Eu869 {}

impl RegionalSubGPolicy for Eu869 {
    const SPEC: RegionalSubGSpec = SPEC;
}
