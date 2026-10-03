use prns_core::identity::IdentityPublicKeys;

use crate::domain_primitives::{DeviceId, DeviceLabel, Enrollment};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollmentState {
    Planned,
    Pairing {
        enrollment: Enrollment,
        target: IdentityPublicKeys,
    },
    Paired {
        target: IdentityPublicKeys,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub id: DeviceId,
    pub label: DeviceLabel,
    pub enrollment: EnrollmentState,
}
