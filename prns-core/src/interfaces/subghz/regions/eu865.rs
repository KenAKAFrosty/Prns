use super::super::configuration::{manual_defaults, one_percent_duty_cycle, sealed};
use super::super::{
    AutoLoRaAvailability, FrequencyRange, RegionalSubGPolicy, RegionalSubGSpec, RegulatoryRegion,
    SubGRegion, TxPower,
};

pub struct Eu865;

pub const SPEC: RegionalSubGSpec = RegionalSubGSpec::new(
    SubGRegion::Regulated(RegulatoryRegion::Eu865),
    "EU865",
    FrequencyRange::from_ordered_hz(865_000_000, 868_000_000),
    manual_defaults(866_500_000, 14),
    TxPower::new(14),
    one_percent_duty_cycle(),
    AutoLoRaAvailability::Unavailable,
);

impl sealed::RegionalSubGPolicy for Eu865 {}

impl RegionalSubGPolicy for Eu865 {
    const SPEC: RegionalSubGSpec = SPEC;
}
