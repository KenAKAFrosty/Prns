use embassy_sync::blocking_mutex::raw::RawMutex;

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
    RemoteControlAuthorizeControllerOutcome, RemoteControlBuildVersion,
    RemoteControlControllerIdentity, RemoteControlControllerInventory, RemoteControlDescription,
    RemoteControlGroupOutcome, RemoteControlInterfaceConfigOutcome, RemoteControlInterfaceGroup,
    RemoteControlInterfaceInventory, RemoteControlInterfacePeersOutcome,
    RemoteControlInterfacePower, RemoteControlLoRaOutcome, RemoteControlLoRaProfile,
    RemoteControlModeOutcome, RemoteControlPowerOutcome, RemoteControlRequestKind,
    RemoteControlRevokeControllerOutcome, RemoteControlSleepOutcome, RemoteControlWifiStation,
    RemoteControlWifiStationOutcome,
};

use super::{PrnsNodeHandle, RemoteControlHandle};

pub struct RemoteControlTargetHandle<
    'a,
    M: RawMutex,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
> {
    remote_control:
        RemoteControlHandle<'a, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    connection: RemoteControlTargetConnection,
}

impl<
        'a,
        M: RawMutex + Sync,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > PrnsNodeHandle<'a, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    pub async fn connect_remote_control_target(
        &self,
        target: IdentityHash,
    ) -> Result<
        RemoteControlTargetHandle<
            'a,
            M,
            COMMANDS,
            COMPLETIONS,
            REQUEST_COMPLETIONS,
            RESPONSE_BYTES,
        >,
        ConnectRemoteControlTargetError,
    > {
        let connection = self.establish_remote_control_target(target).await?;
        Ok(RemoteControlTargetHandle {
            remote_control: self.remote_control(connection.link_id()),
            connection,
        })
    }
}

impl<
        M: RawMutex + Sync,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > RemoteControlTargetConnectionTransport
    for PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
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

impl<
        M: RawMutex + Sync,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > RemoteControlTargetHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
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

    pub async fn authorize_controller(
        &self,
        controller: RemoteControlControllerIdentity,
    ) -> Result<
        (RemoteControlAuthorizeControllerOutcome, RttMillis),
        RemoteControlTargetOperationError,
    > {
        self.connection
            .admit(RemoteControlRequestKind::AuthorizeController)?;
        self.remote_control
            .authorize_controller(controller)
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
        offset: u8,
    ) -> Result<(RemoteControlInterfacePeersOutcome, RttMillis), RemoteControlTargetOperationError>
    {
        self.connection
            .admit(RemoteControlRequestKind::InventoryInterfacePeers)?;
        self.remote_control
            .inventory_interface_peers(id, offset)
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
}
