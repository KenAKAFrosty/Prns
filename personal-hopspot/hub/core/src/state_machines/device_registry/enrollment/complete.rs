use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableUpdateWithOutcome};
use prns_core::remote_control::RemoteControlTargetIdentity;

use super::super::{DeviceRegistry, EnrollmentState};
use crate::domain_primitives::{DeviceId, Enrollment};

#[derive(Debug, PartialEq, Eq)]
pub struct CompleteEnrollment {
    pub enrollment: Enrollment,
    pub target: RemoteControlTargetIdentity,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum CompleteEnrollmentOutcome {
    Recorded {
        enrollment: Enrollment,
    },
    MissingDevice {
        rejected: CompleteEnrollment,
    },
    StaleEnrollment {
        rejected: CompleteEnrollment,
    },
    TargetMismatch {
        rejected: CompleteEnrollment,
    },
    TargetAlreadyPaired {
        rejected: CompleteEnrollment,
        device: DeviceId,
    },
}

impl StepInputOf<DeviceRegistry> for CompleteEnrollment {
    type Outcome = CompleteEnrollmentOutcome;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        let already_paired = registry.paired_target(&self.target);
        match registry.devices.update_with(
            WarpId::new(self.enrollment.device.0),
            self,
            |mut row, input| row.enrollment_mut().complete(input, already_paired),
        ) {
            WarpTableUpdateWithOutcome::Updated { output, .. } => output,
            WarpTableUpdateWithOutcome::Absent {
                context: rejected, ..
            } => CompleteEnrollmentOutcome::MissingDevice { rejected },
        }
    }
}

impl EnrollmentState {
    fn complete(
        &mut self,
        input: CompleteEnrollment,
        already_paired: Option<DeviceId>,
    ) -> CompleteEnrollmentOutcome {
        match self.check_completion(&input, already_paired) {
            CompletionCheck::Ready { target } => {
                *self = Self::Paired { target };
                CompleteEnrollmentOutcome::Recorded {
                    enrollment: input.enrollment,
                }
            }
            CompletionCheck::Stale => {
                CompleteEnrollmentOutcome::StaleEnrollment { rejected: input }
            }
            CompletionCheck::Mismatch => {
                CompleteEnrollmentOutcome::TargetMismatch { rejected: input }
            }
            CompletionCheck::Duplicate { device } => {
                CompleteEnrollmentOutcome::TargetAlreadyPaired {
                    rejected: input,
                    device,
                }
            }
        }
    }

    pub(super) fn check_completion(
        &self,
        input: &CompleteEnrollment,
        already_paired: Option<DeviceId>,
    ) -> CompletionCheck {
        let Some(target) = self.pending_target(input.enrollment) else {
            return CompletionCheck::Stale;
        };
        if input.target.public_keys() != &target {
            return CompletionCheck::Mismatch;
        }
        if let Some(device) = already_paired {
            return CompletionCheck::Duplicate { device };
        }
        CompletionCheck::Ready { target }
    }
}

pub(super) enum CompletionCheck {
    Ready {
        target: prns_core::identity::IdentityPublicKeys,
    },
    Stale,
    Mismatch,
    Duplicate {
        device: DeviceId,
    },
}

impl DeviceRegistry {
    pub(super) fn paired_target(&self, identity: &RemoteControlTargetIdentity) -> Option<DeviceId> {
        self.devices.rows().find_map(|row| match row.enrollment {
            EnrollmentState::Paired { target } if target == identity.public_keys() => {
                Some(DeviceId(row.id.value()))
            }
            EnrollmentState::Paired { .. }
            | EnrollmentState::Pairing { .. }
            | EnrollmentState::Planned => None,
        })
    }
}
