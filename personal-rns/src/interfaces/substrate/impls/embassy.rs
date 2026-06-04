//! Embassy implementation of the per-interface runtime lanes. Inbound,
//! outbound, command, and report lanes are `embassy_sync::Channel`s split between
//! the worker's [`InterfaceWorkerContext`] and the runtime's
//! [`EmbassyInterfaceHandle`].
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

use crate::engine::InstantMillis;
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundPacket, InboundSink, Interface,
    InterfaceHandle, InterfaceId, InterfaceWorkerContext, OutboundDrain, OutboundPacket, QueueFull,
    SendError, StartedInterface, Substrate,
};

/// Control-lane depth. Lifecycle signals are rare, so a few slots is ample.
const CONTROL_DEPTH: usize = 4;

/// Shared host wake. Each interface signals it when inbound data or a report is
/// queued, so one host sleep can wait on all interfaces.
pub type WakeSignal = Signal<CriticalSectionRawMutex, ()>;

/// A fresh [`WakeSignal`] for board `static`s.
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

/// One interface's four channels, bundled so the board declares a single
/// `static` per interface. `MAX_BUFFERED_PACKETS` is the data-lane capacity; control lanes use
/// [`CONTROL_DEPTH`].
pub struct EmbassyInterfaceChannels<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> {
    inbound: Channel<CriticalSectionRawMutex, InboundSlot<MTU>, MAX_BUFFERED_PACKETS>,
    outbound: Channel<CriticalSectionRawMutex, OutboundSlot<MTU>, MAX_BUFFERED_PACKETS>,
    command: Channel<CriticalSectionRawMutex, ControlCommand, CONTROL_DEPTH>,
    report: Channel<CriticalSectionRawMutex, ControlReport, CONTROL_DEPTH>,
}

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize>
    EmbassyInterfaceChannels<MTU, MAX_BUFFERED_PACKETS>
{
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

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> Default
    for EmbassyInterfaceChannels<MTU, MAX_BUFFERED_PACKETS>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Worker-held producer end of one interface's inbound channel. It stamps
/// arrivals from the embassy-global clock and signals the host on publish.
pub struct EmbassyInboundSink<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> {
    producer: Sender<'static, CriticalSectionRawMutex, InboundSlot<MTU>, MAX_BUFFERED_PACKETS>,
    wake: &'static WakeSignal,
}

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> InboundSink
    for EmbassyInboundSink<MTU, MAX_BUFFERED_PACKETS>
{
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
pub struct EmbassyOutboundDrain<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> {
    consumer: Receiver<'static, CriticalSectionRawMutex, OutboundSlot<MTU>, MAX_BUFFERED_PACKETS>,
}

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> OutboundDrain
    for EmbassyOutboundDrain<MTU, MAX_BUFFERED_PACKETS>
{
    fn drain_each(&mut self, mut write: impl FnMut(OutboundPacket<'_>)) -> usize {
        let mut drained = 0;
        while let Ok(slot) = self.consumer.try_receive() {
            write(OutboundPacket::new(&slot.bytes[..slot.len as usize]));
            drained += 1;
        }
        drained
    }
}

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize>
    EmbassyOutboundDrain<MTU, MAX_BUFFERED_PACKETS>
{
    /// Await until at least one outbound packet is queued, without consuming it.
    pub async fn ready(&self) {
        self.consumer.ready_to_receive().await;
    }

    /// Copy the next queued outbound packet into `out`, returning its length.
    /// Copying lets the caller hold the bytes across an `.await`.
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
pub struct EmbassyHostSubstrate<const MTU: usize, const MAX_BUFFERED_PACKETS: usize>;

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> Substrate
    for EmbassyHostSubstrate<MTU, MAX_BUFFERED_PACKETS>
{
    type InboundSink = EmbassyInboundSink<MTU, MAX_BUFFERED_PACKETS>;
    type OutboundDrain = EmbassyOutboundDrain<MTU, MAX_BUFFERED_PACKETS>;
    type Control = EmbassyControlEndpoint;
}

/// Runtime-held end of one interface's lanes.
pub struct EmbassyInterfaceHandle<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> {
    id: InterfaceId,
    inbound: Receiver<'static, CriticalSectionRawMutex, InboundSlot<MTU>, MAX_BUFFERED_PACKETS>,
    outbound: Sender<'static, CriticalSectionRawMutex, OutboundSlot<MTU>, MAX_BUFFERED_PACKETS>,
    commands: Sender<'static, CriticalSectionRawMutex, ControlCommand, CONTROL_DEPTH>,
    reports: Receiver<'static, CriticalSectionRawMutex, ControlReport, CONTROL_DEPTH>,
}

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> InterfaceHandle
    for EmbassyInterfaceHandle<MTU, MAX_BUFFERED_PACKETS>
{
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

    fn send(&mut self, packet: OutboundPacket<'_>) -> Result<(), SendError> {
        if packet.bytes.len() > MTU {
            return Err(SendError::PacketTooLarge);
        }
        let mut slot = OutboundSlot {
            len: packet.bytes.len() as u16,
            bytes: [0u8; MTU],
        };
        slot.bytes[..packet.bytes.len()].copy_from_slice(packet.bytes);
        self.outbound
            .try_send(slot)
            .map_err(|_| SendError::QueueFull)
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
pub struct EmbassyInterfaceSeam<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> {
    pub worker_context: InterfaceWorkerContext<EmbassyHostSubstrate<MTU, MAX_BUFFERED_PACKETS>>,
    pub runtime_handle: EmbassyInterfaceHandle<MTU, MAX_BUFFERED_PACKETS>,
}

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize>
    EmbassyInterfaceSeam<MTU, MAX_BUFFERED_PACKETS>
{
    /// Split a board's `'static` [`EmbassyInterfaceChannels`] into the worker-held
    /// context and the runtime-held handle for interface `id`. `wake` is the host's
    /// one shared [`WakeSignal`] — pass the same `&'static` reference to every seam
    /// so any interface's `submit` / `report` rouses the single host sleep.
    pub fn split(
        id: InterfaceId,
        channels: &'static EmbassyInterfaceChannels<MTU, MAX_BUFFERED_PACKETS>,
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

    /// Start `interface` onto this seam: hand it the worker context, keep its runtime
    /// handle, and bundle the [`StartedInterface`] the runtime pools.
    pub fn start_interface<I>(
        self,
        interface: I,
    ) -> StartedInterface<EmbassyInterfaceHandle<MTU, MAX_BUFFERED_PACKETS>, I::Worker>
    where
        I: Interface<EmbassyHostSubstrate<MTU, MAX_BUFFERED_PACKETS>>,
    {
        let descriptor = interface.descriptor();
        let drive = interface.start(self.worker_context);
        StartedInterface {
            descriptor,
            handle: self.runtime_handle,
            drive,
        }
    }
}
