use super::super::configuration::{manual_defaults, one_percent_duty_cycle, sealed};
use super::super::{
    AutoLoRaAvailability, FrequencyRange, RegionalSubGPolicy, RegionalSubGSpec, RegulatoryRegion,
    SubGRegion, TxPower,
};

pub struct Eu868;

pub const SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Regulated(RegulatoryRegion::Eu868),
    "EU868",
    FrequencyRange::from_ordered_hz(868_000_000, 868_600_000),
    manual_defaults(868_300_000, 14),
    TxPower::new(14),
    one_percent_duty_cycle(),
    AutoLoRaAvailability::Unavailable,
);

impl sealed::RegionalSubGPolicy for Eu868 {}

impl RegionalSubGPolicy for Eu868 {
    const SPEC: RegionalSubGSpec = SPEC;
}
