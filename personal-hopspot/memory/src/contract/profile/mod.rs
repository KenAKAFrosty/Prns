mod accounting;
mod validation;

use super::{
    AddressSpace, AddressSpaceId, FirmwarePlacement, JournalLayout, MemoryRegion, MemoryRegionId,
    ProcessorArchitecture, RuntimeReservation,
};

pub use validation::ValidationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryProfileId(pub &'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryProfile {
    pub id: MemoryProfileId,
    pub architecture: ProcessorArchitecture,
    pub address_spaces: &'static [AddressSpace],
    pub regions: &'static [MemoryRegion],
    pub firmware: FirmwarePlacement,
    pub journals: &'static [JournalLayout],
    pub runtime_reservations: &'static [RuntimeReservation],
}

impl MemoryProfile {
    #[must_use]
    pub fn address_space(&self, id: AddressSpaceId) -> Option<&AddressSpace> {
        self.address_spaces.iter().find(|space| space.id == id)
    }

    #[must_use]
    pub fn region(&self, id: MemoryRegionId) -> Option<&MemoryRegion> {
        self.regions.iter().find(|region| region.id == id)
    }
}

#[cfg(test)]
mod tests;
