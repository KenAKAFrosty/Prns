use core::num::NonZeroU64;

use prns_core::remote_control::RemoteControlPairingAttemptId;

use super::DeviceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enrollment {
    pub(crate) device: DeviceId,
    pub(crate) generation: NonZeroU64,
    pub(crate) attempt: RemoteControlPairingAttemptId,
}

impl Enrollment {
    pub const fn device(self) -> DeviceId {
        self.device
    }

    pub const fn attempt(self) -> RemoteControlPairingAttemptId {
        self.attempt
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentFailure {
    Expired,
    Rejected,
    ConnectionLost,
    PersistenceFailed,
}
