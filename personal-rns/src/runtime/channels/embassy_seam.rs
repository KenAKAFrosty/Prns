//! The embassy worker seam — the embassy-native analog of the std seam
//! ([`std_host`](super::std_host)). A per-interface three-lane seam: inbound and
//! outbound rings the interface fills/drains in place plus a control lane, each an
//! `embassy_sync::Channel`. The interface's task holds the [`InterfaceWorkerContext`];
//! the runtime holds the [`EmbassyInterfaceHandle`].
//!
//! Two embassy specifics versus std:
//! - **Static storage, not allocated rings.** A `Channel` lives in a `'static`,
//!   declared at the board (the embassy idiom). [`EmbassyInterfaceChannels`] bundles
//!   one interface's four channels for a single board static, and
//!   [`EmbassyInterfaceSeam::split`] hands out the two ends.
//! - **The wake is a shared [`Signal`], not a baked-in channel send.** Submitting
//!   inbound (or a report) [`signal`](Signal::signal)s the host's one
//!   [`WakeSignal`], which its `wait` awaits — the analog of the std `SyncSender`
//!   wake. The clock is embassy-global, so unlike std there is no `clock_base` to
//!   thread: stamps come straight from `Instant::now()`.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_time::Instant as EmbassyInstant;

use crate::engine::{InboundPacket, InstantMillis, OutboundPacket};
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundSink, InterfaceHandle, InterfaceId,
    InterfaceWorkerContext, OutboundDrain, QueueFull, Substrate,
};

/// Control-lane depth. Lifecycle signals are rare, so a few slots is ample.
const CONTROL_DEPTH: usize = 4;

/// The host's one wake: every interface's seam holds a reference and
/// [`signal`](Signal::signal)s it on `submit` / `report`, so a host blocked on
/// `wake.wait()` returns the moment any interface has something. The board declares
/// it as a `static` and hands `&'static` references to every seam.
pub type WakeSignal = Signal<CriticalSectionRawMutex, ()>;

/// A fresh, un-signaled [`WakeSignal`] for a board `static` — so a board writes
/// `static WAKE: WakeSignal = new_wake_signal();` without having to name
/// `embassy_sync` itself.
pub const fn new_wake_signal() -> WakeSignal {
    Signal::new()
}

/// One inbound slot: the worker fills `bytes[..len]` in place and the sink stamps
/// `arrived_at`. Rides the channel by value — an MTU-bounded move, no allocation.
struct InboundSlot<const MTU: usize> {
    arrived_at: InstantMillis,
    len: u16,
    bytes: [u8; MTU],
}

/// One outbound slot: the runtime fills `bytes[..len]`, the worker drains it.
struct OutboundSlot<const MTU: usize> {
    len: u16,
    bytes: [u8; MTU],
}

/// One interface's four channels, bundled so the board declares a single `static`
/// per interface. [`EmbassyInterfaceSeam::split`] hands the worker the producing /
/// consuming ends it needs and the runtime the mirror ends. `DEPTH` is the in-flight
/// capacity of each data ring; the control lanes use [`CONTROL_DEPTH`].
pub struct EmbassyInterfaceChannels<const MTU: usize, const DEPTH: usize> {
    inbound: Channel<CriticalSectionRawMutex, InboundSlot<MTU>, DEPTH>,
    outbound: Channel<CriticalSectionRawMutex, OutboundSlot<MTU>, DEPTH>,
    command: Channel<CriticalSectionRawMutex, ControlCommand, CONTROL_DEPTH>,
    report: Channel<CriticalSectionRawMutex, ControlReport, CONTROL_DEPTH>,
}

impl<const MTU: usize, const DEPTH: usize> EmbassyInterfaceChannels<MTU, DEPTH> {
    /// A fresh set of empty channels — `const`, so it can back a board `static`.
    pub const fn new() -> Self {
        Self {
            inbound: Channel::new(),
            outbound: Channel::new(),
            command: Channel::new(),
            report: Channel::new(),
        }
    }
}

impl<const MTU: usize, const DEPTH: usize> Default for EmbassyInterfaceChannels<MTU, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

