use core::future::Future;
use personal_rns::runtime::{ConnectRemoteControlTargetError, RequestPathError};
use personal_rns::wire::DestinationHash;

#[derive(Debug, PartialEq, Eq)]
pub enum PrnsConnectionError {
    Path(RequestPathError),
    Target(ConnectRemoteControlTargetError),
}
use personal_rns::remote_control::{RemoteControlInterfaceInventory, RemoteControlInterfacePage};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{
    PrnsNodeHandle, RemoteControlError, RemoteControlTargetConnectionTransport,
};
use personal_rns::units::RttMillis;

use hopspot_hub_core::DeviceControlCommand;
use personal_rns::remote_control::RemoteControlApplyOutcome;

pub trait PrnsDeviceTransport: RemoteControlTargetConnectionTransport + Send + 'static {
    fn control(
        &self,
        link: LinkId,
        command: DeviceControlCommand,
    ) -> impl Future<Output = Result<(RemoteControlApplyOutcome, RttMillis), RemoteControlError>> + Send;

    fn discover_target(
        &self,
        destination: DestinationHash,
    ) -> impl Future<Output = Result<(), RequestPathError>> + Send;

    fn inventory_interfaces(
        &self,
        link: LinkId,
        page: RemoteControlInterfacePage,
    ) -> impl Future<
        Output = Result<(RemoteControlInterfaceInventory, RttMillis), RemoteControlError>,
    > + Send;
}

impl PrnsDeviceTransport for PrnsNodeHandle {
    async fn control(
        &self,
        link: LinkId,
        command: DeviceControlCommand,
    ) -> Result<(RemoteControlApplyOutcome, RttMillis), RemoteControlError> {
        let remote = self.remote_control(link);
        match command {
            DeviceControlCommand::DisplayVisibility(visibility) => {
                remote.set_display_visibility(visibility).await
            }
            DeviceControlCommand::GnssPower(power) => remote.set_gnss_power(power).await,
        }
    }

    async fn discover_target(&self, destination: DestinationHash) -> Result<(), RequestPathError> {
        self.request_path(destination).await.map(|_| ())
    }

    async fn inventory_interfaces(
        &self,
        link: LinkId,
        page: RemoteControlInterfacePage,
    ) -> Result<(RemoteControlInterfaceInventory, RttMillis), RemoteControlError> {
        self.remote_control(link)
            .inventory_interfaces_page(page)
            .await
    }
}
