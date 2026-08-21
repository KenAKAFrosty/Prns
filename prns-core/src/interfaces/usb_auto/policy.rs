use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EgressCapability,
    IngressCapability, InterfaceCapabilities, InterfaceDefaults, InterfaceDescriptor, InterfaceId,
    InterfaceMode, MtuPolicy, TransportCapability,
};

const FULL_SPEED_BULK_PACKETS_PER_FRAME: u64 = 19;
const FULL_SPEED_BULK_PACKET_BYTES: u64 = 64;
const USB_FRAMES_PER_SECOND: u64 = 1_000;
const FULL_SPEED_BULK_CEILING_BPS: BitrateBps = BitrateBps::guess(
    FULL_SPEED_BULK_PACKETS_PER_FRAME * FULL_SPEED_BULK_PACKET_BYTES * 8 * USB_FRAMES_PER_SECOND,
);

pub const HOST_USB_BITRATE_BPS: BitrateBps = FULL_SPEED_BULK_CEILING_BPS;
pub const HOST_USB_HW_MTU: usize = 8_192;
pub const DEVICE_USB_HW_MTU: usize = 8_192;
pub const DEVICE_USB_BITRATE_BPS: BitrateBps = FULL_SPEED_BULK_CEILING_BPS;

pub fn host_descriptor(id: InterfaceId) -> InterfaceDescriptor {
    HOST_DEFAULTS
        .configured(ConfiguredInterfacePolicy::default())
        .descriptor(id)
}

pub const HOST_DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
    },
    mode: InterfaceMode::PointToPoint,
    gravity: crate::interfaces::InterfaceGravity::ZERO,
    announce_rate_limit: None,
    bitrate: HOST_USB_BITRATE_BPS,
    mtu: MtuPolicy::fixed(HOST_USB_HW_MTU),
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};

pub fn device_descriptor(id: InterfaceId) -> InterfaceDescriptor {
    DEVICE_DEFAULTS
        .configured(ConfiguredInterfacePolicy::default())
        .descriptor(id)
}

pub const DEVICE_DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::PointToPoint,
    gravity: crate::interfaces::InterfaceGravity::ZERO,
    announce_rate_limit: None,
    bitrate: DEVICE_USB_BITRATE_BPS,
    mtu: MtuPolicy::fixed(DEVICE_USB_HW_MTU),
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};
