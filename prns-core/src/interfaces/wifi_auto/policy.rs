use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
    LOCAL_INTERFACE_BITRATE_ESTIMATE,
};
use crate::routing::links::MAX_LINK_MTU;

pub const HARDWARE_MTU: usize = 1196;

pub const WIFI_BITRATE_GUESS_BPS: BitrateBps = BitrateBps::guess(10_000_000);

pub const WIFI_LAN_BITRATE_BPS: BitrateBps = LOCAL_INTERFACE_BITRATE_ESTIMATE;

pub const WIFI_EMBEDDED_BITRATE_CEILING_BPS: BitrateBps = BitrateBps::guess(125_000_000);

pub const WIFI_HW_MTU_CAP: usize = if HARDWARE_MTU < MAX_LINK_MTU {
    HARDWARE_MTU
} else {
    MAX_LINK_MTU
};

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::Full,
    bitrate: WIFI_LAN_BITRATE_BPS,
    mtu: MtuPolicy::optimized_from_bitrate(WIFI_HW_MTU_CAP),
    announce_rate_limit: None,
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};

#[must_use]
pub fn configured_policy(configured: ConfiguredInterfacePolicy) -> EffectiveInterfacePolicy {
    DEFAULTS.configured(configured)
}

#[must_use]
pub fn policy_for_bitrate(bitrate: BitrateBps) -> EffectiveInterfacePolicy {
    configured_policy(ConfiguredInterfacePolicy {
        bitrate: Some(bitrate),
        ..ConfiguredInterfacePolicy::default()
    })
}

pub fn descriptor(id: InterfaceId, policy: EffectiveInterfacePolicy) -> InterfaceDescriptor {
    policy.descriptor(id)
}
