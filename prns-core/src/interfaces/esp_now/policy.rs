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
pub fn descriptor(id: InterfaceId, bitrate: BitrateBps) -> InterfaceDescriptor {
    policy_for_bitrate(bitrate).descriptor(id)
}

#[must_use]
pub fn policy_for_bitrate(bitrate: BitrateBps) -> crate::interfaces::EffectiveInterfacePolicy {
    DEFAULTS.configured(ConfiguredInterfacePolicy {
        bitrate: Some(bitrate),
        ..ConfiguredInterfacePolicy::default()
    })
}

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
    },
    mode: InterfaceMode::Full,
    gravity: crate::interfaces::InterfaceGravity::ZERO,
    bitrate: ESP_NOW_BITRATE_BPS,
    mtu: MtuPolicy::fixed(ESP_NOW_HW_MTU),
    announce_rate_limit: None,
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_uses_the_selected_phy_bitrate() {
        let id = InterfaceId::new([9; 8]);
        let bitrate = BitrateBps::guess(12_000_000);

        assert_eq!(descriptor(id, bitrate).bitrate, bitrate);
    }
}
