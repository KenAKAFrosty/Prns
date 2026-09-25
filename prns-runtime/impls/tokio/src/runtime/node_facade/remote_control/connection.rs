use crate::engine::{EstablishLinkFailure, IdentifyFailure};
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::links::LinkId;
use crate::runtime::{
    CloseRemoteControlTargetOutcome, ConnectRemoteControlTargetError, RemoteControlAnnounceSelf,
    RemoteControlDescribe, RemoteControlDescribeBuild, RemoteControlDescribePower,
    RemoteControlInventoryInterfaces, RemoteControlSleepRadios, RemoteControlTargetConnection,
    RemoteControlTargetConnectionControl, RemoteControlTargetConnectionTransport,
    RemoteControlTargetOperationError, RemoteControlWakeRadios, SendError,
};
use crate::units::RttMillis;
use crate::wire::DestinationHash;
use prns_core::capabilities::power::PowerSnapshot;
use prns_core::interfaces::InterfaceMode;
use prns_core::remote_control::{
    RemoteControlApplyOutcome, RemoteControlAuthorizeControllerOutcome, RemoteControlBuildVersion,
    RemoteControlControllerIdentity, RemoteControlControllerInventory, RemoteControlControllerPage,
    RemoteControlDescription, RemoteControlDiscoveryGroups,
    RemoteControlDiscoveryGroupsInventoryOutcome, RemoteControlDiscoveryGroupsReplaceOutcome,
    RemoteControlDisplayAutoOff, RemoteControlDisplayVisibility, RemoteControlEspRadioMode,
    RemoteControlGnssPower, RemoteControlGroupOutcome, RemoteControlInterfaceConfigOutcome,
    RemoteControlInterfaceGroup, RemoteControlInterfaceInventory, RemoteControlInterfacePage,
    RemoteControlInterfacePeersOutcome, RemoteControlInterfacePower, RemoteControlLoRaOutcome,
    RemoteControlLoRaProfile, RemoteControlModeOutcome, RemoteControlPeerPage,
    RemoteControlPowerOutcome, RemoteControlRequestKind, RemoteControlRequestSet,
    RemoteControlRevokeControllerOutcome, RemoteControlSleepOutcome, RemoteControlStationUplink,
    RemoteControlSystemPower, RemoteControlWifiCredentialRevision, RemoteControlWifiStageOutcome,
    RemoteControlWifiStation, RemoteControlWifiStationOutcome, RemoteControlWifiTransactionStatus,
};

use super::{PrnsNodeHandle, RemoteControlHandle};

pub struct RemoteControlTargetHandle<'a> {
    remote_control: RemoteControlHandle<'a>,
    connection: RemoteControlTargetConnection,
}

