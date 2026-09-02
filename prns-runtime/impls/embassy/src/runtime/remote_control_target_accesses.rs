use crate::engine::CommandId;
use crate::identity::IdentityHash;
use crate::remote_control::{
    ForgetRemoteControlTargetOutcome, RemoteControlTargetAccess, RemoteControlTargetIdentity,
    SetRemoteControlTargetAccessOutcome,
};
use crate::runtime::{
    ForgetRemoteControlTargetServiceError, RemoteControlTargetInventory,
    RemoteControlTargetInventoryServiceError, ResolveRemoteControlTargetServiceError,
    ResolvedRemoteControlTarget, SetRemoteControlTargetAccessServiceError,
};

use super::remote_control_authorization_exchange::{
    RemoteControlAuthorizationCommand, RemoteControlAuthorizationExchange,
};

pub(super) enum RemoteControlTargetAccessCommand {
    Inventory {
        id: CommandId,
    },
    ResolveTarget {
        id: CommandId,
        target: IdentityHash,
    },
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
    Inventory(Result<RemoteControlTargetInventory, RemoteControlTargetInventoryServiceError>),
    Resolved(Result<ResolvedRemoteControlTarget, ResolveRemoteControlTargetServiceError>),
    AccessSet(
        Result<SetRemoteControlTargetAccessOutcome, SetRemoteControlTargetAccessServiceError>,
    ),
    Forgotten(Result<ForgetRemoteControlTargetOutcome, ForgetRemoteControlTargetServiceError>),
}

pub(super) type RemoteControlTargetAccessExchange<M> = RemoteControlAuthorizationExchange<
    M,
    RemoteControlTargetAccessCommand,
    RemoteControlTargetAccessCompletion,
>;

impl RemoteControlAuthorizationCommand for RemoteControlTargetAccessCommand {
    fn id(&self) -> CommandId {
        match self {
            Self::Inventory { id }
            | Self::ResolveTarget { id, .. }
            | Self::SetTargetAccess { id, .. }
            | Self::ForgetTarget { id, .. } => *id,
        }
    }
}
