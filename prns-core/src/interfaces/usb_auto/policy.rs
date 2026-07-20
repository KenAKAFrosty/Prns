use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EgressCapability,
    IngressCapability, InterfaceCapabilities, InterfaceDefaults, InterfaceDescriptor, InterfaceId,
    InterfaceMode, MtuPolicy, TransportCapability,
};

pub const HOST_USB_BITRATE_BPS: BitrateBps = BitrateBps::guess(1_000_000_000);
pub const HOST_USB_HW_MTU: usize = 8_192;
pub const DEVICE_USB_HW_MTU: usize = 8_192;
pub const DEVICE_USB_BITRATE_BPS: BitrateBps = BitrateBps::guess(6_000_000);

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
    announce_rate_limit: None,
    bitrate: DEVICE_USB_BITRATE_BPS,
    mtu: MtuPolicy::fixed(DEVICE_USB_HW_MTU),
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};
