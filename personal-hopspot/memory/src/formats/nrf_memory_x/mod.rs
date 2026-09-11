use core::fmt;

use crate::{
    AddressRange, AddressSpaceGeometry, AddressSpaceId, AddressSpaceKind, MemoryProfile,
    MemoryProfileId, ProcessorArchitecture, ReservationAccounting, ReservationCharge,
    ReservationId, ValidationError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NrfMemoryXBinding {
    pub profiles: &'static [MemoryProfileId],
    pub application_ram: AddressSpaceId,
    pub minimum_runtime_stack: ReservationId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NrfMemoryXLayout {
    pub application_flash: AddressRange,
    pub application_ram: AddressRange,
    pub minimum_runtime_stack_bytes: u64,
}

impl fmt::Display for NrfMemoryXLayout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let application_flash_origin = self.application_flash.start();
        let application_flash_bytes = self.application_flash.byte_len();
        let application_ram_origin = self.application_ram.start();
        let application_ram_bytes = self.application_ram.byte_len();
        let minimum_runtime_stack_bytes = self.minimum_runtime_stack_bytes;
        write!(
            formatter,
            "APPLICATION_FLASH_ORIGIN = {application_flash_origin:#010X};\n\
APPLICATION_FLASH_BYTES = {application_flash_bytes:#X};\n\
APPLICATION_RAM_ORIGIN = {application_ram_origin:#010X};\n\
APPLICATION_RAM_BYTES = {application_ram_bytes:#X};\n\n\
MEMORY\n\
{{\n\
  FLASH : ORIGIN = APPLICATION_FLASH_ORIGIN, LENGTH = APPLICATION_FLASH_BYTES\n\
  RAM   : ORIGIN = APPLICATION_RAM_ORIGIN, LENGTH = APPLICATION_RAM_BYTES\n\
}}\n\n\
ASSERT(\n\
  ORIGIN(RAM) + LENGTH(RAM) - _stack_end >= {minimum_runtime_stack_bytes},\n\
  \"nRF52840 static memory leaves too little runtime stack\"\n\
)\n"
        )
    }
}

impl NrfMemoryXBinding {
    #[must_use]
    pub fn supports(&self, profile: MemoryProfileId) -> bool {
        self.profiles.contains(&profile)
    }

    pub fn resolve(&self, profile: &MemoryProfile) -> Result<NrfMemoryXLayout, NrfMemoryXError> {
        profile
            .validate()
            .map_err(NrfMemoryXError::InvalidMemoryProfile)?;
        if !self.supports(profile.id) {
            return Err(NrfMemoryXError::UnsupportedProfile {
                profile: profile.id,
            });
        }
        for (index, profile_id) in self.profiles.iter().enumerate() {
            if self.profiles[..index].contains(profile_id) {
                return Err(NrfMemoryXError::DuplicateProfile {
                    profile: *profile_id,
                });
            }
        }
        if profile.architecture != ProcessorArchitecture::ThumbV7em {
            return Err(NrfMemoryXError::UnsupportedArchitecture {
                profile: profile.id,
                architecture: profile.architecture,
            });
        }
        let Some(firmware) = profile.region(profile.firmware.firmware_owned_region) else {
            return Err(NrfMemoryXError::InvalidMemoryProfile(
                ValidationError::UnknownFirmwareRegion {
                    region: profile.firmware.firmware_owned_region,
                },
            ));
        };
        let Some(flash) = profile.address_space(firmware.address_space) else {
            return Err(NrfMemoryXError::InvalidMemoryProfile(
                ValidationError::UnknownFirmwareAddressSpace {
                    address_space: firmware.address_space,
                },
            ));
        };
        if flash.kind != AddressSpaceKind::InternalFlash {
            return Err(NrfMemoryXError::FirmwareOutsideInternalFlash {
                address_space: flash.id,
            });
        }
        let Some(ram) = profile.address_space(self.application_ram) else {
            return Err(NrfMemoryXError::MissingApplicationRam {
                address_space: self.application_ram,
            });
        };
        if ram.kind != AddressSpaceKind::InternalRam {
            return Err(NrfMemoryXError::InvalidApplicationRam {
                address_space: ram.id,
            });
        }
        let AddressSpaceGeometry::Fixed(application_ram) = ram.geometry else {
            return Err(NrfMemoryXError::UnboundedApplicationRam {
                address_space: ram.id,
            });
        };
        let Some(stack) = profile
            .runtime_reservations
            .iter()
            .find(|reservation| reservation.id == self.minimum_runtime_stack)
        else {
            return Err(NrfMemoryXError::MissingMinimumRuntimeStack {
                reservation: self.minimum_runtime_stack,
            });
        };
        if stack.address_space != ram.id
            || stack.accounting
                != (ReservationAccounting::Dedicated {
                    charge: ReservationCharge::AdditionalToStatic,
                })
        {
            return Err(NrfMemoryXError::InvalidMinimumRuntimeStack {
                reservation: stack.id,
            });
        }
        Ok(NrfMemoryXLayout {
            application_flash: firmware.range,
            application_ram,
            minimum_runtime_stack_bytes: stack.bytes,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NrfMemoryXError {
    InvalidMemoryProfile(ValidationError),
    UnsupportedProfile {
        profile: MemoryProfileId,
    },
    DuplicateProfile {
        profile: MemoryProfileId,
    },
    UnsupportedArchitecture {
        profile: MemoryProfileId,
        architecture: ProcessorArchitecture,
    },
    FirmwareOutsideInternalFlash {
        address_space: AddressSpaceId,
    },
    MissingApplicationRam {
        address_space: AddressSpaceId,
    },
    InvalidApplicationRam {
        address_space: AddressSpaceId,
    },
    UnboundedApplicationRam {
        address_space: AddressSpaceId,
    },
    MissingMinimumRuntimeStack {
        reservation: ReservationId,
    },
    InvalidMinimumRuntimeStack {
        reservation: ReservationId,
    },
}

impl fmt::Display for NrfMemoryXError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid nRF memory.x binding: {self:?}")
    }
}

#[cfg(test)]
mod tests;
