use personal_hopspot_memory::{
    memory_profile_named, AddressRange, AddressSpaceGeometry, AddressSpaceKind,
    AddressSpaceKindLookupError, MemoryProfileId, ProcessorArchitecture, ValidationError,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{EspBuild, NrfSerialDfuBuild, NrfSerialDfuCompatibility, Uf2BuildVariant};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryProfileReference(String);

impl MemoryProfileReference {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn resolve(&self) -> Result<ResolvedMemoryProfile, MemoryProfileReferenceError> {
        let profile = memory_profile_named(&self.0).ok_or_else(|| {
            MemoryProfileReferenceError::UnknownProfile {
                profile: self.0.clone(),
            }
        })?;
        profile
            .validate()
            .map_err(|error| MemoryProfileReferenceError::InvalidProfile {
                profile: profile.id,
                error,
            })?;
        let firmware = profile
            .region(profile.firmware.firmware_owned_region)
            .ok_or(MemoryProfileReferenceError::MissingFirmwareRegion {
                profile: profile.id,
            })?;
        let internal_flash = profile
            .unique_address_space_for_kind(AddressSpaceKind::InternalFlash)
            .map_err(|error| MemoryProfileReferenceError::InvalidInternalFlash {
                profile: profile.id,
                error,
            })?;
        let flash_geometry = internal_flash.geometry;
        let AddressSpaceGeometry::Fixed(flash_range) = flash_geometry else {
            return Err(MemoryProfileReferenceError::UnboundedInternalFlash {
                profile: profile.id,
                address_space: internal_flash.id,
            });
        };
        if flash_range.start() != 0 {
            return Err(MemoryProfileReferenceError::NonzeroInternalFlashOrigin {
                profile: profile.id,
                address_space: internal_flash.id,
                origin: flash_range.start(),
            });
        }
        let flash_capacity = u32::try_from(flash_range.byte_len()).map_err(|_| {
            MemoryProfileReferenceError::AddressExceedsU32 {
                profile: profile.id,
                address: flash_range.byte_len(),
            }
        })?;
        Ok(ResolvedMemoryProfile {
            id: profile.id,
            architecture: profile.architecture,
            internal_flash_capacity: flash_capacity,
            firmware_owned: ApplicationAddressRange::try_from_memory(profile.id, firmware.range)?,
            transport_envelope: ApplicationAddressRange::try_from_memory(
                profile.id,
                profile.firmware.transport_envelope.range,
            )?,
        })
    }
}

impl EspBuild {
    pub fn memory_layout(&self) -> Result<ResolvedMemoryProfile, MemoryProfileReferenceError> {
        self.memory_profile.resolve()
    }
}

impl Uf2BuildVariant {
    pub fn memory_layout(&self) -> Result<ResolvedMemoryProfile, MemoryProfileReferenceError> {
        self.memory_profile.resolve()
    }
}

impl NrfSerialDfuBuild {
    pub fn memory_layout(&self) -> Result<ResolvedMemoryProfile, MemoryProfileReferenceError> {
        self.memory_profile.resolve()
    }

    pub fn manifest_compatibility(
        &self,
    ) -> Result<NrfSerialDfuCompatibility, MemoryProfileReferenceError> {
        let application = self.memory_layout()?.transport_envelope();
        Ok(NrfSerialDfuCompatibility {
            softdevice_family: self.compatibility.softdevice_family.clone(),
            softdevice_version: self.compatibility.softdevice_version.clone(),
            fwid: self.compatibility.fwid.clone(),
            device_type: self.compatibility.device_type.clone(),
            device_revision: self.compatibility.device_revision,
            application_version: self.compatibility.application_version,
            application_base: format!("0x{:08x}", application.start()),
            application_end_exclusive: format!("0x{:08x}", application.end_exclusive()),
            bank_layout: self.compatibility.bank_layout,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedMemoryProfile {
    id: MemoryProfileId,
    architecture: ProcessorArchitecture,
    internal_flash_capacity: u32,
    firmware_owned: ApplicationAddressRange,
    transport_envelope: ApplicationAddressRange,
}

impl ResolvedMemoryProfile {
    #[must_use]
    pub const fn id(self) -> MemoryProfileId {
        self.id
    }

    #[must_use]
    pub const fn architecture(self) -> ProcessorArchitecture {
        self.architecture
    }

    #[must_use]
    pub const fn internal_flash_capacity(self) -> u32 {
        self.internal_flash_capacity
    }

    #[must_use]
    pub const fn firmware_owned(self) -> ApplicationAddressRange {
        self.firmware_owned
    }

    #[must_use]
    pub const fn transport_envelope(self) -> ApplicationAddressRange {
        self.transport_envelope
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplicationAddressRange {
    start: u32,
    end_exclusive: u32,
}

impl ApplicationAddressRange {
    fn try_from_memory(
        profile: MemoryProfileId,
        range: AddressRange,
    ) -> Result<Self, MemoryProfileReferenceError> {
        let start = u32::try_from(range.start()).map_err(|_| {
            MemoryProfileReferenceError::AddressExceedsU32 {
                profile,
                address: range.start(),
            }
        })?;
        let end_exclusive = u32::try_from(range.end()).map_err(|_| {
            MemoryProfileReferenceError::AddressExceedsU32 {
                profile,
                address: range.end(),
            }
        })?;
        Ok(Self {
            start,
            end_exclusive,
        })
    }

    #[must_use]
    pub const fn start(self) -> u32 {
        self.start
    }

    #[must_use]
    pub const fn end_exclusive(self) -> u32 {
        self.end_exclusive
    }

    #[must_use]
    pub const fn byte_len(self) -> u32 {
        self.end_exclusive - self.start
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MemoryProfileReferenceError {
    #[error("unknown embedded memory profile {profile:?}")]
    UnknownProfile { profile: String },
    #[error("embedded memory profile {profile} is invalid: {error:?}")]
    InvalidProfile {
        profile: MemoryProfileId,
        error: ValidationError,
    },
    #[error("embedded memory profile {profile} has no firmware-owned region")]
    MissingFirmwareRegion { profile: MemoryProfileId },
    #[error("embedded memory profile {profile} has invalid internal flash: {error}")]
    InvalidInternalFlash {
        profile: MemoryProfileId,
        error: AddressSpaceKindLookupError,
    },
    #[error("embedded memory profile {profile} internal flash {address_space:?} is not fixed")]
    UnboundedInternalFlash {
        profile: MemoryProfileId,
        address_space: personal_hopspot_memory::AddressSpaceId,
    },
    #[error(
        "embedded memory profile {profile} internal flash {address_space:?} starts at 0x{origin:x}"
    )]
    NonzeroInternalFlashOrigin {
        profile: MemoryProfileId,
        address_space: personal_hopspot_memory::AddressSpaceId,
        origin: u64,
    },
    #[error("embedded memory profile {profile} address 0x{address:x} exceeds u32")]
    AddressExceedsU32 {
        profile: MemoryProfileId,
        address: u64,
    },
}