/// The producer end of one interface's inbound channel (held by the worker task).
/// Stamps `arrived_at` off the embassy-global clock and signals the host's wake on
/// every publish, so the worker needs no clock and no separate wake of its own.
pub struct EmbassyInboundSink<const MTU: usize, const DEPTH: usize> {
    producer: Sender<'static, CriticalSectionRawMutex, InboundSlot<MTU>, DEPTH>,
    wake: &'static WakeSignal,
}

impl<const MTU: usize, const DEPTH: usize> InboundSink for EmbassyInboundSink<MTU, DEPTH> {
    fn submit(&mut self, fill: impl FnOnce(&mut [u8]) -> usize) -> Result<(), QueueFull> {
        let mut slot = InboundSlot {
            arrived_at: InstantMillis(EmbassyInstant::now().as_millis()),
            len: 0,
            bytes: [0u8; MTU],
        };
        slot.len = fill(&mut slot.bytes) as u16;
        let queued = self.producer.try_send(slot);
        // Wake even on a full ring — a backed-up ring is exactly when the host
        // should wake to drain. Coalesced: `Signal` holds one pending value.
        self.wake.signal(());
        queued.map_err(|_| QueueFull)
    }
}

/// The consumer end of one interface's outbound channel (held by the worker task).
pub struct EmbassyOutboundDrain<const MTU: usize, const DEPTH: usize> {
    consumer: Receiver<'static, CriticalSectionRawMutex, OutboundSlot<MTU>, DEPTH>,
}

impl<const MTU: usize, const DEPTH: usize> OutboundDrain for EmbassyOutboundDrain<MTU, DEPTH> {
    fn drain_each(&mut self, mut write: impl FnMut(OutboundPacket<'_>)) -> usize {
        let mut drained = 0;
        while let Ok(slot) = self.consumer.try_receive() {
            write(OutboundPacket::new(&slot.bytes[..slot.len as usize]));
            drained += 1;
        }
        drained
    }
}

impl<const MTU: usize, const DEPTH: usize> EmbassyOutboundDrain<MTU, DEPTH> {
    /// Await until at least one outbound packet is queued, without consuming it, so an
    /// async worker can `select` inbound-vs-outbound and only then drain. The await
    /// the sync trait can't offer: [`drain_each`](OutboundDrain::drain_each) only
    /// reports what is *already* there.
    pub async fn ready(&self) {
        self.consumer.ready_to_receive().await;
    }

    /// Copy the next queued outbound packet's bytes into `out`, returning its length,
    /// or `None` if the ring is empty. The async-worker twin of the sync
    /// [`drain_each`](OutboundDrain::drain_each): a worker pulls one packet, frames
    /// it, and `.await`s the write — which a sync closure can't. The copy-out is the
    /// price of crossing an `.await` (a sync host instead borrows the slot in place).
    pub fn try_next_into(&mut self, out: &mut [u8]) -> Option<usize> {
        let slot = self.consumer.try_receive().ok()?;
        let len = slot.len as usize;
        out[..len].copy_from_slice(&slot.bytes[..len]);
        Some(len)
    }
}

/// The worker's end of the control lane: pulls commands the runtime issued, pushes
/// reports back (signaling the host's wake so a report rouses it promptly).
pub struct EmbassyControlEndpoint {
    commands: Receiver<'static, CriticalSectionRawMutex, ControlCommand, CONTROL_DEPTH>,
    reports: Sender<'static, CriticalSectionRawMutex, ControlReport, CONTROL_DEPTH>,
    wake: &'static WakeSignal,
}

impl ControlEndpoint for EmbassyControlEndpoint {
    fn next_command(&mut self) -> Option<ControlCommand> {
        self.commands.try_receive().ok()
    }

