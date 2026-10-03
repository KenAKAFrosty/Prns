use core::num::NonZeroU64;
use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableUpdateWithOutcome};
use prns_core::remote_control::{RemoteControlPairingAttemptId, RemoteControlTargetIdentity};

use super::super::{DeviceRegistry, EnrollmentState};
use crate::domain_primitives::{DeviceId, Enrollment};

#[derive(Debug, PartialEq, Eq)]
pub struct BeginEnrollment {
    pub device: DeviceId,
    pub attempt: RemoteControlPairingAttemptId,
    pub target: RemoteControlTargetIdentity,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum BeginEnrollmentOutcome {
    Started {
        enrollment: Enrollment,
    },
    MissingDevice {
        rejected: BeginEnrollment,
    },
    AlreadyPairing {
        rejected: BeginEnrollment,
        active: Enrollment,
    },
    AlreadyPaired {
        rejected: BeginEnrollment,
    },
    IdentifiersExhausted {
        rejected: BeginEnrollment,
    },
}

impl StepInputOf<DeviceRegistry> for BeginEnrollment {
    type Outcome = BeginEnrollmentOutcome;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        match registry
            .devices
            .update_with(WarpId::new(self.device.0), self, |mut row, input| {
                row.enrollment_mut()
                    .begin(input, &mut registry.next_enrollment)
            }) {
            WarpTableUpdateWithOutcome::Updated { output, .. } => output,
            WarpTableUpdateWithOutcome::Absent {
                context: rejected, ..
            } => BeginEnrollmentOutcome::MissingDevice { rejected },
        }
    }
}

impl EnrollmentState {
    fn begin(
        &mut self,
        input: BeginEnrollment,
        next: &mut Option<NonZeroU64>,
    ) -> BeginEnrollmentOutcome {
        match self {
            Self::Planned => {}
            Self::Pairing { enrollment, .. } => {
                return BeginEnrollmentOutcome::AlreadyPairing {
                    rejected: input,
                    active: *enrollment,
                };
            }
            Self::Paired { .. } => {
                return BeginEnrollmentOutcome::AlreadyPaired { rejected: input };
            }
        }
        let Some(generation) = *next else {
            return BeginEnrollmentOutcome::IdentifiersExhausted { rejected: input };
        };
        let enrollment = Enrollment {
            device: input.device,
            generation,
            attempt: input.attempt,
        };
        *self = Self::Pairing {
            enrollment,
            target: *input.target.public_keys(),
        };
        *next = generation.checked_add(1);
        BeginEnrollmentOutcome::Started { enrollment }
    }
}
