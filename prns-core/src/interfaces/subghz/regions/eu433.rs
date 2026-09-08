use super::super::configuration::{manual_defaults, sealed, ten_percent_duty_cycle};
use super::super::{
    AutoLoRaAvailability, FrequencyRange, RegionalSubGPolicy, RegionalSubGSpec, RegulatoryRegion,
    SubGRegion, TxPower,
};

pub struct Eu433;

pub const SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Regulated(RegulatoryRegion::Eu433),
    "EU433",
    FrequencyRange::from_ordered_hz(433_050_000, 434_790_000),
    manual_defaults(433_900_000, 12),
    TxPower::new(12),
    ten_percent_duty_cycle(),
    AutoLoRaAvailability::Unavailable,
);

impl sealed::RegionalSubGPolicy for Eu433 {}

impl RegionalSubGPolicy for Eu433 {
    const SPEC: RegionalSubGSpec = SPEC;
}
