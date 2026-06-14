//! The host-agnostic core of the TCP interface: frame *capacities* and the descriptor
//! shape. The wire figures — bitrate, and the hardware MTU derived from it — are
//! per-instance: only the host knows its pipe, so the constructors demand a bitrate and
//! the descriptor maps it through the reference's `Interface.optimise_mtu` tier table
//! ([`hardware_mtu_for_bitrate`]), the same autoconfiguration the reference runs on every
//! TCP interface. The buffers here are sized to the engine's own ceiling instead — a
//! capacity, not a claim — so every frame any negotiable MTU can produce already fits.

use crate::interfaces::rns_serial_framing;
use crate::interfaces::{
    hardware_mtu_for_bitrate, AnnounceBandwidthCap, EgressCapability, IngressCapability,
    InterfaceCapabilities, InterfaceConfig, InterfaceId, InterfaceMode, TransportCapability,
};
use crate::reactor::interface_seam::MAX_WIRE_FRAME_LEN;
use crate::routing::links::MAX_LINK_MTU;

/// One socket read's worth. The reference uses 4 KiB, which is fine for slow links
/// but makes local-gigabit resource frames trickle through hundreds of userspace
/// reads. Keep this TCP-only; serial's read buffer stays sized for its byte stream.
/// A TCP read can now absorb one worst-case encoded engine frame.
pub const READ_BUF_LEN: usize = FRAMED_LEN;

/// What a host should claim when it genuinely doesn't know its pipe: a modern wired-LAN
/// figure, whose tier is the table's top. The reference guesses 10 Mbps
/// (`TCPClientInterface.BITRATE_GUESS`), which its own tier table maps to an 8 KiB MTU —
/// a 2005 answer; a host that knows its real figure should pass it instead, in either
/// direction.
pub const TCP_BITRATE_GUESS_BPS: u32 = 1_000_000_000;

/// The most this interface can carry per wire packet — the engine's ceiling. What an
/// instance *declares* is its bitrate tier clamped to this.
pub const TCP_HW_MTU_CAP: usize = MAX_LINK_MTU;
/// Capacity, not a claim: the largest IFAC'd frame the engine can emit or accept, so the
/// serve loop's buffers carry any MTU the descriptor below can declare.
pub const FRAME_CAP: usize = MAX_WIRE_FRAME_LEN;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(FRAME_CAP);

/// The declared hardware MTU is the bitrate's tier clamped to the engine ceiling: the
/// in-transit MTU clamp takes interface declarations at face value when a relay books a
/// transported link, so an interface must never promise more than its buffers carry. The
/// day the ceiling rises, every declaration here rises with it for free.
pub fn descriptor(id: InterfaceId, bitrate_bps: u32) -> InterfaceConfig {
    InterfaceConfig {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::PointToPoint,
        hardware_mtu: hardware_mtu_for_bitrate(bitrate_bps).map(|tier| tier.min(MAX_LINK_MTU)),
        announce_rate_limit: None,
        bitrate_bps: Some(bitrate_bps),
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
}
