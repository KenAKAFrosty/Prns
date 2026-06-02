//! The seam an interface meets the runtime through: bytes in, bytes out, and a
//! control plane of signals. [`InterfaceWorkerContext`] is the same for every
//! interface and every drive mode — the one universal thing — so writing an
//! interface forces you to express it as "received bytes go here, bytes to send
//! come from here, lifecycle signals pass over this." Two byte lanes carry the
//! data ([`InboundSink`] / [`OutboundDrain`], filled and drained in place so no
//! owned packet crosses the seam); one control lane carries lifecycle signals
//! ([`ControlCommand`] / [`ControlReport`], passed by value — they're tiny). The
//! concrete worker-side lane ends are a platform's [`Substrate`]; the runtime
//! holds the other ends as an
//! [`InterfaceHandle`](crate::interfaces::InterfaceHandle).

use crate::engine::OutboundPacket;
use crate::interfaces::{ConnectionState, QueueFull};

/// The producer end of an interface's inbox: the interface fills a slot in place
/// with a decoded Reticulum packet, and the sink publishes it (stamping arrival
/// on the runtime's clock, so the interface needs no clock of its own). Filling
/// in place is what keeps it zero-copy — no owned packet crosses the seam.
pub trait InboundSink {
    /// Reserve a slot, hand it to `fill` to write a packet into (returning the
    /// length written), and publish it. [`QueueFull`] means the inbox is backed
    /// up and the packet was dropped — never blocks.
    fn submit(&mut self, fill: impl FnOnce(&mut [u8]) -> usize) -> Result<(), QueueFull>;
}

/// The consumer end of an interface's outbox: the runtime queues packets to send
/// on the mirror of this, and the interface drains them here, each lent in place
/// as a borrowed [`OutboundPacket`].
pub trait OutboundDrain {
    /// Lend every packet currently queued to `write`, in order, returning how
    /// many were drained. Each packet borrows the queue slot — no copy.
    fn drain_each(&mut self, write: impl FnMut(OutboundPacket<'_>)) -> usize;
}

/// A lifecycle command the runtime issues to an interface over the control
/// plane. The worker's loop matches it exhaustively.
#[derive(Clone, Copy, Debug)]
pub enum ControlCommand {
    /// Wind down: finish in-flight work, release the device, then
    /// [`report`](ControlEndpoint::report) [`ControlReport::Stopped`].
    Stop,
}

/// What an interface reports back to the runtime over the control plane.
#[derive(Clone, Copy, Debug)]
pub enum ControlReport {
    ConnectionState(ConnectionState),
    /// Teardown is complete — the loop has exited and the device is released, so
    /// the runtime may drop this interface's seam.
    Stopped,
}

/// The worker's end of the control plane: a 1:1 lifecycle lane shared with the
/// runtime. The worker checks for commands inside its own loop and reports state
/// back — both non-blocking, so neither side ever stalls the other. Signals are
/// tiny [`Copy`] enums passed by value (no zero-copy ceremony for a one-byte
/// `Stop`); the lane is fixed-capacity, so no allocator is involved. How the
/// runtime *waits* for a report is its own business — poll, block, or await —
/// and stays out of this contract.
pub trait ControlEndpoint {
    /// The next command the runtime has issued, or `None` if it has said nothing
    /// since the last check. Never blocks — the worker polls this between I/O.
    fn next_command(&mut self) -> Option<ControlCommand>;

    /// Tell the runtime something (e.g. that teardown is complete). Non-blocking;
    /// the runtime reads it when it chooses to.
    fn report(&mut self, report: ControlReport);
}

/// A platform's lane-end types, bundled so an [`InterfaceWorkerContext`] varies
/// by one knob instead of three — the interface-facing dual of the runtime's
/// `Host`.
pub trait Substrate {
    type InboundSink: InboundSink;
    type OutboundDrain: OutboundDrain;
    type Control: ControlEndpoint;
}

/// The whole seam an interface is handed at construction: where its received
/// packets go, where the packets it must transmit come from, and the control
/// lane it trades lifecycle signals with the runtime over.
pub struct InterfaceWorkerContext<S: Substrate> {
    pub inbound: S::InboundSink,
    pub outbound: S::OutboundDrain,
    pub control: S::Control,
}
