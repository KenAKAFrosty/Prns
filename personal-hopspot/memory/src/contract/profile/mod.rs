mod accounting;
mod validation;

use core::fmt;

use super::{
    AddressSpace, AddressSpaceId, AddressSpaceKind, FirmwarePlacement, JournalLayout, MemoryRegion,
    MemoryRegionId, ProcessorArchitecture, RegionRole, RuntimeReservation,
};

pub use validation::ValidationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryProfileId(pub &'static str);

impl MemoryProfileId {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for MemoryProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSpaceKindLookupError {
    Missing {
        kind: AddressSpaceKind,
    },
    Ambiguous {
        kind: AddressSpaceKind,
        first: AddressSpaceId,
        second: AddressSpaceId,
    },
}

impl fmt::Display for AddressSpaceKindLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid address-space kind lookup: {self:?}")
    }
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

    pub const fn unique_address_space_for_kind(
        &self,
        kind: AddressSpaceKind,
    ) -> Result<&AddressSpace, AddressSpaceKindLookupError> {
        let mut matching_index = self.address_spaces.len();
        let mut index = 0;
        while index < self.address_spaces.len() {
            if self.address_spaces[index].kind.same_as(kind) {
                if matching_index < self.address_spaces.len() {
                    return Err(AddressSpaceKindLookupError::Ambiguous {
                        kind,
                        first: self.address_spaces[matching_index].id,
                        second: self.address_spaces[index].id,
                    });
                }
                matching_index = index;
            }
            index += 1;
        }
        if matching_index == self.address_spaces.len() {
            Err(AddressSpaceKindLookupError::Missing { kind })
        } else {
            Ok(&self.address_spaces[matching_index])
        }
    }
}

#[cfg(test)]
mod tests;
