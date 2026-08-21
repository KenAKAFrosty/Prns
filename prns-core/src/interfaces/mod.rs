mod descriptor;
mod framing;
mod identity;
mod packet;
mod policy;
mod status;

pub mod ax25_kiss;
pub mod backbone;
pub mod bluetooth_auto;
pub mod browser_rendezvous;
pub mod channel_rendezvous;
pub mod esp_now;
pub mod i2p;
pub mod kiss;
pub mod local_network;
pub mod lora;
pub mod pipe;
pub mod rnode;
#[cfg(feature = "shared-instance-rpc")]
pub mod rns_management;
pub mod serial;
pub mod shared_instance;
pub mod tcp;
pub mod udp;
pub mod usb_auto;
pub mod weave;
pub mod websocket;
pub mod wifi_auto;
pub mod wifi_aware;
pub mod wifi_direct;

#[cfg(feature = "alloc")]
pub use descriptor::IndexedAttachedInterfaces;
pub use descriptor::{
    hardware_mtu_for_bitrate, AttachedInterfaces, BitrateBps, Egress, InterfaceDescriptor,
};
pub use identity::{InterfaceId, InterfaceKind, InterfaceOriginKind, MacAddress, INTERFACE_ID_LEN};
pub use packet::{
    frame_cap_for, IfacContext, IfacMaskError, IfacSize, IfacSizeError, InboundPacket,
    InterfaceIfac, OutboundPacket, PacketPhyStats, RssiDbm, SignalQualityTenthsPercent,
    SnrQuarterDb, BROADCAST_WIRE_FRAME_LEN, DEFAULT_IFAC_SIZE, EMBEDDED_MAX_LINK_MTU,
    EMBEDDED_MAX_WIRE_FRAME_LEN, IFAC_MAX_SIZE, MAX_WIRE_FRAME_LEN,
};
pub use policy::{
    AirtimeDutyCycle, AnnounceBandwidthCap, AnnounceRateLimit, Capabilities,
    ConfiguredInterfacePolicy, EffectiveInterfacePolicy, EgressCapability, FrequencyMilliHertz,
    IngressCapability, IngressControlPolicy, InterfaceCapabilities, InterfaceCapabilitiesError,
    InterfaceCommonPolicy, InterfaceDefaults, InterfaceForwardingPolicy, InterfaceGravity,
    InterfaceMode, MtuBytes, MtuPolicy, PathRequestEgressControl, RecursivePathRequestPolicy,
    TransportCapability, LOCAL_INTERFACE_BITRATE_ESTIMATE, TRAVERSED_NETWORK_BITRATE_ESTIMATE,
};
pub use status::{
    AirtimeUtilization, ConnectionState, InterfaceSnapshot, InterfaceStatus, InterfaceVitals,
    Membership, TransferRates,
};
#[cfg(feature = "tokio-host")]
pub use status::{ConnectionView, ReportsStatus, StatusView};

pub use framing::{kiss_framing, rns_serial_framing, FrameSink, FrameSinkError};
