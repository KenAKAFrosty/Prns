pub mod capabilities;
pub mod connection_state;
pub mod id;
pub mod ifac;
pub mod mac;
pub mod mode;
pub mod substrate;

mod config;
mod definition;
mod framing;
pub mod impls;
mod packet;
mod status;

pub use capabilities::{
    Capabilities, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCapabilitiesError, TransportCapability,
};
pub use connection_state::ConnectionState;
pub use id::InterfaceId;
pub use mac::MacAddress;
pub use mode::InterfaceMode;

pub use config::{
    hardware_mtu_for_bitrate, AirtimeDutyCycle, AnnounceBandwidthCap, AnnounceRateLimit,
    InterfaceConfig,
};
pub use definition::{InterfaceDefinitionView, InterfaceKind};
pub use packet::{InboundPacket, OutboundPacket};
pub use status::{AirtimeUtilization, InterfaceStatus, TransferRates};

pub use framing::rns_serial_framing;
