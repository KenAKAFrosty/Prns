use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
    TRAVERSED_NETWORK_BITRATE_ESTIMATE,
};
use crate::routing::links::MAX_LINK_MTU;

/// What a host should claim when it genuinely doesn't know its pipe: a conservative 500 Mbps
/// estimate for traffic that crosses an actual network. The reference guesses 10 Mbps
/// (`TCPClientInterface.BITRATE_GUESS`), which its own tier table maps to an 8 KiB MTU —
/// a 2005 answer; a host that knows its real figure should pass it instead, in either
/// direction.
pub const TCP_BITRATE_ESTIMATE: BitrateBps = TRAVERSED_NETWORK_BITRATE_ESTIMATE;

/// The most this interface can carry per wire packet — the engine's ceiling. What an
/// instance *declares* is its bitrate tier clamped to this.
pub const TCP_HW_MTU_CAP: usize = MAX_LINK_MTU;

/// The declared hardware MTU is the bitrate's tier clamped to the engine ceiling: the
/// in-transit MTU clamp takes interface declarations at face value when a relay books a
/// transported link, so an interface must never promise more than its buffers carry. The
/// day the ceiling rises, every declaration here rises with it for free.
pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::PointToPoint,
    bitrate: TCP_BITRATE_ESTIMATE,
    mtu: MtuPolicy::optimized_from_bitrate(MAX_LINK_MTU),
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
