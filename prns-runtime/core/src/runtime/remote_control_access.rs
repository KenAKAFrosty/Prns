use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantError,
    SetRemoteControlControllerGrantOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRemoteControlControllerGrantControlError {
    NodeStopped,
    Busy,
    CapacityExhausted,
}

impl From<SetRemoteControlControllerGrantError> for SetRemoteControlControllerGrantControlError {
    fn from(error: SetRemoteControlControllerGrantError) -> Self {
        match error {
            SetRemoteControlControllerGrantError::CapacityExhausted => Self::CapacityExhausted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeRemoteControlControllerControlError {
    NodeStopped,
    Busy,
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
