use super::*;
use crate::{AddressSpaceId, ReservationId, HELTEC_V4, T_ECHO_S140_V6};

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
