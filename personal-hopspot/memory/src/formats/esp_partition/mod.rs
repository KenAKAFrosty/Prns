use core::fmt;

use crate::{
    AddressSpaceKind, MemoryProfile, MemoryProfileId, MemoryRegionId, RegionRole, ValidationError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EspPartitionKind {
    FactoryApplication,
    NvsData,
    PhyData,
    Custom { partition_type: u8, subtype: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EspPartitionBinding {
    pub region: MemoryRegionId,
    pub name: &'static str,
    pub kind: EspPartitionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EspPartitionTable {
    pub profiles: &'static [MemoryProfileId],
    pub partitions: &'static [EspPartitionBinding],
}

impl EspPartitionTable {
    #[must_use]
    pub fn supports(&self, profile: MemoryProfileId) -> bool {
        self.profiles.contains(&profile)
    }

    pub fn validate(&self, profile: &MemoryProfile) -> Result<(), EspPartitionTableError> {
        profile
            .validate()
            .map_err(EspPartitionTableError::InvalidMemoryProfile)?;
        if !self.supports(profile.id) {
            return Err(EspPartitionTableError::UnsupportedProfile {
                profile: profile.id,
            });
        }
        for (index, profile_id) in self.profiles.iter().enumerate() {
            if self.profiles[..index].contains(profile_id) {
                return Err(EspPartitionTableError::DuplicateProfile {
                    profile: *profile_id,
                });
            }
        }
        let mut previous_region: Option<&crate::MemoryRegion> = None;
        for (index, partition) in self.partitions.iter().enumerate() {
            if partition.name.is_empty() {
                return Err(EspPartitionTableError::EmptyPartitionName {
                    region: partition.region,
                });
            }
            if let Some(prior) = self.partitions[..index]
                .iter()
                .find(|prior| prior.region == partition.region)
            {
                return Err(EspPartitionTableError::DuplicateRegion {
                    first: prior.region,
                    second: partition.region,
                });
            }
            if let Some(prior) = self.partitions[..index]
                .iter()
                .find(|prior| prior.name == partition.name)
            {
                return Err(EspPartitionTableError::DuplicateName {
                    first: prior.region,
                    second: partition.region,
                    name: partition.name,
                });
            }
            let Some(region) = profile.region(partition.region) else {
                return Err(EspPartitionTableError::UnknownRegion {
                    region: partition.region,
                });
            };
            let Some(address_space) = profile.address_space(region.address_space) else {
                return Err(EspPartitionTableError::InvalidMemoryProfile(
                    ValidationError::UnknownRegionAddressSpace {
                        region: region.id,
                        address_space: region.address_space,
                    },
                ));
            };
            if address_space.kind != AddressSpaceKind::InternalFlash {
                return Err(EspPartitionTableError::RegionOutsideInternalFlash {
                    region: region.id,
                });
            }
            if !partition_kind_matches_region(partition.kind, region.role) {
                return Err(EspPartitionTableError::PartitionKindMismatch {
                    region: region.id,
                    kind: partition.kind,
                });
            }
            if let Some(previous) = previous_region {
                if region.range.start() < previous.range.end() {
                    return Err(EspPartitionTableError::PartitionsOutOfOrder {
                        previous: previous.id,
                        current: region.id,
                    });
                }
            }
            previous_region = Some(region);
        }
        for region in profile.regions {
            if region_requires_partition(region.role)
                && !self
                    .partitions
                    .iter()
                    .any(|partition| partition.region == region.id)
            {
                return Err(EspPartitionTableError::MissingRegion { region: region.id });
            }
        }
        Ok(())
    }
}

const fn partition_kind_matches_region(kind: EspPartitionKind, role: RegionRole) -> bool {
    match kind {
        EspPartitionKind::FactoryApplication => matches!(role, RegionRole::FirmwareImage),
        EspPartitionKind::NvsData => matches!(role, RegionRole::PlatformData),
        EspPartitionKind::PhyData => matches!(role, RegionRole::PhyInitialization),
        EspPartitionKind::Custom { .. } => matches!(
            role,
            RegionRole::BleIdentity
                | RegionRole::Provisioning
                | RegionRole::NodeIdentity
                | RegionRole::RemoteControlIdentity
                | RegionRole::RadioProfile
                | RegionRole::Journal
        ),
    }
}

const fn region_requires_partition(role: RegionRole) -> bool {
    !matches!(
        role,
        RegionRole::Bootloader
            | RegionRole::PartitionTable
            | RegionRole::SoftDevice
            | RegionRole::RecoveryBootloader
            | RegionRole::FactoryReserved
            | RegionRole::Reserved
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EspPartitionTableError {
    InvalidMemoryProfile(ValidationError),
    UnsupportedProfile {
        profile: MemoryProfileId,
    },
    DuplicateProfile {
        profile: MemoryProfileId,
    },
    EmptyPartitionName {
        region: MemoryRegionId,
    },
    DuplicateRegion {
        first: MemoryRegionId,
        second: MemoryRegionId,
    },
    DuplicateName {
        first: MemoryRegionId,
        second: MemoryRegionId,
        name: &'static str,
    },
    UnknownRegion {
        region: MemoryRegionId,
    },
    RegionOutsideInternalFlash {
        region: MemoryRegionId,
    },
    PartitionKindMismatch {
        region: MemoryRegionId,
        kind: EspPartitionKind,
    },
    PartitionsOutOfOrder {
        previous: MemoryRegionId,
        current: MemoryRegionId,
    },
    MissingRegion {
        region: MemoryRegionId,
    },
}

impl fmt::Display for EspPartitionTableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid ESP partition table: {self:?}")
    }
}

#[cfg(test)]
mod tests;
