use crate::engine::{EstablishLinkFailure, IdentifyFailure};
use crate::identity::IdentityHash;
use crate::remote_control::{RemoteControlRequestKind, RemoteControlRequestSet};
use crate::routing::links::LinkId;
use crate::wire::DestinationHash;

use super::{
    RemoteControlError, RemoteControlTargetAccessControl, ResolveRemoteControlTargetControlError,
    ResolvedRemoteControlTarget, SendError,
};

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlTargetConnection {
    target: IdentityHash,
    link_id: LinkId,
    permitted_requests: RemoteControlRequestSet,
}

impl RemoteControlTargetConnection {
    #[must_use]
    fn established(link_id: LinkId, resolved: ResolvedRemoteControlTarget) -> Self {
        Self {
            target: resolved.target(),
            link_id,
            permitted_requests: *resolved.permitted_requests(),
        }
    }

    #[must_use]
    pub const fn target(&self) -> IdentityHash {
        self.target
    }

    #[must_use]
    pub const fn link_id(&self) -> LinkId {
        self.link_id
    }

    #[must_use]
    pub const fn permitted_requests(&self) -> &RemoteControlRequestSet {
        &self.permitted_requests
    }

    #[must_use]
    pub fn permits(&self, request: RemoteControlRequestKind) -> bool {
        self.permitted_requests.supports(request)
    }

    pub fn admit(
        &self,
        request: RemoteControlRequestKind,
    ) -> Result<(), RemoteControlTargetOperationError> {
        if self.permits(request) {
            return Ok(());
        }
        Err(RemoteControlTargetOperationError::NotPermitted(request))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectRemoteControlTargetError {
    Resolve(ResolveRemoteControlTargetControlError),
    EstablishLink(SendError<EstablishLinkFailure>),
    Identify(SendError<IdentifyFailure>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseRemoteControlTargetOutcome {
    Queued,
    NotQueued,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlTargetOperationError {
    NotPermitted(RemoteControlRequestKind),
    Exchange(RemoteControlError),
}

impl From<RemoteControlError> for RemoteControlTargetOperationError {
    fn from(error: RemoteControlError) -> Self {
        Self::Exchange(error)
    }
}

pub trait RemoteControlTargetConnectionControl: RemoteControlTargetConnectionTransport {
    fn establish_remote_control_target(
        &self,
        target: IdentityHash,
    ) -> impl core::future::Future<
        Output = Result<RemoteControlTargetConnection, ConnectRemoteControlTargetError>,
    > + Send {
        async move {
            let resolved = self
                .resolve_remote_control_target(target)
                .await
                .map_err(ConnectRemoteControlTargetError::Resolve)?;
            let destination = resolved.endpoint().destination_hash();
            let controller = resolved.controller().identity_hash();
            let link_id = self
                .establish_remote_control_link(destination)
                .await
                .map_err(ConnectRemoteControlTargetError::EstablishLink)?;
            if let Err(error) = self.identify_remote_control_link(link_id, controller).await {
                self.close_remote_control_link(link_id);
                return Err(ConnectRemoteControlTargetError::Identify(error));
            }
            Ok(RemoteControlTargetConnection::established(
                link_id, resolved,
            ))
        }
    }
}

impl<T> RemoteControlTargetConnectionControl for T where T: RemoteControlTargetConnectionTransport {}

pub trait RemoteControlTargetConnectionTransport: RemoteControlTargetAccessControl + Sync {
    fn establish_remote_control_link(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<Output = Result<LinkId, SendError<EstablishLinkFailure>>> + Send;

    fn identify_remote_control_link(
        &self,
        link_id: LinkId,
        identity: IdentityHash,
    ) -> impl core::future::Future<Output = Result<(), SendError<IdentifyFailure>>> + Send;

    fn close_remote_control_link(&self, link_id: LinkId) -> CloseRemoteControlTargetOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
    };
    use crate::remote_control::{
        RemoteControlControllerIdentity, RemoteControlTargetAccess, RemoteControlTargetIdentity,
    };

    fn public_keys(fill: u8) -> IdentityPublicKeys {
        IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
        }
    }

    #[test]
    fn established_connection_retains_resolution_and_admits_only_permitted_requests() {
        let controller = RemoteControlControllerIdentity::new(public_keys(0x21));
        let access = RemoteControlTargetAccess::new(
            RemoteControlTargetIdentity::new(public_keys(0x31)),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        )
        .unwrap();
        let target = access.target().identity_hash();
        let link_id = LinkId::new([0x41; 16]);
        let connection = RemoteControlTargetConnection::established(
            link_id,
            ResolvedRemoteControlTarget::from((&controller, &access)),
        );

        assert_eq!(
            (connection.target(), connection.link_id()),
            (target, link_id)
        );
        assert_eq!(
            connection.permitted_requests(),
            &RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        );
        assert_eq!(connection.admit(RemoteControlRequestKind::Describe), Ok(()));
        assert_eq!(
            connection.admit(RemoteControlRequestKind::AnnounceSelf),
            Err(RemoteControlTargetOperationError::NotPermitted(
                RemoteControlRequestKind::AnnounceSelf,
            )),
        );
    }
}
