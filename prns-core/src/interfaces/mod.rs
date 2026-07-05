pub mod capabilities;
pub mod connection_state;
pub mod id;
pub mod ifac;
pub mod kind;
pub mod mac;
pub mod mode;

mod bitrate;
mod descriptor;
mod framing;
mod packet;
mod status;

pub mod ax25_kiss;
pub mod backbone;
pub mod bluetooth_auto;
pub mod channel_rendezvous;
pub mod esp_now;
pub mod kiss;
pub mod lora;
pub mod pipe;
pub mod radios;
#[cfg(feature = "std")]
pub mod rnode;
pub mod serial;
pub mod shared_instance;
pub mod tcp;
pub mod udp;
pub mod usb_auto;
pub mod websocket;
pub mod wifi_auto;
pub mod wifi_aware;
pub mod wifi_direct;

pub use capabilities::{
    Capabilities, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCapabilitiesError, TransportCapability,
};
pub use connection_state::ConnectionState;
pub use id::{InterfaceId, INTERFACE_ID_LEN};
pub use kind::InterfaceKind;
pub use mac::MacAddress;
pub use mode::InterfaceMode;

pub use bitrate::BitrateBps;
pub use descriptor::{
    hardware_mtu_for_bitrate, AirtimeDutyCycle, AnnounceBandwidthCap, AnnounceRateLimit,
    InterfaceDescriptor,
};
pub use packet::{InboundPacket, OutboundPacket};
pub use status::{
    AirtimeUtilization, InterfaceSnapshot, InterfaceStatus, InterfaceVitals, Membership,
    TransferRates,
};
#[cfg(feature = "tokio-host")]
pub use status::{ReportsStatus, StatusView};

pub use framing::{kiss_framing, rns_serial_framing};
