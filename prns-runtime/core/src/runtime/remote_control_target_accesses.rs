use crate::identity::IdentityHash;
use crate::remote_control::{
    ForgetRemoteControlTargetOutcome, RemoteControlControllerIdentity, RemoteControlEndpoint,
    RemoteControlRequestSet, RemoteControlTargetAccess, RemoteControlTargetIdentity,
    SetRemoteControlTargetAccessError, SetRemoteControlTargetAccessOutcome,
};

#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedRemoteControlTarget {
    target: IdentityHash,
    endpoint: RemoteControlEndpoint,
    controller: RemoteControlControllerIdentity,
    permitted_requests: RemoteControlRequestSet,
}

impl ResolvedRemoteControlTarget {
    #[must_use]
    pub const fn target(&self) -> IdentityHash {
        self.target
    }

    #[must_use]
    pub const fn endpoint(&self) -> RemoteControlEndpoint {
        self.endpoint
    }

    #[must_use]
    pub const fn controller(&self) -> &RemoteControlControllerIdentity {
        &self.controller
    }

    #[must_use]
    pub const fn permitted_requests(&self) -> &RemoteControlRequestSet {
        &self.permitted_requests
    }
}

impl From<(&RemoteControlControllerIdentity, &RemoteControlTargetAccess)>
    for ResolvedRemoteControlTarget
{
    fn from(
        (controller, access): (&RemoteControlControllerIdentity, &RemoteControlTargetAccess),
    ) -> Self {
        Self {
            target: access.target().identity_hash(),
            endpoint: access.endpoint(),
            controller: *controller,
            permitted_requests: *access.permitted_requests(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveRemoteControlTargetServiceError {
    Unavailable,
    TargetNotAuthorized,
    TransactionInProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveRemoteControlTargetControlError {
    NodeStopped,
    Busy,
    Unavailable,
    TargetNotAuthorized,
}

impl From<ResolveRemoteControlTargetServiceError> for ResolveRemoteControlTargetControlError {
    fn from(error: ResolveRemoteControlTargetServiceError) -> Self {
        match error {
            ResolveRemoteControlTargetServiceError::Unavailable => Self::Unavailable,
            ResolveRemoteControlTargetServiceError::TargetNotAuthorized => {
                Self::TargetNotAuthorized
            }
            ResolveRemoteControlTargetServiceError::TransactionInProgress => Self::Busy,
        }
    }
}

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
    fn resolve_remote_control_target(
        &self,
        target: IdentityHash,
    ) -> impl core::future::Future<
        Output = Result<ResolvedRemoteControlTarget, ResolveRemoteControlTargetControlError>,
    > + Send;

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
