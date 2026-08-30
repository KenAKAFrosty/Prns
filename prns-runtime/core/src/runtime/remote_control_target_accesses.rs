use crate::remote_control::{
    ForgetRemoteControlTargetOutcome, RemoteControlTargetAccess, RemoteControlTargetIdentity,
    SetRemoteControlTargetAccessError, SetRemoteControlTargetAccessOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRemoteControlTargetAccessServiceError {
    Unavailable,
    CapacityExhausted,
    TransactionInProgress,
}

impl From<SetRemoteControlTargetAccessError> for SetRemoteControlTargetAccessServiceError {
    fn from(error: SetRemoteControlTargetAccessError) -> Self {
        match error {
            SetRemoteControlTargetAccessError::CapacityExhausted => Self::CapacityExhausted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgetRemoteControlTargetServiceError {
    Unavailable,
    TransactionInProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRemoteControlTargetAccessControlError {
    NodeStopped,
    Busy,
    Unavailable,
    CapacityExhausted,
}

impl From<SetRemoteControlTargetAccessServiceError> for SetRemoteControlTargetAccessControlError {
    fn from(error: SetRemoteControlTargetAccessServiceError) -> Self {
        match error {
            SetRemoteControlTargetAccessServiceError::Unavailable => Self::Unavailable,
            SetRemoteControlTargetAccessServiceError::CapacityExhausted => Self::CapacityExhausted,
            SetRemoteControlTargetAccessServiceError::TransactionInProgress => Self::Busy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgetRemoteControlTargetControlError {
    NodeStopped,
    Busy,
    Unavailable,
}

impl From<ForgetRemoteControlTargetServiceError> for ForgetRemoteControlTargetControlError {
    fn from(error: ForgetRemoteControlTargetServiceError) -> Self {
        match error {
            ForgetRemoteControlTargetServiceError::Unavailable => Self::Unavailable,
            ForgetRemoteControlTargetServiceError::TransactionInProgress => Self::Busy,
        }
    }
}

pub trait RemoteControlTargetAccessControl {
    fn set_remote_control_target_access(
        &self,
        access: RemoteControlTargetAccess,
    ) -> impl core::future::Future<
        Output = Result<
            SetRemoteControlTargetAccessOutcome,
            SetRemoteControlTargetAccessControlError,
        >,
    > + Send;

    fn forget_remote_control_target(
        &self,
        target: RemoteControlTargetIdentity,
    ) -> impl core::future::Future<
        Output = Result<ForgetRemoteControlTargetOutcome, ForgetRemoteControlTargetControlError>,
    > + Send;
}
