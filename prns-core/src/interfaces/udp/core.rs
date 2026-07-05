//! The host-agnostic core of the UDP interface: datagram ceilings and the descriptor
//! shape. As with TCP, the wire figures are per-instance — only the host knows its pipe,
//! so the constructor demands a bitrate and the descriptor maps it through the
//! reference's `Interface.optimise_mtu` tier table — but UDP adds a ceiling TCP doesn't
//! have: a frame is one datagram, and IPv4 UDP cannot carry more than
//! [`UDP_DATAGRAM_MAX`] bytes, so no declared MTU may promise past it.

use crate::interfaces::{
    hardware_mtu_for_bitrate, AnnounceBandwidthCap, EgressCapability, IngressCapability,
    InterfaceCapabilities, InterfaceDescriptor, InterfaceId, InterfaceMode, TransportCapability,
};
use crate::routing::links::MAX_LINK_MTU;

/// What a host should claim when it genuinely doesn't know its pipe — the same modern
/// wired-LAN figure as TCP's, for the same reason. The reference guesses 10 Mbps
/// (`UDPInterface.BITRATE_GUESS`).
pub const UDP_BITRATE_GUESS_BPS: u32 = 1_000_000_000;

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
pub fn descriptor(id: InterfaceId, bitrate_bps: u32) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::PointToPoint,
        hardware_mtu: hardware_mtu_for_bitrate(bitrate_bps).map(|tier| tier.min(UDP_HW_MTU_CAP)),
        announce_rate_limit: None,
        bitrate_bps: Some(bitrate_bps),
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
}
