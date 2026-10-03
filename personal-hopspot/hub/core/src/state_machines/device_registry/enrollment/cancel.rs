use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableUpdateOutcome};

use super::super::{DeviceRegistry, EnrollmentState};
use crate::domain_primitives::Enrollment;

#[derive(Debug, PartialEq, Eq)]
pub struct CancelEnrollment {
    pub enrollment: Enrollment,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum CancelEnrollmentOutcome {
    Cancelled { enrollment: Enrollment },
    MissingDevice { enrollment: Enrollment },
    StaleEnrollment { enrollment: Enrollment },
}

impl StepInputOf<DeviceRegistry> for CancelEnrollment {
    type Outcome = CancelEnrollmentOutcome;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        let enrollment = self.enrollment;
        match registry
            .devices
            .update(WarpId::new(enrollment.device.0), |mut row| {
                let state = row.enrollment_mut();
                if state.pending_target(enrollment).is_none() {
                    return CancelEnrollmentOutcome::StaleEnrollment { enrollment };
                }
                *state = EnrollmentState::Planned;
                CancelEnrollmentOutcome::Cancelled { enrollment }
            }) {
            WarpTableUpdateOutcome::Updated { output, .. } => output,
            WarpTableUpdateOutcome::Absent { .. } => {
                CancelEnrollmentOutcome::MissingDevice { enrollment }
            }
        }
    }
}
