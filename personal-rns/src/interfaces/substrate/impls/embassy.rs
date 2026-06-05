use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
use embassy_time::Instant as EmbassyInstant;
use static_cell::{ConstStaticCell, StaticCell};

use crate::engine::InstantMillis;
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundPacket, InboundSink, Interface,
    InterfaceHandle, InterfaceId, InterfaceWorkerContext, OutboundDrain, OutboundPacket, QueueFull,
    SendError, StartedInterface, Substrate,
};

const CONTROL_DEPTH: usize = 4;

pub type WakeSignal = Signal<CriticalSectionRawMutex, ()>;

pub const fn new_wake_signal() -> WakeSignal {
    Signal::new()
}

struct InboundSlot<const MTU: usize> {
    arrived_at: InstantMillis,
    len: u16,
    bytes: [u8; MTU],
}

struct OutboundSlot<const MTU: usize> {
    len: u16,
    bytes: [u8; MTU],
}

impl<const MTU: usize> OutboundSlot<MTU> {
    const EMPTY: Self = Self {
        len: 0,
        bytes: [0u8; MTU],
    };
}

pub struct EmbassyInterfaceChannels<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> {
    inbound: Channel<CriticalSectionRawMutex, InboundSlot<MTU>, MAX_BUFFERED_PACKETS>,
    outbound_slots: ConstStaticCell<[OutboundSlot<MTU>; MAX_BUFFERED_PACKETS]>,
    outbound:
        StaticCell<zerocopy_channel::Channel<'static, CriticalSectionRawMutex, OutboundSlot<MTU>>>,
    command: Channel<CriticalSectionRawMutex, ControlCommand, CONTROL_DEPTH>,
    report: Channel<CriticalSectionRawMutex, ControlReport, CONTROL_DEPTH>,
}

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize>
    EmbassyInterfaceChannels<MTU, MAX_BUFFERED_PACKETS>
{
    pub const fn new() -> Self {
        Self {
            inbound: Channel::new(),
            outbound_slots: ConstStaticCell::new([OutboundSlot::EMPTY; MAX_BUFFERED_PACKETS]),
            outbound: StaticCell::new(),
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

pub struct EmbassyOutboundDrain<const MTU: usize> {
    consumer: zerocopy_channel::Receiver<'static, CriticalSectionRawMutex, OutboundSlot<MTU>>,
}

impl<const MTU: usize> OutboundDrain for EmbassyOutboundDrain<MTU> {
    fn drain_each(&mut self, mut write: impl FnMut(OutboundPacket<'_>)) -> usize {
        let mut drained = 0;
        while let Some(slot) = self.consumer.try_receive() {
            write(OutboundPacket::new(&slot.bytes[..slot.len as usize]));
            self.consumer.receive_done();
            drained += 1;
        }
        drained
    }
}

impl<const MTU: usize> EmbassyOutboundDrain<MTU> {
    /// Resolves once at least one outbound packet is queued, without dequeuing
    /// it (the grant is dropped untaken; only `receive_done` consumes).
    pub async fn ready(&mut self) {
        let _ = self.consumer.receive().await;
    }

    pub fn try_next_into(&mut self, out: &mut [u8]) -> Option<usize> {
        let slot = self.consumer.try_receive()?;
        let len = slot.len as usize;
        out[..len].copy_from_slice(&slot.bytes[..len]);
        self.consumer.receive_done();
        Some(len)
    }
}

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
    pub async fn await_command(&self) -> ControlCommand {
        self.commands.receive().await
    }
}

pub struct EmbassyHostSubstrate<const MTU: usize, const MAX_BUFFERED_PACKETS: usize>;

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> Substrate
    for EmbassyHostSubstrate<MTU, MAX_BUFFERED_PACKETS>
{
    type InboundSink = EmbassyInboundSink<MTU, MAX_BUFFERED_PACKETS>;
    type OutboundDrain = EmbassyOutboundDrain<MTU>;
    type Control = EmbassyControlEndpoint;
}

pub struct EmbassyInterfaceHandle<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> {
    id: InterfaceId,
    inbound: Receiver<'static, CriticalSectionRawMutex, InboundSlot<MTU>, MAX_BUFFERED_PACKETS>,
    outbound: zerocopy_channel::Sender<'static, CriticalSectionRawMutex, OutboundSlot<MTU>>,
    commands: Sender<'static, CriticalSectionRawMutex, ControlCommand, CONTROL_DEPTH>,
    reports: Receiver<'static, CriticalSectionRawMutex, ControlReport, CONTROL_DEPTH>,
}

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> InterfaceHandle
    for EmbassyInterfaceHandle<MTU, MAX_BUFFERED_PACKETS>
{
    fn drain_inbound(&mut self, mut f: impl FnMut(InboundPacket<'_>)) -> usize {
        let mut drained = 0;
        while let Ok(mut slot) = self.inbound.try_receive() {
            f(InboundPacket {
                arrived_at: slot.arrived_at,
                source_interface: self.id,
                bytes: &mut slot.bytes[..slot.len as usize],
            });
            drained += 1;
        }
        drained
    }

    fn acquire_send_grant(
        &mut self,
        fill: impl FnOnce(&mut [u8]) -> usize,
    ) -> Result<usize, SendError> {
        let Some(slot) = self.outbound.try_send() else {
            return Err(SendError::QueueFull);
        };
        let written = fill(&mut slot.bytes);
        slot.len = written as u16;
        self.outbound.send_done();
        Ok(written)
    }

    fn request_stop(&mut self) {
        let _ = self.commands.try_send(ControlCommand::Stop);
    }

    fn next_report(&mut self) -> Option<ControlReport> {
        self.reports.try_receive().ok()
    }
}

pub struct EmbassyInterfaceSeam<const MTU: usize, const MAX_BUFFERED_PACKETS: usize> {
    pub worker_context: InterfaceWorkerContext<EmbassyHostSubstrate<MTU, MAX_BUFFERED_PACKETS>>,
    pub runtime_handle: EmbassyInterfaceHandle<MTU, MAX_BUFFERED_PACKETS>,
}

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize>
    EmbassyInterfaceSeam<MTU, MAX_BUFFERED_PACKETS>
{
    pub fn split(
        id: InterfaceId,
        channels: &'static EmbassyInterfaceChannels<MTU, MAX_BUFFERED_PACKETS>,
        wake: &'static WakeSignal,
    ) -> Self {
        let outbound = channels.outbound.init(zerocopy_channel::Channel::new(
            channels.outbound_slots.take(),
        ));
        let (outbound_sender, outbound_receiver) = outbound.split();
        Self {
            worker_context: InterfaceWorkerContext {
                inbound: EmbassyInboundSink {
                    producer: channels.inbound.sender(),
                    wake,
                },
                outbound: EmbassyOutboundDrain {
                    consumer: outbound_receiver,
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
                outbound: outbound_sender,
                commands: channels.command.sender(),
                reports: channels.report.receiver(),
            },
        }
    }

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
