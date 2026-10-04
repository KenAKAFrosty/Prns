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

pub trait PrnsInventoryTransport: RemoteControlTargetConnectionTransport + Send + 'static {
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

impl PrnsInventoryTransport for PrnsNodeHandle {
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
