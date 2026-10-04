mod begin;
mod cancel;
mod complete;
mod fail;

pub use begin::{BeginEnrollment, BeginEnrollmentError, BeginEnrollmentOutcome};
pub use cancel::{CancelEnrollment, CancelEnrollmentOutcome};
pub use complete::{CompleteEnrollment, CompleteEnrollmentOutcome};
pub use fail::{FailEnrollment, FailEnrollmentOutcome};

use super::EnrollmentState;
use crate::domain_primitives::Enrollment;
use prns_core::identity::IdentityPublicKeys;

impl EnrollmentState {
    fn pending_target(&self, enrollment: Enrollment) -> Option<IdentityPublicKeys> {
        match self {
            Self::Pairing {
                enrollment: active,
                target,
            } if *active == enrollment => Some(*target),
            Self::Pairing { .. } | Self::Planned | Self::Paired { .. } => None,
        }
    }
}

mod prepare;
pub use prepare::{PrepareEnrollmentCompletion, PrepareEnrollmentCompletionOutcome};
