pub mod as923;
pub mod au915;
pub mod cn470;
pub mod eu433;
pub mod eu865;
pub mod eu868;
pub mod eu869;
pub mod in865;
pub mod jp920;
pub mod kr920;
pub mod us915;

use super::{RegionalSubGSpec, RegulatoryRegion};

pub(crate) const fn specification(region: RegulatoryRegion) -> RegionalSubGSpec {
    match region {
        RegulatoryRegion::Us915 => us915::SPEC,
        RegulatoryRegion::Au915 => au915::SPEC,
        RegulatoryRegion::Eu433 => eu433::SPEC,
        RegulatoryRegion::Eu865 => eu865::SPEC,
        RegulatoryRegion::Eu868 => eu868::SPEC,
        RegulatoryRegion::Eu869 => eu869::SPEC,
        RegulatoryRegion::As923 => as923::SPEC,
        RegulatoryRegion::In865 => in865::SPEC,
        RegulatoryRegion::Cn470 => cn470::SPEC,
        RegulatoryRegion::Kr920 => kr920::SPEC,
        RegulatoryRegion::Jp920 => jp920::SPEC,
    }
}
