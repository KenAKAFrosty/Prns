use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity,
    RevokeRemoteControlControllerError, RevokeRemoteControlControllerOutcome,
    SetRemoteControlControllerGrantError, SetRemoteControlControllerGrantOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRemoteControlControllerGrantControlError {
    NodeStopped,
    Busy,
    Unavailable,
    CapacityExhausted,
}

impl From<SetRemoteControlControllerGrantError> for SetRemoteControlControllerGrantControlError {
    fn from(error: SetRemoteControlControllerGrantError) -> Self {
        match error {
            SetRemoteControlControllerGrantError::Unavailable => Self::Unavailable,
            SetRemoteControlControllerGrantError::CapacityExhausted => Self::CapacityExhausted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeRemoteControlControllerControlError {
    NodeStopped,
    Busy,
    Unavailable,
}

impl From<RevokeRemoteControlControllerError> for RevokeRemoteControlControllerControlError {
    fn from(error: RevokeRemoteControlControllerError) -> Self {
        match error {
            RevokeRemoteControlControllerError::Unavailable => Self::Unavailable,
        }
    }
}

pub trait RemoteControlAccessControl {
    fn set_remote_control_controller_grant(
        &self,
        grant: RemoteControlControllerGrant,
    ) -> impl core::future::Future<
        Output = Result<
            SetRemoteControlControllerGrantOutcome,
            SetRemoteControlControllerGrantControlError,
        >,
    > + Send;

    fn revoke_remote_control_controller(
        &self,
        controller: RemoteControlControllerIdentity,
    ) -> impl core::future::Future<
        Output = Result<
            RevokeRemoteControlControllerOutcome,
            RevokeRemoteControlControllerControlError,
        >,
    > + Send;
}
