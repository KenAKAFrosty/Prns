//! The host-agnostic core of the local shared-instance interface: the loopback port, the framing
//! capacities (the engine's ceiling, shared with TCP), and the descriptor shape. A local bus is
//! effectively free, so the figures are fixed rather than per-instance: RNS declares 1 Gbps on both
//! ends, and the spawned client is `MODE_FULL` — full participation, so an app's announces propagate
//! out our real interfaces as if they were ours.

use crate::interfaces::rns_serial_framing;
use crate::interfaces::wire_limits::MAX_WIRE_FRAME_LEN;
use crate::interfaces::{
    hardware_mtu_for_bitrate, AnnounceBandwidthCap, BitrateBps, EgressCapability,
    IngressCapability, InterfaceCapabilities, InterfaceDescriptor, InterfaceId, InterfaceMode,
    TransportCapability,
};
use crate::routing::links::MAX_LINK_MTU;

/// RNS's shared-instance loopback port (`Reticulum.local_interface_port`). A starting RNS app tries
/// to bind this; if our daemon already holds it, the app connects here as a local client instead.
pub const DEFAULT_LOCAL_PORT: u16 = 37428;

/// The bitrate RNS declares on both local ends (`LocalClientInterface.bitrate`): a local bus is not
/// the bottleneck, so 1 Gbps, the wired-LAN tier.
pub const LOCAL_BITRATE_BPS: BitrateBps = BitrateBps::guess(1_000_000_000);

/// RNS's default `local_socket_path` on Linux (`Reticulum.__init__`: it becomes `"default"` whenever
/// AF_UNIX is in use and no path is configured). The abstract socket the daemon binds is then
/// `\0rns/default`, which a default-config Linux app connects to in preference to TCP — so binding it
/// is what lets an unconfigured Sideband/NomadNet find the daemon on Linux.
pub const DEFAULT_SOCKET_PATH: &str = "default";

/// The most a local interface carries per wire packet — the engine's ceiling, same as TCP.
pub const HW_MTU_CAP: usize = MAX_LINK_MTU;
/// Capacity, not a claim: the largest IFAC'd frame the engine can emit or accept, so the serve
/// loop's buffers carry any MTU the descriptor declares.
pub const FRAME_CAP: usize = MAX_WIRE_FRAME_LEN;
pub const FRAMED_LEN: usize = rns_serial_framing::max_encoded_len(FRAME_CAP);
/// One socket read's worth — sized to absorb one worst-case encoded engine frame, since a local
/// read can be a whole gigabit-speed resource frame at once.
pub const READ_BUF_LEN: usize = FRAMED_LEN;

/// The descriptor for one spawned local client. `MODE_FULL` (RNS gives its shared instance and apps
/// full participation), cross-interface egress (the daemon relays an app's traffic out its real
/// interfaces and to its other apps), and the 1 Gbps tier clamped to the engine ceiling.
#[must_use]
pub fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        hardware_mtu: hardware_mtu_for_bitrate(LOCAL_BITRATE_BPS.get())
            .map(|tier| tier.min(MAX_LINK_MTU)),
        announce_rate_limit: None,
        bitrate: LOCAL_BITRATE_BPS,
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
}
