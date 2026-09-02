use crate::engine::{EstablishLinkFailure, IdentifyFailure};
use crate::identity::IdentityHash;
use crate::routing::links::LinkId;
use crate::runtime::{
    CloseRemoteControlTargetOutcome, ConnectRemoteControlTargetError, RemoteControlAnnounceSelf,
    RemoteControlDescribe, RemoteControlTargetConnection, RemoteControlTargetConnectionControl,
    RemoteControlTargetConnectionTransport, RemoteControlTargetOperationError, SendError,
};
use crate::units::RttMillis;
use crate::wire::DestinationHash;
use prns_core::remote_control::RemoteControlDescription;

use super::{PrnsNodeHandle, RemoteControlHandle};

pub struct RemoteControlTargetHandle<'a> {
    remote_control: RemoteControlHandle<'a>,
    connection: RemoteControlTargetConnection,
}

impl PrnsNodeHandle {
    pub async fn connect_remote_control_target(
        &self,
        target: IdentityHash,
    ) -> Result<RemoteControlTargetHandle<'_>, ConnectRemoteControlTargetError> {
        let connection = self.establish_remote_control_target(target).await?;
        Ok(RemoteControlTargetHandle {
            remote_control: self.remote_control(connection.link_id()),
            connection,
        })
    }
}

impl RemoteControlTargetConnectionTransport for PrnsNodeHandle {
    async fn establish_remote_control_link(
        &self,
        destination: DestinationHash,
    ) -> Result<LinkId, SendError<EstablishLinkFailure>> {
        self.establish_link(destination).await
    }

    async fn identify_remote_control_link(
        &self,
        link_id: LinkId,
        identity: IdentityHash,
    ) -> Result<(), SendError<IdentifyFailure>> {
        self.identify(link_id, identity).await
    }

    fn close_remote_control_link(&self, link_id: LinkId) -> CloseRemoteControlTargetOutcome {
        match self.close_link(link_id) {
            true => CloseRemoteControlTargetOutcome::Queued,
            false => CloseRemoteControlTargetOutcome::NotQueued,
        }
    }
}

