//! The host-agnostic core of the UDP interface: datagram ceilings and the descriptor
//! shape. As with TCP, the wire figures are per-instance — only the host knows its pipe,
//! so the constructor demands a bitrate and the descriptor maps it through the
//! reference's `Interface.optimise_mtu` tier table — but UDP adds a ceiling TCP doesn't
//! have: a frame is one datagram, and IPv4 UDP cannot carry more than
//! [`UDP_DATAGRAM_MAX`] bytes, so no declared MTU may promise past it.

use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
    TRAVERSED_NETWORK_BITRATE_ESTIMATE,
};
use crate::routing::links::MAX_LINK_MTU;

/// What a host should claim when it genuinely doesn't know its pipe — the same modern
/// wired-LAN figure as TCP's, for the same reason. The reference guesses 10 Mbps
/// (`UDPInterface.BITRATE_GUESS`).
pub const UDP_BITRATE_ESTIMATE: BitrateBps = TRAVERSED_NETWORK_BITRATE_ESTIMATE;

/// IPv4 UDP's hard payload ceiling: 65,535 minus the 20-byte IP and 8-byte UDP headers.
/// One frame is one datagram, so this is a protocol law, not a tuning choice.
pub const UDP_DATAGRAM_MAX: usize = 65_507;

/// The most this interface can carry per wire packet: the engine's ceiling, clamped by
/// the datagram law above.
pub const UDP_HW_MTU_CAP: usize = if MAX_LINK_MTU < UDP_DATAGRAM_MAX {
    MAX_LINK_MTU
} else {
    UDP_DATAGRAM_MAX
};

/// Capacity, not a claim: any datagram IPv4 can deliver fits, so an oversized peer frame
/// arrives intact and is judged by the engine, never truncated at the socket.
pub const RECV_BUF_LEN: usize = 65_535;

/// The declared hardware MTU is the bitrate's tier clamped to the datagram-bounded
/// ceiling: the in-transit MTU clamp takes interface declarations at face value, and a
/// UDP interface physically cannot carry a frame past [`UDP_DATAGRAM_MAX`].
pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::PointToPoint,
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
