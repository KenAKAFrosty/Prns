use super::*;
use crate::{AddressSpaceId, ReservationId, HELTEC_V4, T_ECHO_S140_V6};
use std::string::ToString;

#[test]
fn memory_x_rendering_is_owned_by_the_resolved_layout() {
    let layout = NrfMemoryXLayout {
        application_flash: AddressRange::new(0x26000, 0xBF000),
        application_ram: AddressRange::new(0x2000_C000, 0x2004_0000),
        minimum_runtime_stack_bytes: 68 * 1024,
    };

    assert_eq!(
        layout.to_string(),
        "APPLICATION_FLASH_ORIGIN = 0x00026000;\n\
APPLICATION_FLASH_BYTES = 0x99000;\n\
APPLICATION_RAM_ORIGIN = 0x2000C000;\n\
APPLICATION_RAM_BYTES = 0x34000;\n\n\
MEMORY\n\
{\n\
  FLASH : ORIGIN = APPLICATION_FLASH_ORIGIN, LENGTH = APPLICATION_FLASH_BYTES\n\
  RAM   : ORIGIN = APPLICATION_RAM_ORIGIN, LENGTH = APPLICATION_RAM_BYTES\n\
}\n\n\
ASSERT(\n\
  ORIGIN(RAM) + LENGTH(RAM) - _stack_end >= 69632,\n\
  \"nRF52840 static memory leaves too little runtime stack\"\n\
)\n"
    );
}

#[test]
fn unsupported_profiles_are_rejected_before_format_resolution() {
    let binding = NrfMemoryXBinding {
        profiles: &[T_ECHO_S140_V6.id],
        application_ram: AddressSpaceId("internal-ram"),
        minimum_runtime_stack: ReservationId("minimum-runtime-stack"),
    };

    assert_eq!(
        binding.resolve(&HELTEC_V4),
        Err(NrfMemoryXError::UnsupportedProfile {
            profile: HELTEC_V4.id,
        })
    );
}

#[test]
fn missing_stack_bindings_are_rejected() {
    let binding = NrfMemoryXBinding {
        profiles: &[T_ECHO_S140_V6.id],
        application_ram: AddressSpaceId("internal-ram"),
        minimum_runtime_stack: ReservationId("missing-stack"),
    };

    assert_eq!(
        binding.resolve(&T_ECHO_S140_V6),
        Err(NrfMemoryXError::MissingMinimumRuntimeStack {
            reservation: ReservationId("missing-stack"),
        })
    );
}
