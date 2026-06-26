//! The interface boundary: the seam between the reactor and one interface. An
//! interface is its own small reactor over one medium — it owns the wire (the
//! socket, the port, the radio; opening it and reconnecting are *its* problem) and
//! bridges it to the engine as two halves: frames it reads off the wire, and frames
//! the engine wants sent. This module is the contract those halves meet across;
//! `tokio` and `embassy` each supply the concrete channels behind it, so one
//! interface body can be driven under either executor.

use crate::interfaces::ifac::IFAC_MAX_SIZE;
use crate::interfaces::{InterfaceConfig, InterfaceKind};

/// An IFAC'd wire frame carries the engine's full link ceiling plus the access tag —
/// the ceiling a uniform-slot host sizes its lanes and scratch buffers by. No frame
/// *type* is bound to it: frames live in grant-lane slots sized per lane and move as
/// grants, never by value.
pub const MAX_WIRE_FRAME_LEN: usize = crate::routing::links::MAX_LINK_MTU + IFAC_MAX_SIZE;

/// The embedded frame ceiling: an embassy reactor's stack scratch and lane slots
/// size to this, never the host's absolute `MAX_WIRE_FRAME_LEN`. Every interface's
/// `hardware_mtu` is clamped to it (see `clamp_to_embedded_ceiling`), so it is both
/// the lane size and the negotiation cap — what a board's DRAM budget can carry
/// across its lane pool. Set to 1472 (wire 1536): the only embedded wires that want
/// more are the host-facing USB/TCP links (USB is direct, TCP is bounded by the WiFi
/// path it rides anyway), so capping them here is near-free; every radio is already
/// under it (ESP-NOW 1406, BLE 500, LoRa ~500) as is the WiFi AutoInterface (1196).
/// Lowered from 2048 to reclaim ~7 KiB of internal lane `.bss` (plus core-1 scratch)
/// so the ESP32-S3 can carry two concurrent BLE peers at full coex+SoftAP. Shared by
/// every embedded board — net RAM-positive everywhere; per-board channel storage
/// keeps its own basis (each board's `LINK_MTU`), tunable independently of this.
pub const EMBEDDED_MAX_LINK_MTU: usize = 1_472;
pub const EMBEDDED_MAX_WIRE_FRAME_LEN: usize = EMBEDDED_MAX_LINK_MTU + IFAC_MAX_SIZE;

/// The broadcast-floor wire frame: the slot a no-MTU interface — or an empty reactor — sizes to.
pub const BROADCAST_WIRE_FRAME_LEN: usize = crate::wire::BROADCAST_MTU + IFAC_MAX_SIZE;

/// The wire frame an interface presents: its declared `hardware_mtu` (or the broadcast floor) plus
/// the IFAC tag. The host reactor sizes each lane and its shared scratch by this — per interface,
/// never the absolute ceiling.
pub fn frame_cap_for(descriptor: &InterfaceConfig) -> usize {
    descriptor
        .hardware_mtu
        .unwrap_or(crate::wire::BROADCAST_MTU)
        + IFAC_MAX_SIZE
}

/// The reactor's half of one interface, seen from inside the interface's run loop.
/// Both directions are async here, and deliberately so: the interface awaits
/// `next_outbound` for work — so it sleeps when there is nothing to send, the dormancy
/// a battery-powered host is built for — and awaits `next_inbound` to hand a received frame
/// across. The *sync* egress the borrow invariant demands lives on the reactor's side
/// of the queue `next_outbound` drains, not here: by the time `next_outbound` yields, the
/// frame already lives in the queue and no engine buffer is borrowed.
#[allow(async_fn_in_trait)]
pub trait InterfaceSeam {
    async fn next_inbound(&mut self, frame: &[u8]);
    /// The next frame owed to this interface's wire, borrowed in place from
    /// its outbound lane; the borrow releases on the following call.
    async fn next_outbound(&mut self) -> &[u8];

    async fn request_tunnel_synthesis(&mut self) {}
}

#[allow(async_fn_in_trait)]
pub trait Interface {
    /// The largest wire packet this interface can carry in one frame — the
    /// value its descriptor declares and its own buffers are sized by.
    const HW_MTU: usize;

    /// The medium this interface speaks — the namespace root of its id
    /// ([`from_channel_tag`](crate::interfaces::InterfaceId::from_channel_tag)).
    const KIND: InterfaceKind;

    fn descriptor(&self) -> InterfaceConfig;

    /// The bytes that uniquely tag *this* channel within its medium — a peer MAC, a remote
    /// `ip:port`, a device id. Combined with [`KIND`](Self::KIND) into the interface's id, so two
    /// distinct concurrent channels must never return the same bytes (the attach path rejects a
    /// live collision loudly); stability across a reconnect is best-effort, whatever the medium
    /// can offer.
    fn channel_tag(&self) -> &[u8];

    async fn run<S: InterfaceSeam>(self, seam: S);
}
