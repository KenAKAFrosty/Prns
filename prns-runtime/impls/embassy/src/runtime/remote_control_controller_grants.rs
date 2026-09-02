use crate::engine::CommandId;
use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
};
use crate::runtime::{
    RevokeRemoteControlControllerServiceError, SetRemoteControlControllerGrantServiceError,
};

use super::remote_control_authorization_exchange::{
    RemoteControlAuthorizationCommand, RemoteControlAuthorizationExchange,
};

pub(super) enum RemoteControlControllerGrantCommand {
    SetControllerGrant {
        id: CommandId,
        grant: RemoteControlControllerGrant,
    },
    RevokeController {
        id: CommandId,
        controller: RemoteControlControllerIdentity,
    },
}

pub(super) enum RemoteControlControllerGrantCompletion {
    ControllerGrantSet(
        Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantServiceError>,
    ),
    ControllerRevoked(
        Result<RevokeRemoteControlControllerOutcome, RevokeRemoteControlControllerServiceError>,
    ),
}

pub(super) type RemoteControlControllerGrantExchange<M> = RemoteControlAuthorizationExchange<
    M,
    RemoteControlControllerGrantCommand,
    RemoteControlControllerGrantCompletion,
>;

impl RemoteControlAuthorizationCommand for RemoteControlControllerGrantCommand {
    fn id(&self) -> CommandId {
        match self {
            Self::SetControllerGrant { id, .. } | Self::RevokeController { id, .. } => *id,
        }
    }
}
