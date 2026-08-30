use crate::engine::CommandId;
use crate::remote_control::{
    ForgetRemoteControlTargetOutcome, RemoteControlTargetAccess, RemoteControlTargetIdentity,
    SetRemoteControlTargetAccessOutcome,
};
use crate::runtime::{
    ForgetRemoteControlTargetServiceError, SetRemoteControlTargetAccessServiceError,
};

use super::remote_control_authorization_exchange::{
    RemoteControlAuthorizationCommand, RemoteControlAuthorizationExchange,
};

pub(super) enum RemoteControlTargetAccessCommand {
    SetTargetAccess {
        id: CommandId,
        access: RemoteControlTargetAccess,
    },
    ForgetTarget {
        id: CommandId,
        target: RemoteControlTargetIdentity,
    },
}

pub(super) enum RemoteControlTargetAccessCompletion {
    TargetAccessSet(
        Result<SetRemoteControlTargetAccessOutcome, SetRemoteControlTargetAccessServiceError>,
    ),
    TargetForgotten(
        Result<ForgetRemoteControlTargetOutcome, ForgetRemoteControlTargetServiceError>,
    ),
}

pub(super) type RemoteControlTargetAccessExchange<M> = RemoteControlAuthorizationExchange<
    M,
    RemoteControlTargetAccessCommand,
    RemoteControlTargetAccessCompletion,
>;

impl RemoteControlAuthorizationCommand for RemoteControlTargetAccessCommand {
    fn id(&self) -> CommandId {
        match self {
            Self::SetTargetAccess { id, .. } | Self::ForgetTarget { id, .. } => *id,
        }
    }
}
