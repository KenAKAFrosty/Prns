use core::future::Future;
use personal_rns::remote_control::{RemoteControlInterfaceInventory, RemoteControlInterfacePage};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{
    PrnsNodeHandle, RemoteControlError, RemoteControlTargetConnectionTransport,
};
use personal_rns::units::RttMillis;

pub trait PrnsInventoryTransport: RemoteControlTargetConnectionTransport + Send + 'static {
    fn inventory_interfaces(
        &self,
        link: LinkId,
        page: RemoteControlInterfacePage,
    ) -> impl Future<
        Output = Result<(RemoteControlInterfaceInventory, RttMillis), RemoteControlError>,
    > + Send;
}

impl PrnsInventoryTransport for PrnsNodeHandle {
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
