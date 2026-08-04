use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
    TRAVERSED_NETWORK_BITRATE_ESTIMATE,
};
use crate::routing::links::MAX_LINK_MTU;

pub const UDP_BITRATE_ESTIMATE: BitrateBps = TRAVERSED_NETWORK_BITRATE_ESTIMATE;

/// IPv4 UDP's hard payload ceiling: 65,535 minus the 20-byte IP and 8-byte UDP headers.
pub const UDP_DATAGRAM_MAX: usize = 65_507;

pub const UDP_HW_MTU_CAP: usize = if MAX_LINK_MTU < UDP_DATAGRAM_MAX {
    MAX_LINK_MTU
} else {
    UDP_DATAGRAM_MAX
};

/// Any legal IPv4 datagram fits so an oversized peer frame reaches the engine intact rather than being truncated at the socket.
pub const RECV_BUF_LEN: usize = 65_535;

/// The declared MTU is clamped to [`UDP_DATAGRAM_MAX`] because the in-transit MTU clamp takes interface declarations at face value.
pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::PointToPoint,
    gravity: crate::interfaces::InterfaceGravity::ZERO,
    bitrate: UDP_BITRATE_ESTIMATE,
    mtu: MtuPolicy::optimized_from_bitrate(UDP_HW_MTU_CAP),
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
