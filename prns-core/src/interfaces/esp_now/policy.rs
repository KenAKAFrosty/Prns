use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EgressCapability,
    IngressCapability, InterfaceCapabilities, InterfaceDefaults, InterfaceDescriptor, InterfaceId,
    InterfaceMode, MtuPolicy, TransportCapability, IFAC_MAX_SIZE,
};

use super::protocol::ESP_NOW_V2_AIR_MTU;

/// The clean-packet MTU we declare: the air ceiling less the largest access tag, so a full frame plus its IFAC code still fits one ESP-NOW datagram.
pub const ESP_NOW_HW_MTU: usize = ESP_NOW_V2_AIR_MTU - IFAC_MAX_SIZE;
/// A representative broadcast goodput for announce pacing and the MTU tier — an honest order of magnitude for the carrier, not a measured peak.
pub const ESP_NOW_BITRATE_BPS: BitrateBps = BitrateBps::guess(1_000_000);

#[must_use]
pub fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    DEFAULTS
        .configured(ConfiguredInterfacePolicy::default())
        .descriptor(id)
}

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
    },
    mode: InterfaceMode::Full,
    bitrate: ESP_NOW_BITRATE_BPS,
    mtu: MtuPolicy::fixed(ESP_NOW_HW_MTU),
    announce_rate_limit: None,
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};
