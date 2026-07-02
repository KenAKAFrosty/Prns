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

#[cfg(any(
    feature = "tokio-host",
    feature = "embassy-contract",
    feature = "bluetooth-core"
))]
pub mod bluetooth_auto;
#[cfg(feature = "embassy-contract")]
pub mod esp_now;
#[cfg(feature = "tokio-host")]
pub mod framed_stream;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod usb_auto;

#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod ax25_kiss;
#[cfg(feature = "tcp")]
pub mod backbone;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod kiss;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod local;
pub mod lora;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod pipe;
#[cfg(feature = "tokio-host")]
pub mod rnode;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod serial;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod tcp;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod udp;
#[cfg(feature = "websocket-core")]
pub mod websocket;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub mod wifi_auto;

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
pub use status::{
    AirtimeUtilization, InterfaceSnapshot, InterfaceStatus, InterfaceVitals, Membership,
    TransferRates,
};
#[cfg(feature = "tokio-host")]
pub use status::{ReportsStatus, StatusView};

pub use framing::{kiss_framing, rns_serial_framing};
