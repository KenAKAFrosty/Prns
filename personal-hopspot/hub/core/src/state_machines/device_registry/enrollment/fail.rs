use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableUpdateWithOutcome};

use super::super::{DeviceRegistry, EnrollmentState};
use crate::domain_primitives::{Enrollment, EnrollmentFailure};

#[derive(Debug, PartialEq, Eq)]
pub struct FailEnrollment {
    pub enrollment: Enrollment,
    pub reason: EnrollmentFailure,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum FailEnrollmentOutcome {
    Failed {
        enrollment: Enrollment,
        reason: EnrollmentFailure,
    },
    MissingDevice {
        rejected: FailEnrollment,
    },
    StaleEnrollment {
        rejected: FailEnrollment,
    },
}

impl StepInputOf<DeviceRegistry> for FailEnrollment {
    type Outcome = FailEnrollmentOutcome;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        match registry.devices.update_with(
            WarpId::new(self.enrollment.device.0),
            self,
            |mut row, input| {
                let state = row.enrollment_mut();
                if state.pending_target(input.enrollment).is_none() {
                    return FailEnrollmentOutcome::StaleEnrollment { rejected: input };
                }
                *state = EnrollmentState::Planned;
                FailEnrollmentOutcome::Failed {
                    enrollment: input.enrollment,
                    reason: input.reason,
                }
            },
        ) {
            WarpTableUpdateWithOutcome::Updated { output, .. } => output,
            WarpTableUpdateWithOutcome::Absent {
                context: rejected, ..
            } => FailEnrollmentOutcome::MissingDevice { rejected },
        }
    }
}
