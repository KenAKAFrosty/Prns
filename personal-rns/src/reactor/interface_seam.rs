//! The interface boundary: the seam between the reactor and one interface. An
//! interface is its own small reactor over one medium — it owns the wire (the
//! socket, the port, the radio; opening it and reconnecting are *its* problem) and
//! bridges it to the engine as two halves: frames it reads off the wire, and frames
//! the engine wants sent. This module is the contract those halves meet across;
//! `tokio` and `embassy` each supply the concrete channels behind it, so one
//! interface body can be driven under either executor.

use crate::interfaces::ifac::IFAC_MAX_SIZE;
use crate::interfaces::InterfaceConfig;

/// An IFAC'd wire frame carries the engine's full link ceiling plus the access tag —
/// the ceiling a uniform-slot host sizes its lanes and scratch buffers by. No frame
/// *type* is bound to it: frames live in grant-lane slots sized per lane and move as
/// grants, never by value.
pub const MAX_WIRE_FRAME_LEN: usize = crate::routing::links::MAX_LINK_MTU + IFAC_MAX_SIZE;

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
}

#[allow(async_fn_in_trait)]
pub trait Interface {
    /// The largest wire packet this interface can carry in one frame — the
    /// value its descriptor declares and its own buffers are sized by.
    const HW_MTU: usize;

    fn descriptor(&self) -> InterfaceConfig;
    async fn run<S: InterfaceSeam>(self, seam: S);
}