impl RemoteControlTargetHandle<'_> {
    #[must_use]
    pub const fn connection(&self) -> &RemoteControlTargetConnection {
        &self.connection
    }

    pub fn close(self) -> CloseRemoteControlTargetOutcome {
        self.remote_control
            .node
            .close_remote_control_link(self.connection.link_id())
    }

    pub async fn announce_self(&self) -> Result<RttMillis, RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlAnnounceSelf::REQUEST.kind())?;
        self.remote_control
            .announce_self()
            .await
            .map_err(Into::into)
    }

    pub async fn describe(
        &self,
    ) -> Result<(RemoteControlDescription, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlDescribe::REQUEST.kind())?;
        self.remote_control.describe().await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use crate::engine::{
        CloseLink, EstablishLink, Identify, IdentifyFailure, LinkEstablished, PrnsCommand,
        Settlement,
    };
    use crate::manifold::driver::HostCommand;
    use crate::remote_control::{
        FixedRemoteControlTargetAccessTable, RemoteControlRequestKind, RemoteControlRequestSet,
        RemoteControlTargetAccess, RemoteControlTargetAccessTable, RemoteControlTargetIdentity,
    };
    use crate::routing::links::LinkId;
    use crate::runtime::remote_control_target_accesses::RemoteControlTargetAccessCommand;
    use crate::runtime::{
        RemoteControlTargetAccessControl, RemoteControlTargetInventory,
        RemoteControlTargetOperationError, ResolvedRemoteControlTarget,
    };

    use super::{ConnectRemoteControlTargetError, PrnsNodeHandle};

    fn resolved_target() -> (
        crate::identity::IdentityHash,
        crate::wire::DestinationHash,
        crate::identity::IdentityHash,
        ResolvedRemoteControlTarget,
    ) {
        let service = crate::runtime::node_facade::test_remote_control_service();
        let identities = service
            .configuration()
            .unwrap()
            .identity_secrets()
            .identities();
        let access = RemoteControlTargetAccess::new(
            RemoteControlTargetIdentity::new(*identities.target().public_keys()),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        )
        .unwrap();
        (
            access.target().identity_hash(),
            access.endpoint().destination_hash(),
            identities.controller().identity_hash(),
            ResolvedRemoteControlTarget::from((identities.controller(), &access)),
        )
    }

    #[tokio::test]
    async fn target_inventory_preserves_the_exact_runtime_settlement() {
        let (commands, _command_rx) = mpsc::unbounded_channel();
        let (handle, mut target_accesses) =
            PrnsNodeHandle::over_with_remote_control_target_access_lane(commands);
        let service = crate::runtime::node_facade::test_remote_control_service();
        let identities = service
            .configuration()
            .unwrap()
            .identity_secrets()
            .identities();
        let access = RemoteControlTargetAccess::new(
            RemoteControlTargetIdentity::new(*identities.target().public_keys()),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        )
        .unwrap();
        let mut table = FixedRemoteControlTargetAccessTable::default();
        assert!(table.set_target_access(access).is_ok());
        let expected = RemoteControlTargetInventory::try_from(&table).unwrap();
        let completion = RemoteControlTargetInventory::try_from(&table).unwrap();

        let reading = handle.remote_control_target_inventory();
        let driving = async move {
            let Some(RemoteControlTargetAccessCommand::Inventory { completion: settle }) =
                target_accesses.receive().await
            else {
                panic!("target inventory command")
            };
            assert!(settle.send(Ok(completion)).is_ok());
        };
        let (inventory, ()) = tokio::join!(reading, driving);

        assert_eq!(inventory, Ok(expected));
    }

    #[tokio::test]
    async fn connection_resolves_links_identifies_and_refuses_unpermitted_egress() {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let (handle, mut target_accesses) =
            PrnsNodeHandle::over_with_remote_control_target_access_lane(commands);
        let (target, destination, controller, resolved) = resolved_target();
        let link_id = LinkId::new([0x31; 16]);

        let connecting = handle.connect_remote_control_target(target);
        let driving = async move {
            let Some(RemoteControlTargetAccessCommand::ResolveTarget {
                target: submitted,
                completion,
            }) = target_accesses.receive().await
            else {
                panic!("resolve target command")
            };
            assert_eq!(submitted, target);
            assert!(completion.send(Ok(resolved)).is_ok());

            let Some(HostCommand::AwaitedEngine { issued, completion }) = command_rx.recv().await
            else {
                panic!("establish link command")
            };
            assert_eq!(
                issued.command,
                PrnsCommand::EstablishLink(EstablishLink { destination }),
            );
            assert!(completion
                .send(Settlement::EstablishLink(Ok(LinkEstablished {
                    link_id,
                    rtt_millis: 17,
                })))
                .is_ok());

            let Some(HostCommand::AwaitedEngine { issued, completion }) = command_rx.recv().await
            else {
                panic!("identify command")
            };
            assert_eq!(
                issued.command,
                PrnsCommand::Identify(Identify {
                    link_id,
                    identity: controller,
                }),
            );
            assert!(completion.send(Settlement::Identify(Ok(()))).is_ok());
            command_rx
        };
        let (connected, mut command_rx) = tokio::join!(connecting, driving);
        let connected = connected.unwrap();

        assert_eq!(connected.connection().target(), target);
        assert_eq!(connected.connection().link_id(), link_id);
        assert_eq!(
            connected.announce_self().await,
            Err(RemoteControlTargetOperationError::NotPermitted(
                RemoteControlRequestKind::AnnounceSelf,
            )),
        );
        assert!(matches!(
            command_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty),
        ));
    }

    #[tokio::test]
    async fn identification_failure_queues_link_cleanup_and_preserves_the_failure() {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let (handle, mut target_accesses) =
            PrnsNodeHandle::over_with_remote_control_target_access_lane(commands);
        let (target, destination, controller, resolved) = resolved_target();
        let link_id = LinkId::new([0x41; 16]);

        let connecting = handle.connect_remote_control_target(target);
        let driving = async move {
            let Some(RemoteControlTargetAccessCommand::ResolveTarget { completion, .. }) =
                target_accesses.receive().await
            else {
                panic!("resolve target command")
            };
            assert!(completion.send(Ok(resolved)).is_ok());

            let Some(HostCommand::AwaitedEngine { issued, completion }) = command_rx.recv().await
            else {
                panic!("establish link command")
            };
            assert_eq!(
                issued.command,
                PrnsCommand::EstablishLink(EstablishLink { destination }),
            );
            assert!(completion
                .send(Settlement::EstablishLink(Ok(LinkEstablished {
                    link_id,
                    rtt_millis: 18,
                })))
                .is_ok());

            let Some(HostCommand::AwaitedEngine { issued, completion }) = command_rx.recv().await
            else {
                panic!("identify command")
            };
            assert_eq!(
                issued.command,
                PrnsCommand::Identify(Identify {
                    link_id,
                    identity: controller,
                }),
            );
            assert!(completion
                .send(Settlement::Identify(Err(IdentifyFailure::WriteFailed)))
                .is_ok());

            let Some(HostCommand::Engine(issued)) = command_rx.recv().await else {
                panic!("close link command")
            };
            assert_eq!(
                issued.command,
                PrnsCommand::CloseLink(CloseLink { link_id }),
            );
        };
        let (connected, ()) = tokio::join!(connecting, driving);

        assert_eq!(
            connected.err(),
            Some(ConnectRemoteControlTargetError::Identify(
                crate::runtime::SendError::Failed(IdentifyFailure::WriteFailed),
            )),
        );
    }
}
