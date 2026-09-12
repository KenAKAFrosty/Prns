use crate::{
    AddressRange, AddressSpaceGeometry, AddressSpaceId, MemoryProfile, MemoryProfileId,
    MemoryRegionId,
};

use super::{
    HELTEC_E290, HELTEC_V4, HELTEC_V4_R8, HELTEC_WIRELESS_STICK_LITE_V3, MESH_POCKET_10000,
    MESH_POCKET_5000, MESH_TOWER_V2, T096, T1000_E, T114, T_BEAM_SUPREME, T_ECHO_S140_V6,
    T_ECHO_S140_V7, XIAO_ESP32_C6,
};

const FLASH: AddressSpaceId = AddressSpaceId("internal-flash");
const RAM: AddressSpaceId = AddressSpaceId("internal-ram");
const IRAM: AddressSpaceId = AddressSpaceId("instruction-ram");
const DRAM: AddressSpaceId = AddressSpaceId("data-ram");
const RECLAIMED_RAM: AddressSpaceId = AddressSpaceId("reclaimed-ram");
const DCACHE_RAM: AddressSpaceId = AddressSpaceId("dcache-ram");
const RTC_FAST_RAM: AddressSpaceId = AddressSpaceId("fast-retention-ram");
const RTC_SLOW_RAM: AddressSpaceId = AddressSpaceId("slow-retention-ram");
const PSRAM: AddressSpaceId = AddressSpaceId("external-psram");

const ESP32S3_FLASH: [AddressRange; 2] = [
    AddressRange::new(0x3C00_0000, 0x3E00_0000),
    AddressRange::new(0x4200_0000, 0x4400_0000),
];
const ESP32S3_IRAM: [AddressRange; 1] = [AddressRange::new(0x4037_0000, 0x4040_0000)];
const ESP32S3_DRAM: [AddressRange; 1] = [AddressRange::new(0x3FC0_0000, 0x4000_0000)];
const ESP32C6_FLASH: [AddressRange; 1] = [AddressRange::new(0x4200_0000, 0x4240_0000)];
const ESP32C6_RAM: [AddressRange; 1] = [AddressRange::new(0x4080_0000, 0x4090_0000)];

