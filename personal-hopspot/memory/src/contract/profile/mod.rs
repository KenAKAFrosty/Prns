mod accounting;
mod validation;

use core::fmt;

use super::{
    AddressSpace, AddressSpaceId, FirmwarePlacement, JournalLayout, MemoryRegion, MemoryRegionId,
    ProcessorArchitecture, RegionRole, RuntimeReservation,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionRoleLookupError {
    Missing {
        role: RegionRole,
    },
    Ambiguous {
        role: RegionRole,
        first: MemoryRegionId,
        second: MemoryRegionId,
    },
}

impl fmt::Display for RegionRoleLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid memory-region role lookup: {self:?}")
    }
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

    pub const fn unique_region_for_role(
        &self,
        role: RegionRole,
    ) -> Result<&MemoryRegion, RegionRoleLookupError> {
        let mut matching_index = self.regions.len();
        let mut index = 0;
        while index < self.regions.len() {
            if self.regions[index].role.same_as(role) {
                if matching_index < self.regions.len() {
                    return Err(RegionRoleLookupError::Ambiguous {
                        role,
                        first: self.regions[matching_index].id,
                        second: self.regions[index].id,
                    });
                }
                matching_index = index;
            }
            index += 1;
        }
        if matching_index == self.regions.len() {
            Err(RegionRoleLookupError::Missing { role })
        } else {
            Ok(&self.regions[matching_index])
        }
    }
}

#[cfg(test)]
mod tests;
