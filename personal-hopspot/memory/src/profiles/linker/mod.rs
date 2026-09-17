use crate::{
    AddressRange, AddressSpaceGeometry, AddressSpaceId, MemoryProfile, MemoryProfileId,
    MemoryRegionId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkerRangeSource {
    Explicit(&'static [AddressRange]),
    MemoryGeometry,
    FirmwareOwnedRegion,
    Unmapped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkerAddressSpace {
    pub address_space: AddressSpaceId,
    source: LinkerRangeSource,
}

impl LinkerAddressSpace {
    pub(super) const fn explicit(
        address_space: AddressSpaceId,
        ranges: &'static [AddressRange],
    ) -> Self {
        Self {
            address_space,
            source: LinkerRangeSource::Explicit(ranges),
        }
    }

    pub(super) const fn memory_geometry(address_space: AddressSpaceId) -> Self {
        Self {
            address_space,
            source: LinkerRangeSource::MemoryGeometry,
        }
    }

    pub(super) const fn firmware_owned(address_space: AddressSpaceId) -> Self {
        Self {
            address_space,
            source: LinkerRangeSource::FirmwareOwnedRegion,
        }
    }

    pub(super) const fn unmapped(address_space: AddressSpaceId) -> Self {
        Self {
            address_space,
            source: LinkerRangeSource::Unmapped,
        }
    }

    pub fn ranges<'a>(
        &'a self,
        memory: &'a MemoryProfile,
    ) -> Result<impl Iterator<Item = AddressRange> + 'a, LinkerAddressValidationError> {
        let space = memory.address_space(self.address_space).ok_or(
            LinkerAddressValidationError::UnknownAddressSpace {
                address_space: self.address_space,
            },
        )?;
        let ranges = match self.source {
            LinkerRangeSource::Explicit(ranges) => ResolvedRanges::Explicit(ranges.iter()),
            LinkerRangeSource::MemoryGeometry => match space.geometry {
                AddressSpaceGeometry::Fixed(range) => ResolvedRanges::Derived(Some(range)),
                geometry => {
                    return Err(LinkerAddressValidationError::UnresolvedMemoryGeometry {
                        address_space: self.address_space,
                        geometry,
                    });
                }
            },
            LinkerRangeSource::FirmwareOwnedRegion => {
                let region_id = memory.firmware.firmware_owned_region;
                let region = memory.region(region_id).ok_or(
                    LinkerAddressValidationError::MissingFirmwareRegion { region: region_id },
                )?;
                if region.address_space != self.address_space {
                    return Err(
                        LinkerAddressValidationError::FirmwareRegionAddressSpaceMismatch {
                            region: region_id,
                            expected: self.address_space,
                            actual: region.address_space,
                        },
                    );
                }
                ResolvedRanges::Derived(Some(region.range))
            }
            LinkerRangeSource::Unmapped => ResolvedRanges::Derived(None),
        };
        Ok(ranges)
    }
}

enum ResolvedRanges<'a> {
    Explicit(core::slice::Iter<'a, AddressRange>),
    Derived(Option<AddressRange>),
}

impl Iterator for ResolvedRanges<'_> {
    type Item = AddressRange;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Explicit(ranges) => ranges.next().copied(),
            Self::Derived(range) => range.take(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct LinkerAddressProfile {
    pub memory_profile: MemoryProfileId,
    pub address_spaces: &'static [LinkerAddressSpace],
}

impl LinkerAddressProfile {
    pub(super) const fn new(
        memory_profile: MemoryProfileId,
        address_spaces: &'static [LinkerAddressSpace],
    ) -> Self {
        Self {
            memory_profile,
            address_spaces,
        }
    }

    #[must_use]
    pub fn address_space(&self, id: AddressSpaceId) -> Option<&LinkerAddressSpace> {
        self.address_spaces
            .iter()
            .find(|space| space.address_space == id)
    }

    pub fn validate(&self, memory: &MemoryProfile) -> Result<(), LinkerAddressValidationError> {
        if self.memory_profile != memory.id {
            return Err(LinkerAddressValidationError::ProfileMismatch {
                expected: memory.id,
                actual: self.memory_profile,
            });
        }
        for (index, linker_space) in self.address_spaces.iter().enumerate() {
            let space = memory.address_space(linker_space.address_space).ok_or(
                LinkerAddressValidationError::UnknownAddressSpace {
                    address_space: linker_space.address_space,
                },
            )?;
            if self.address_spaces[..index]
                .iter()
                .any(|prior| prior.address_space == linker_space.address_space)
            {
                return Err(LinkerAddressValidationError::DuplicateAddressSpace {
                    address_space: linker_space.address_space,
                });
            }
            for (range_index, range) in linker_space.ranges(memory)?.enumerate() {
                if linker_space
                    .ranges(memory)?
                    .take(range_index)
                    .any(|prior| prior.overlaps(range))
                {
                    return Err(LinkerAddressValidationError::OverlappingRanges {
                        address_space: linker_space.address_space,
                    });
                }
            }
            for other in &self.address_spaces[index + 1..] {
                let other_space = memory.address_space(other.address_space).ok_or(
                    LinkerAddressValidationError::UnknownAddressSpace {
                        address_space: other.address_space,
                    },
                )?;
                let mut overlaps = false;
                for first in linker_space.ranges(memory)? {
                    if other.ranges(memory)?.any(|second| first.overlaps(second)) {
                        overlaps = true;
                        break;
                    }
                }
                let aliased = space.backing_store == other_space.backing_store
                    && space.backing_offset == other_space.backing_offset;
                if overlaps && !aliased {
                    return Err(LinkerAddressValidationError::ConflictingRanges {
                        first: linker_space.address_space,
                        second: other.address_space,
                    });
                }
            }
        }
        if let Some(space) = memory
            .address_spaces
            .iter()
            .find(|space| self.address_space(space.id).is_none())
        {
            return Err(LinkerAddressValidationError::MissingAddressSpace {
                address_space: space.id,
            });
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LinkerAddressValidationError {
    ProfileMismatch {
        expected: MemoryProfileId,
        actual: MemoryProfileId,
    },
    MissingAddressSpace {
        address_space: AddressSpaceId,
    },
    UnknownAddressSpace {
        address_space: AddressSpaceId,
    },
    DuplicateAddressSpace {
        address_space: AddressSpaceId,
    },
    OverlappingRanges {
        address_space: AddressSpaceId,
    },
    ConflictingRanges {
        first: AddressSpaceId,
        second: AddressSpaceId,
    },
    UnresolvedMemoryGeometry {
        address_space: AddressSpaceId,
        geometry: AddressSpaceGeometry,
    },
    MissingFirmwareRegion {
        region: MemoryRegionId,
    },
    FirmwareRegionAddressSpaceMismatch {
        region: MemoryRegionId,
        expected: AddressSpaceId,
        actual: AddressSpaceId,
    },
}

fn linker_address_profiles() -> impl Iterator<Item = &'static LinkerAddressProfile> {
    super::espressif::linker::LINKER_ADDRESS_PROFILES
        .iter()
        .chain(super::nrf52840::linker::LINKER_ADDRESS_PROFILES.iter())
        .copied()
}

#[must_use]
pub fn linker_address_profile(id: MemoryProfileId) -> Option<&'static LinkerAddressProfile> {
    linker_address_profiles().find(|profile| profile.memory_profile == id)
}

#[cfg(test)]
mod tests;
