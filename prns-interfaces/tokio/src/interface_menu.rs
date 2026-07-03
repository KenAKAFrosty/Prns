//! The menu: every attachable interface this crate offers, curated in one place. Each entry
//! goes through [`attach`](prns_runtime::runtime::TokioPrnsHandle::attach) — single wires and
//! discovery fleets alike — and the feature gates mean the menu you can browse is exactly the
//! menu this build can serve. Fleet members (the per-peer types a fleet stands up itself)
//! are deliberately not on it.

#[cfg(all(
    feature = "auto",
    any(feature = "wifi", feature = "usb-host", feature = "ble-host")
))]
pub use crate::auto::Auto;
#[cfg(feature = "ax25")]
pub use crate::ax25::Ax25KissInterface;
#[cfg(feature = "backbone")]
pub use crate::backbone::client::BackboneClientInterface;
#[cfg(feature = "backbone")]
pub use crate::backbone::server::BackboneServer;
#[cfg(feature = "ble")]
pub use crate::ble::tokio::BluetoothAuto;
#[cfg(feature = "ble-host")]
pub use crate::ble_host::{AttachedBle, AutoBle};
#[cfg(feature = "from-plan")]
pub use crate::from_plan::FromPlan;
#[cfg(feature = "kiss")]
pub use crate::kiss::KissInterface;
#[cfg(feature = "pipe")]
pub use crate::pipe::PipeInterface;
#[cfg(feature = "rnode")]
pub use crate::rnode::RNodeInterface;
#[cfg(feature = "serial")]
pub use crate::serial::SerialInterface;
#[cfg(feature = "shared-instance")]
pub use crate::shared_instance::server::LocalServer;
#[cfg(feature = "tcp")]
pub use crate::tcp::client::TcpClientInterface;
#[cfg(feature = "tcp")]
pub use crate::tcp::server::TcpServer;
#[cfg(feature = "udp")]
pub use crate::udp::UdpInterface;
#[cfg(feature = "usb")]
pub use crate::usb::UsbAutoHost;
#[cfg(feature = "usb-host")]
pub use crate::usb_host::AutoUsb;
#[cfg(feature = "websocket")]
pub use crate::websocket::client::WebSocketClientInterface;
#[cfg(feature = "websocket")]
pub use crate::websocket::server::WebSocketServer;
#[cfg(feature = "wifi")]
pub use crate::wifi::AutoWifi;
#[cfg(feature = "wifi-direct")]
pub use crate::wifi_direct::tokio::WifiDirectAuto;
