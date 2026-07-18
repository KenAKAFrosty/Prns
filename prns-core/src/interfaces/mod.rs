pub mod capabilities;
pub mod connection_state;
pub mod id;
pub mod ifac;
pub mod kind;
pub mod mac;
pub mod mode;
pub mod wire_limits;

mod attached;
mod bitrate;
mod descriptor;
mod framing;
mod origin;
mod packet;
mod policy;
mod status;

pub mod ax25_kiss;
pub mod backbone;
pub mod bluetooth_auto;
pub mod channel_rendezvous;
pub mod esp_now;
pub mod i2p;
pub mod kiss;
pub mod lora;
pub mod pipe;
pub mod radios;
pub mod rnode;
#[cfg(feature = "shared-instance-rpc")]
pub mod rns_management;
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
pub use origin::InterfaceOriginKind;

#[cfg(feature = "alloc")]
pub use attached::IndexedAttachedInterfaces;
pub use attached::{AttachedInterfaces, Egress};
pub use bitrate::BitrateBps;
pub use descriptor::{
    hardware_mtu_for_bitrate, AirtimeDutyCycle, AnnounceBandwidthCap, AnnounceRateLimit,
    FrequencyMilliHertz, IngressControlPolicy, InterfaceCommonPolicy, InterfaceDescriptor,
    InterfaceForwardingPolicy, PathRequestEgressControl,
};
pub use packet::{
    InboundPacket, OutboundPacket, PacketPhyStats, RssiDbm, SignalQualityTenthsPercent,
    SnrQuarterDb,
};
pub use policy::{
    ConfiguredInterfacePolicy, EffectiveInterfacePolicy, InterfaceDefaults, MtuBytes, MtuPolicy,
    LOCAL_INTERFACE_BITRATE_ESTIMATE, TRAVERSED_NETWORK_BITRATE_ESTIMATE,
};
pub use status::{
    AirtimeUtilization, InterfaceSnapshot, InterfaceStatus, InterfaceVitals, Membership,
    TransferRates,
};
#[cfg(feature = "tokio-host")]
pub use status::{ConnectionView, ReportsStatus, StatusView};

pub use framing::{kiss_framing, rns_serial_framing, FrameSink, FrameSinkError};