macro_rules! remote_control_target_apply_method {
    ($method:ident, $kind:ident, $field:ident, $field_type:ty) => {
        pub async fn $method(
            &self,
            $field: $field_type,
        ) -> Result<(RemoteControlApplyOutcome, RttMillis), RemoteControlTargetOperationError> {
            self.connection.admit(RemoteControlRequestKind::$kind)?;
            self.remote_control
                .$method($field)
                .await
                .map_err(Into::into)
        }
    };
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

    pub async fn connect_remote_control_target_resolved(
        &self,
        resolved: crate::runtime::ResolvedRemoteControlTarget,
    ) -> Result<RemoteControlTargetHandle<'_>, ConnectRemoteControlTargetError> {
        let connection = self
            .establish_remote_control_target_resolved(resolved)
            .await?;
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
    remote_control_target_apply_method!(
        set_system_power,
        SetSystemPower,
        power,
        RemoteControlSystemPower
    );
    remote_control_target_apply_method!(
        set_gnss_power,
        SetGnssPower,
        power,
        RemoteControlGnssPower
    );
    remote_control_target_apply_method!(
        set_display_visibility,
        SetDisplayVisibility,
        visibility,
        RemoteControlDisplayVisibility
    );
    remote_control_target_apply_method!(
        set_display_auto_off,
        SetDisplayAutoOff,
        auto_off,
        RemoteControlDisplayAutoOff
    );
    remote_control_target_apply_method!(
        set_esp_radio_mode,
        SetEspRadioMode,
        mode,
        RemoteControlEspRadioMode
    );
    remote_control_target_apply_method!(
        activate_wifi_credentials,
        ActivateWifiCredentials,
        revision,
        RemoteControlWifiCredentialRevision
    );
    remote_control_target_apply_method!(
        confirm_wifi_credentials,
        ConfirmWifiCredentials,
        revision,
        RemoteControlWifiCredentialRevision
    );
    remote_control_target_apply_method!(
        cancel_wifi_credentials,
        CancelWifiCredentials,
        revision,
        RemoteControlWifiCredentialRevision
    );

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

    pub async fn describe_build(
        &self,
    ) -> Result<(RemoteControlBuildVersion, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlDescribeBuild::REQUEST.kind())?;
        self.remote_control
            .describe_build()
            .await
            .map_err(Into::into)
    }

    pub async fn describe_power(
        &self,
    ) -> Result<(PowerSnapshot, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlDescribePower::REQUEST.kind())?;
        self.remote_control
            .describe_power()
            .await
            .map_err(Into::into)
    }

    pub async fn inventory_interfaces(
        &self,
    ) -> Result<(RemoteControlInterfaceInventory, RttMillis), RemoteControlTargetOperationError>
    {
        self.connection
            .admit(RemoteControlInventoryInterfaces::REQUEST.kind())?;
        self.remote_control
            .inventory_interfaces()
            .await
            .map_err(Into::into)
    }

    pub async fn inventory_interfaces_page(
        &self,
        page: RemoteControlInterfacePage,
    ) -> Result<(RemoteControlInterfaceInventory, RttMillis), RemoteControlTargetOperationError>
    {
        self.connection
            .admit(RemoteControlInventoryInterfaces::REQUEST.kind())?;
        self.remote_control
            .inventory_interfaces_page(page)
            .await
            .map_err(Into::into)
    }

    pub async fn set_interface_power(
        &self,
        id: InterfaceId,
        power: RemoteControlInterfacePower,
    ) -> Result<(RemoteControlPowerOutcome, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlRequestKind::SetInterfacePower)?;
        self.remote_control
            .set_interface_power(id, power)
            .await
            .map_err(Into::into)
    }

    pub async fn set_interface_mode(
        &self,
        id: InterfaceId,
        mode: InterfaceMode,
    ) -> Result<(RemoteControlModeOutcome, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlRequestKind::SetInterfaceMode)?;
        self.remote_control
            .set_interface_mode(id, mode)
            .await
            .map_err(Into::into)
    }

    pub async fn set_interface_group(
        &self,
        id: InterfaceId,
        group: RemoteControlInterfaceGroup,
    ) -> Result<(RemoteControlGroupOutcome, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlRequestKind::SetInterfaceGroup)?;
        self.remote_control
            .set_interface_group(id, group)
            .await
            .map_err(Into::into)
    }

    pub async fn inventory_interface_discovery_groups(
        &self,
        id: InterfaceId,
    ) -> Result<
        (RemoteControlDiscoveryGroupsInventoryOutcome, RttMillis),
        RemoteControlTargetOperationError,
    > {
        self.connection
            .admit(RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups)?;
        self.remote_control
            .inventory_interface_discovery_groups(id)
            .await
            .map_err(Into::into)
    }

    pub async fn replace_interface_discovery_groups(
        &self,
        id: InterfaceId,
        groups: RemoteControlDiscoveryGroups,
    ) -> Result<
        (RemoteControlDiscoveryGroupsReplaceOutcome, RttMillis),
        RemoteControlTargetOperationError,
    > {
        self.connection
            .admit(RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups)?;
        self.remote_control
            .replace_interface_discovery_groups(id, groups)
            .await
            .map_err(Into::into)
    }

    pub async fn set_interface_lora_profile(
        &self,
        id: InterfaceId,
        profile: RemoteControlLoRaProfile,
    ) -> Result<(RemoteControlLoRaOutcome, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlRequestKind::SetInterfaceLoRaProfile)?;
        self.remote_control
            .set_interface_lora_profile(id, profile)
            .await
            .map_err(Into::into)
    }

    pub async fn set_interface_wifi_station(
        &self,
        id: InterfaceId,
        station: RemoteControlWifiStation,
    ) -> Result<(RemoteControlWifiStationOutcome, RttMillis), RemoteControlTargetOperationError>
    {
        self.connection
            .admit(RemoteControlRequestKind::SetInterfaceWifiStation)?;
        self.remote_control
            .set_interface_wifi_station(id, station)
            .await
            .map_err(Into::into)
    }

    pub async fn inventory_controllers(
        &self,
    ) -> Result<(RemoteControlControllerInventory, RttMillis), RemoteControlTargetOperationError>
    {
        self.connection
            .admit(RemoteControlRequestKind::InventoryControllers)?;
        self.remote_control
            .inventory_controllers()
            .await
            .map_err(Into::into)
    }

    pub async fn inventory_controllers_page(
        &self,
        page: RemoteControlControllerPage,
    ) -> Result<(RemoteControlControllerInventory, RttMillis), RemoteControlTargetOperationError>
    {
        self.connection
            .admit(RemoteControlRequestKind::InventoryControllers)?;
        self.remote_control
            .inventory_controllers_page(page)
            .await
            .map_err(Into::into)
    }

    pub async fn authorize_controller(
        &self,
        controller: RemoteControlControllerIdentity,
        permitted_requests: RemoteControlRequestSet,
    ) -> Result<
        (RemoteControlAuthorizeControllerOutcome, RttMillis),
        RemoteControlTargetOperationError,
    > {
        self.connection
            .admit(RemoteControlRequestKind::AuthorizeController)?;
        self.remote_control
            .authorize_controller(controller, permitted_requests)
            .await
            .map_err(Into::into)
    }

    pub async fn revoke_controller(
        &self,
        hash: IdentityHash,
    ) -> Result<(RemoteControlRevokeControllerOutcome, RttMillis), RemoteControlTargetOperationError>
    {
        self.connection
            .admit(RemoteControlRequestKind::RevokeController)?;
        self.remote_control
            .revoke_controller(hash)
            .await
            .map_err(Into::into)
    }

    pub async fn inventory_interface_peers(
        &self,
        id: InterfaceId,
        page: RemoteControlPeerPage,
    ) -> Result<(RemoteControlInterfacePeersOutcome, RttMillis), RemoteControlTargetOperationError>
    {
        self.connection
            .admit(RemoteControlRequestKind::InventoryInterfacePeers)?;
        self.remote_control
            .inventory_interface_peers(id, page)
            .await
            .map_err(Into::into)
    }

    pub async fn inventory_interface_config(
        &self,
        id: InterfaceId,
    ) -> Result<(RemoteControlInterfaceConfigOutcome, RttMillis), RemoteControlTargetOperationError>
    {
        self.connection
            .admit(RemoteControlRequestKind::InventoryInterfaceConfig)?;
        self.remote_control
            .inventory_interface_config(id)
            .await
            .map_err(Into::into)
    }

    pub async fn sleep_radios(
        &self,
    ) -> Result<(RemoteControlSleepOutcome, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlSleepRadios::REQUEST.kind())?;
        self.remote_control.sleep_radios().await.map_err(Into::into)
    }

    pub async fn wake_radios(
        &self,
    ) -> Result<(RemoteControlSleepOutcome, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlWakeRadios::REQUEST.kind())?;
        self.remote_control.wake_radios().await.map_err(Into::into)
    }

    pub async fn set_station_uplink(
        &self,
        id: InterfaceId,
        uplink: RemoteControlStationUplink,
    ) -> Result<(RemoteControlApplyOutcome, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlRequestKind::SetStationUplink)?;
        self.remote_control
            .set_station_uplink(id, uplink)
            .await
            .map_err(Into::into)
    }

    pub async fn stage_wifi_credentials(
        &self,
        station: RemoteControlWifiStation,
    ) -> Result<(RemoteControlWifiStageOutcome, RttMillis), RemoteControlTargetOperationError> {
        self.connection
            .admit(RemoteControlRequestKind::StageWifiCredentials)?;
        self.remote_control
            .stage_wifi_credentials(station)
            .await
            .map_err(Into::into)
    }

    pub async fn inspect_wifi_transaction(
        &self,
    ) -> Result<(RemoteControlWifiTransactionStatus, RttMillis), RemoteControlTargetOperationError>
    {
        self.connection
            .admit(RemoteControlRequestKind::InspectWifiTransaction)?;
        self.remote_control
            .inspect_wifi_transaction()
            .await
            .map_err(Into::into)
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
            crate::remote_control::RemoteControlControllerAuthority::Operator,
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
            crate::remote_control::RemoteControlControllerAuthority::Operator,
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
                    destination: crate::wire::DestinationHash::new([0; 16]),
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
                    destination: crate::wire::DestinationHash::new([0; 16]),
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
