use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantError,
    SetRemoteControlControllerGrantOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRemoteControlControllerGrantServiceError {
    Unavailable,
    CapacityExhausted,
    TransactionInProgress,
}

impl From<SetRemoteControlControllerGrantError> for SetRemoteControlControllerGrantServiceError {
    fn from(error: SetRemoteControlControllerGrantError) -> Self {
        match error {
            SetRemoteControlControllerGrantError::CapacityExhausted => Self::CapacityExhausted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeRemoteControlControllerServiceError {
    Unavailable,
    TransactionInProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRemoteControlControllerGrantControlError {
    NodeStopped,
    Busy,
    Unavailable,
    CapacityExhausted,
}

impl From<SetRemoteControlControllerGrantServiceError>
    for SetRemoteControlControllerGrantControlError
{
    fn from(error: SetRemoteControlControllerGrantServiceError) -> Self {
        match error {
            SetRemoteControlControllerGrantServiceError::Unavailable => Self::Unavailable,
            SetRemoteControlControllerGrantServiceError::CapacityExhausted => {
                Self::CapacityExhausted
            }
            SetRemoteControlControllerGrantServiceError::TransactionInProgress => Self::Busy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeRemoteControlControllerControlError {
    NodeStopped,
    Busy,
    Unavailable,
}

impl From<RevokeRemoteControlControllerServiceError> for RevokeRemoteControlControllerControlError {
    fn from(error: RevokeRemoteControlControllerServiceError) -> Self {
        match error {
            RevokeRemoteControlControllerServiceError::Unavailable => Self::Unavailable,
            RevokeRemoteControlControllerServiceError::TransactionInProgress => Self::Busy,
        }
    }
}

pub trait RemoteControlControllerGrantControl {
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
