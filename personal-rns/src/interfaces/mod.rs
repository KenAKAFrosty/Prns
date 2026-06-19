pub mod capabilities;
pub mod connection_state;
pub mod id;
pub mod ifac;
pub mod kind;
pub mod mac;
pub mod mode;
pub mod substrate;

mod config;
mod framing;
pub mod impls;
mod packet;
mod status;

#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod bluetooth_auto;
#[cfg(feature = "tokio-host")]
pub mod framed_stream;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod rns_parity;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod usb_auto;

pub use capabilities::{
    Capabilities, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCapabilitiesError, TransportCapability,
};
pub use connection_state::ConnectionState;
pub use id::{InterfaceId, INTERFACE_ID_LEN};
pub use kind::InterfaceKind;
pub use mac::MacAddress;
pub use mode::InterfaceMode;

pub use config::{
    hardware_mtu_for_bitrate, AirtimeDutyCycle, AnnounceBandwidthCap, AnnounceRateLimit,
    InterfaceConfig,
};
pub use packet::{InboundPacket, OutboundPacket};
pub use status::{AirtimeUtilization, InterfaceSnapshot, InterfaceStatus, TransferRates};
#[cfg(feature = "tokio-host")]
pub use status::{ReportsStatus, StatusView};

pub use framing::rns_serial_framing;
