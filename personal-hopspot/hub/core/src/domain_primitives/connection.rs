use core::num::NonZeroU64;
use prns_core::identity::IdentityPublicKeys;
use prns_core::remote_control::RemoteControlTargetIdentity;

use super::DeviceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Connection {
    pub(crate) device: DeviceId,
    pub(crate) generation: NonZeroU64,
    pub(crate) target: IdentityPublicKeys,
}

impl Connection {
    pub const fn device(self) -> DeviceId {
        self.device
    }

    pub const fn target(self) -> RemoteControlTargetIdentity {
        RemoteControlTargetIdentity::new(self.target)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectionReason {
    Cancelled,
    TimedOut,
    TransportLost,
    AuthenticationFailed,
}