    fn report(&mut self, report: ControlReport) {
        // A full report lane means the runtime hasn't drained yet; dropping a
        // duplicate lifecycle signal is benign (the runtime re-checks each cycle).
        let _ = self.reports.try_send(report);
        self.wake.signal(());
    }
}

impl EmbassyControlEndpoint {
    /// Await the next command the runtime issues — the async-worker twin of the
    /// non-blocking [`next_command`](ControlEndpoint::next_command), so a worker can
    /// `select` on it and wind down promptly on [`Stop`](ControlCommand::Stop)
    /// instead of only noticing it the next time some I/O happens to wake the loop.
    pub async fn await_command(&self) -> ControlCommand {
        self.commands.receive().await
    }
}

/// The embassy platform's worker-side lane-end types, bundled for [`InterfaceWorkerContext`].
pub struct EmbassyHostSubstrate<const MTU: usize, const DEPTH: usize>;

impl<const MTU: usize, const DEPTH: usize> Substrate for EmbassyHostSubstrate<MTU, DEPTH> {
    type InboundSink = EmbassyInboundSink<MTU, DEPTH>;
    type OutboundDrain = EmbassyOutboundDrain<MTU, DEPTH>;
    type Control = EmbassyControlEndpoint;
}

/// The runtime's end of one interface's seam: drains the inbound the worker
/// stamped (tagging it with the interface id this channel belongs to), queues
/// outbound for the worker to transmit, and trades control signals.
pub struct EmbassyInterfaceHandle<const MTU: usize, const DEPTH: usize> {
    id: InterfaceId,
    inbound: Receiver<'static, CriticalSectionRawMutex, InboundSlot<MTU>, DEPTH>,
    outbound: Sender<'static, CriticalSectionRawMutex, OutboundSlot<MTU>, DEPTH>,
    commands: Sender<'static, CriticalSectionRawMutex, ControlCommand, CONTROL_DEPTH>,
    reports: Receiver<'static, CriticalSectionRawMutex, ControlReport, CONTROL_DEPTH>,
}

impl<const MTU: usize, const DEPTH: usize> InterfaceHandle for EmbassyInterfaceHandle<MTU, DEPTH> {
    fn drain_inbound(&mut self, mut f: impl FnMut(InboundPacket<'_>)) -> usize {
        let mut drained = 0;
        while let Ok(slot) = self.inbound.try_receive() {
            f(InboundPacket {
                arrived_at: slot.arrived_at,
                source_interface: self.id,
                bytes: &slot.bytes[..slot.len as usize],
            });
            drained += 1;
        }
        drained
    }

    fn send(&mut self, packet: OutboundPacket<'_>) -> bool {
        if packet.bytes.len() > MTU {
            return false;
        }
        let mut slot = OutboundSlot {
            len: packet.bytes.len() as u16,
            bytes: [0u8; MTU],
        };
        slot.bytes[..packet.bytes.len()].copy_from_slice(packet.bytes);
        self.outbound.try_send(slot).is_ok()
    }

    fn request_stop(&mut self) {
        let _ = self.commands.try_send(ControlCommand::Stop);
    }

    fn next_report(&mut self) -> Option<ControlReport> {
        self.reports.try_receive().ok()
    }
}

/// The two halves of a built interface seam: the [`InterfaceWorkerContext`] the
/// interface's task is handed, and the [`EmbassyInterfaceHandle`] the runtime keeps.
pub struct EmbassyInterfaceSeam<const MTU: usize, const DEPTH: usize> {
    pub worker_context: InterfaceWorkerContext<EmbassyHostSubstrate<MTU, DEPTH>>,
    pub runtime_handle: EmbassyInterfaceHandle<MTU, DEPTH>,
}

impl<const MTU: usize, const DEPTH: usize> EmbassyInterfaceSeam<MTU, DEPTH> {
    /// Split a board's `'static` [`EmbassyInterfaceChannels`] into the worker-held
    /// context and the runtime-held handle for interface `id`. `wake` is the host's
    /// one shared [`WakeSignal`] — pass the same `&'static` reference to every seam
    /// so any interface's `submit` / `report` rouses the single host sleep.
    pub fn split(
        id: InterfaceId,
        channels: &'static EmbassyInterfaceChannels<MTU, DEPTH>,
        wake: &'static WakeSignal,
    ) -> Self {
        Self {
            worker_context: InterfaceWorkerContext {
                inbound: EmbassyInboundSink {
                    producer: channels.inbound.sender(),
                    wake,
                },
                outbound: EmbassyOutboundDrain {
                    consumer: channels.outbound.receiver(),
                },
                control: EmbassyControlEndpoint {
                    commands: channels.command.receiver(),
                    reports: channels.report.sender(),
                    wake,
                },
            },
            runtime_handle: EmbassyInterfaceHandle {
                id,
                inbound: channels.inbound.receiver(),
                outbound: channels.outbound.sender(),
                commands: channels.command.sender(),
                reports: channels.report.receiver(),
            },
        }
    }
}
