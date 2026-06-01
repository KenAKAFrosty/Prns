use crate::engine::OutboundPacket;
use crate::interfaces::{InterfaceDescriptor, InterfaceStats};

/// The runtime tried to hand a worker more than its outbound queue can hold; the
/// packet was not enqueued. The caller decides drop vs retry — submitting never
/// blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueFull;

/// An interface that runs as its own mini-program: it owns all byte I/O, its
/// peers, fan-out, and discovery — opaquely — and meets the system only through
/// this seam.
///
/// Deliberately **not** a sub-trait of [`Interface`](crate::interfaces::Interface).
/// An `Interface` is called inline for byte I/O (`try_read`/`write`), which
/// pulls byte-management up into whoever holds it; a worker is the opposite —
/// the system never reads or writes its bytes directly. It hands the worker
/// packets to send via [`submit`](InterfaceWorker::submit), and receives the
/// worker's inbound by draining the runtime's shared mailbox, into which the
/// worker stamps and pushes its own
/// [`InboundPacket`](crate::engine::InboundPacket)s (its own id + arrival time).
/// So the two are siblings, not parent and child.
///
/// How a worker actually runs is its own call — its own OS thread, its own
/// async task, or, if it cannot run itself, by also implementing
/// [`RuntimeDriven`](crate::interfaces::RuntimeDriven) so the runtime advances
/// it cooperatively. The `Send`-ness and trait set a worker offers are exactly
/// the capabilities a host matches against what it can provide.
pub trait InterfaceWorker {
    /// The maximum serialized packet length, in bytes, this worker handles —
    /// the buffer capacity a runtime sizes its shared inbound mailbox to so a
    /// stamped packet always fits. A compile-time knob, well-known on the trait
    /// so a host wires the mailbox off this one number instead of reaching into
    /// the worker's internals (RNS pins 1196 for the AutoInterface; a LoRa
    /// worker is far smaller). The runtime stays generic over it and infers it
    /// from the wired mailbox — it never picks a size itself.
    const PACKET_BUFFER_SIZE: usize;

    /// The routing facts the engine registers and routes on.
    fn descriptor(&self) -> InterfaceDescriptor;

    /// A cheap snapshot of liveness + counters. Never blocks.
    fn health(&self) -> InterfaceStats;

    /// Hand the worker one serialized Reticulum wire packet — an
    /// [`OutboundPacket`] — to transmit. The worker delivers it however its
    /// medium requires — a shared-multicast worker fans it out to every peer it
    /// knows, opaquely. Non-blocking: [`QueueFull`] means the worker is backed
    /// up and the caller chooses drop vs retry.
    fn submit(&mut self, packet: OutboundPacket) -> Result<(), QueueFull>;
}