const ESP32S3_SPACES: [LinkerAddressSpace; 8] = [
    LinkerAddressSpace::explicit(FLASH, &ESP32S3_FLASH),
    LinkerAddressSpace::explicit(IRAM, &ESP32S3_IRAM),
    LinkerAddressSpace::explicit(DRAM, &ESP32S3_DRAM),
    LinkerAddressSpace::unmapped(RECLAIMED_RAM),
    LinkerAddressSpace::unmapped(DCACHE_RAM),
    LinkerAddressSpace::memory_geometry(RTC_FAST_RAM),
    LinkerAddressSpace::memory_geometry(RTC_SLOW_RAM),
    LinkerAddressSpace::unmapped(PSRAM),
];
const ESP32S3_NO_PSRAM_SPACES: [LinkerAddressSpace; 7] = [
    ESP32S3_SPACES[0],
    ESP32S3_SPACES[1],
    ESP32S3_SPACES[2],
    ESP32S3_SPACES[3],
    ESP32S3_SPACES[4],
    ESP32S3_SPACES[5],
    ESP32S3_SPACES[6],
];
const ESP32C6_SPACES: [LinkerAddressSpace; 3] = [
    LinkerAddressSpace::explicit(FLASH, &ESP32C6_FLASH),
    LinkerAddressSpace::explicit(IRAM, &ESP32C6_RAM),
    LinkerAddressSpace::explicit(DRAM, &ESP32C6_RAM),
];
const NRF52840_SPACES: [LinkerAddressSpace; 2] = [
    LinkerAddressSpace::firmware_owned(FLASH),
    LinkerAddressSpace::memory_geometry(RAM),
];

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
    const fn explicit(address_space: AddressSpaceId, ranges: &'static [AddressRange]) -> Self {
        Self {
            address_space,
            source: LinkerRangeSource::Explicit(ranges),
        }
    }

    const fn memory_geometry(address_space: AddressSpaceId) -> Self {
        Self {
            address_space,
            source: LinkerRangeSource::MemoryGeometry,
        }
    }

    const fn firmware_owned(address_space: AddressSpaceId) -> Self {
        Self {
            address_space,
            source: LinkerRangeSource::FirmwareOwnedRegion,
        }
    }

    const fn unmapped(address_space: AddressSpaceId) -> Self {
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

const fn profile(
    memory_profile: MemoryProfileId,
    address_spaces: &'static [LinkerAddressSpace],
) -> LinkerAddressProfile {
    LinkerAddressProfile {
        memory_profile,
        address_spaces,
    }
}

const HELTEC_V4_LINKER: LinkerAddressProfile = profile(HELTEC_V4.id, &ESP32S3_SPACES);
const HELTEC_V4_R8_LINKER: LinkerAddressProfile = profile(HELTEC_V4_R8.id, &ESP32S3_SPACES);
const HELTEC_E290_LINKER: LinkerAddressProfile = profile(HELTEC_E290.id, &ESP32S3_SPACES);
const T_BEAM_SUPREME_LINKER: LinkerAddressProfile = profile(T_BEAM_SUPREME.id, &ESP32S3_SPACES);
const HELTEC_WIRELESS_STICK_LITE_V3_LINKER: LinkerAddressProfile =
    profile(HELTEC_WIRELESS_STICK_LITE_V3.id, &ESP32S3_NO_PSRAM_SPACES);
const XIAO_ESP32_C6_LINKER: LinkerAddressProfile = profile(XIAO_ESP32_C6.id, &ESP32C6_SPACES);
const T_ECHO_S140_V6_LINKER: LinkerAddressProfile = profile(T_ECHO_S140_V6.id, &NRF52840_SPACES);
const T_ECHO_S140_V7_LINKER: LinkerAddressProfile = profile(T_ECHO_S140_V7.id, &NRF52840_SPACES);
const T096_LINKER: LinkerAddressProfile = profile(T096.id, &NRF52840_SPACES);
const T114_LINKER: LinkerAddressProfile = profile(T114.id, &NRF52840_SPACES);
const MESH_POCKET_5000_LINKER: LinkerAddressProfile =
    profile(MESH_POCKET_5000.id, &NRF52840_SPACES);
const MESH_POCKET_10000_LINKER: LinkerAddressProfile =
    profile(MESH_POCKET_10000.id, &NRF52840_SPACES);
const T1000_E_LINKER: LinkerAddressProfile = profile(T1000_E.id, &NRF52840_SPACES);
const MESH_TOWER_V2_LINKER: LinkerAddressProfile = profile(MESH_TOWER_V2.id, &NRF52840_SPACES);

pub const ALL_LINKER_ADDRESS_PROFILES: [&LinkerAddressProfile; 14] = [
    &HELTEC_V4_LINKER,
    &HELTEC_V4_R8_LINKER,
    &HELTEC_E290_LINKER,
    &HELTEC_WIRELESS_STICK_LITE_V3_LINKER,
    &T_BEAM_SUPREME_LINKER,
    &XIAO_ESP32_C6_LINKER,
    &T_ECHO_S140_V6_LINKER,
    &T_ECHO_S140_V7_LINKER,
    &T096_LINKER,
    &T114_LINKER,
    &MESH_POCKET_5000_LINKER,
    &MESH_POCKET_10000_LINKER,
    &T1000_E_LINKER,
    &MESH_TOWER_V2_LINKER,
];

#[must_use]
pub fn linker_address_profile(id: MemoryProfileId) -> Option<&'static LinkerAddressProfile> {
    ALL_LINKER_ADDRESS_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.memory_profile == id)
}

#[cfg(test)]
mod tests;
