//! The menu: every attachable family's one [`Attachable`](prns_runtime::runtime::Attachable)
//! registration, so `handle.attach(x)` works on any of them and the wire-vs-fleet
//! distinction lives here instead of at callsites.

#[cfg(any(
    feature = "tcp",
    feature = "udp",
    feature = "websocket",
    feature = "backbone",
    feature = "serial",
    feature = "kiss",
    feature = "ax25",
    feature = "pipe",
    feature = "rnode",
    feature = "usb"
))]
macro_rules! attaches_as_wire {
    (impl[$($generics:tt)*] $ty:ty) => {
        impl<$($generics)*> prns_runtime::runtime::Attachable for $ty
        where
            Self: prns_runtime::reactor::interface_seam::Interface
                + prns_runtime::interfaces::ReportsStatus
                + Send
                + 'static,
        {
            type Attached = prns_runtime::runtime::AttachedInterface;
            fn attach_to(
                self,
                handle: &prns_runtime::runtime::PrnsNodeHandle,
            ) -> prns_runtime::runtime::AttachedInterface {
                handle.add_interface(self)
            }

            fn attach_to_with_ifac(
                self,
                handle: &prns_runtime::runtime::PrnsNodeHandle,
                ifac: prns_core::interfaces::ifac::IfacContext,
                network_name: Option<std::string::String>,
            ) -> prns_runtime::runtime::AttachedInterface {
                handle.add_interface_with_ifac_name(self, ifac, network_name)
            }
        }
    };
}

#[cfg(any(
    feature = "wifi",
    feature = "tcp",
    feature = "websocket",
    feature = "shared-instance",
    feature = "backbone",
    feature = "wifi-direct",
    feature = "ble"
))]
macro_rules! attaches_as_fleet {
    (impl[$($generics:tt)*] $ty:ty) => {
        impl<$($generics)*> prns_runtime::runtime::Attachable for $ty
        where
            Self: prns_runtime::runtime::InterfaceSupervisor
                + prns_runtime::interfaces::ReportsStatus
                + Send
                + 'static,
        {
            type Attached = prns_runtime::runtime::AttachedSupervisor;
            fn attach_to(
                self,
                handle: &prns_runtime::runtime::PrnsNodeHandle,
            ) -> prns_runtime::runtime::AttachedSupervisor {
                handle.supervise(self)
            }

            fn attach_to_with_ifac(
                self,
                handle: &prns_runtime::runtime::PrnsNodeHandle,
                ifac: prns_core::interfaces::ifac::IfacContext,
                network_name: Option<std::string::String>,
            ) -> prns_runtime::runtime::AttachedSupervisor {
                handle.supervise_with_ifac_name(self, ifac, network_name)
            }
        }
    };
}

#[cfg(feature = "tcp")]
attaches_as_wire!(impl[] crate::tcp::client::TcpClientInterface);
#[cfg(feature = "udp")]
attaches_as_wire!(impl[] crate::udp::UdpInterface);
#[cfg(feature = "websocket")]
attaches_as_wire!(impl[] crate::websocket::client::WebSocketClientInterface);
#[cfg(feature = "backbone")]
attaches_as_wire!(impl[] crate::backbone::client::BackboneClientInterface);
#[cfg(feature = "serial")]
attaches_as_wire!(impl[Open] crate::serial::SerialInterface<Open>);
#[cfg(feature = "kiss")]
attaches_as_wire!(impl[Open] crate::kiss::KissInterface<Open>);
#[cfg(feature = "ax25")]
attaches_as_wire!(impl[Open] crate::ax25::Ax25KissInterface<Open>);
#[cfg(feature = "pipe")]
attaches_as_wire!(impl[Open] crate::pipe::PipeInterface<Open>);
#[cfg(feature = "rnode")]
attaches_as_wire!(impl[Open] crate::rnode::RNodeInterface<Open>);
#[cfg(feature = "usb")]
attaches_as_wire!(impl[Scan, Open] crate::usb::UsbAutoHost<Scan, Open>);

#[cfg(feature = "wifi")]
attaches_as_fleet!(impl[] crate::wifi::AutoWifi);
#[cfg(feature = "tcp")]
attaches_as_fleet!(impl[] crate::tcp::server::TcpServer);
#[cfg(feature = "websocket")]
attaches_as_fleet!(impl[] crate::websocket::server::WebSocketServer);
#[cfg(feature = "shared-instance")]
attaches_as_fleet!(impl[] crate::shared_instance::server::LocalServer);
#[cfg(feature = "backbone")]
attaches_as_fleet!(impl[] crate::backbone::server::BackboneServer);
#[cfg(feature = "wifi-direct")]
attaches_as_fleet!(impl[B] crate::wifi_direct::tokio::WifiDirectAuto<B>);
#[cfg(feature = "ble")]
attaches_as_fleet!(impl[B, const MAX_PEERS: usize] crate::ble::tokio::BluetoothAuto<B, MAX_PEERS>);
