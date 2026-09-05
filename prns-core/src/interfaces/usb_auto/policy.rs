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
    gravity: crate::interfaces::InterfaceGravityDefault::FromBitrate,
    announce_rate_limit: None,
    bitrate: HOST_USB_BITRATE_BPS,
    mtu: MtuPolicy::fixed(HOST_USB_HW_MTU),
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};

pub fn device_descriptor(id: InterfaceId, bitrate: BitrateBps) -> InterfaceDescriptor {
    DEVICE_DEFAULTS
        .configured(ConfiguredInterfacePolicy {
            bitrate: Some(bitrate),
            ..ConfiguredInterfacePolicy::default()
        })
        .descriptor(id)
}

pub const DEVICE_DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::PointToPoint,
    gravity: crate::interfaces::InterfaceGravityDefault::FromBitrate,
    announce_rate_limit: None,
    bitrate: DEVICE_USB_BITRATE_BPS,
    mtu: MtuPolicy::fixed(DEVICE_USB_HW_MTU),
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_descriptor_uses_the_explicit_link_bitrate() {
        let id = InterfaceId::new([0xA5; 8]);
        let bitrate = BitrateBps::guess(92_160);

        let descriptor = device_descriptor(id, bitrate);

        assert_eq!(descriptor.bitrate, bitrate);
        assert_eq!(descriptor.hardware_mtu, Some(DEVICE_USB_HW_MTU));
        assert_eq!(descriptor.capabilities, DEVICE_DEFAULTS.capabilities);
    }

    #[test]
    fn native_usb_default_retains_the_full_speed_bulk_ceiling() {
        assert_eq!(DEVICE_USB_BITRATE_BPS.get(), 9_728_000);
    }
}
