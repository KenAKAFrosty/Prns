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

impl<const MTU: usize> InboundSlot<MTU> {
    const EMPTY: Self = Self {
        arrived_at: InstantMillis(0),
        len: 0,
        bytes: [0u8; MTU],
    };
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
    inbound_slots: ConstStaticCell<[InboundSlot<MTU>; MAX_BUFFERED_PACKETS]>,
    inbound:
        StaticCell<zerocopy_channel::Channel<'static, CriticalSectionRawMutex, InboundSlot<MTU>>>,
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
            inbound_slots: ConstStaticCell::new([InboundSlot::EMPTY; MAX_BUFFERED_PACKETS]),
            inbound: StaticCell::new(),
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

pub struct EmbassyInboundSink<const MTU: usize> {
    producer: zerocopy_channel::Sender<'static, CriticalSectionRawMutex, InboundSlot<MTU>>,
    wake: &'static WakeSignal,
}

impl<const MTU: usize> InboundSink for EmbassyInboundSink<MTU> {
    fn submit(&mut self, fill: impl FnOnce(&mut [u8]) -> usize) -> Result<(), QueueFull> {
        // Wake on both paths — a backed-up ring is exactly when the host
        // should wake to drain. Coalesced: `Signal` holds one pending value.
        let Some(slot) = self.producer.try_send() else {
            self.wake.signal(());
            return Err(QueueFull);
        };
        slot.arrived_at = InstantMillis(EmbassyInstant::now().as_millis());
        slot.len = fill(&mut slot.bytes) as u16;
        self.producer.send_done();
        self.wake.signal(());
        Ok(())
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
    /// it (the grant is dropped untaken; only completing a lease consumes).
    pub async fn ready(&mut self) {
        let _ = self.consumer.receive().await;
    }

    /// Lease the packet at the head of the queue, if any.
    pub fn lease(&mut self) -> Option<OutboundLease<'_, MTU>> {
        let _ = self.consumer.try_receive()?;
        Some(OutboundLease { drain: self })
    }
}

/// A queued outbound packet, borrowed in place from its ring slot. The packet
/// stays queued until [`complete`](Self::complete). Dropping the lease
/// instead (including by a cancelled send future) leaves it at the head of
/// the queue, so an interrupted transmission retries rather than losing the
/// packet. [`keep`](Self::keep) is the same outcome as a named act.
///
/// Every example below is a `compile_fail` doctest, not sample code. The
/// test suite asserts that the compiler *rejects* it, pinned to the expected
/// error code, so each misuse shown is a checked guarantee.
///
/// Consuming is by value, so a packet can never be completed twice:
///
/// ```compile_fail,E0382
/// fn double_complete(drain: &mut personal_rns::interfaces::substrate::EmbassyOutboundDrain<500>) {
///     let lease = drain.lease().unwrap();
///     lease.complete();
///     lease.complete();
/// }
/// ```
///
/// ...its bytes are unreachable once consumed:
///
/// ```compile_fail,E0382
/// fn packet_after_complete(drain: &mut personal_rns::interfaces::substrate::EmbassyOutboundDrain<500>) {
///     let mut lease = drain.lease().unwrap();
///     lease.complete();
///     let _ = lease.packet();
/// }
/// ```
///
/// ...and the lease holds the drain exclusively, so a second packet can't be
/// leased while one is in hand:
///
/// ```compile_fail,E0499
/// fn two_leases(drain: &mut personal_rns::interfaces::substrate::EmbassyOutboundDrain<500>) {
///     let mut first = drain.lease().unwrap();
///     let second = drain.lease();
///     let _ = first.packet();
/// }
/// ```
#[must_use = "dropping a lease leaves the packet queued; complete() consumes it, keep() names the retry"]
pub struct OutboundLease<'d, const MTU: usize> {
    drain: &'d mut EmbassyOutboundDrain<MTU>,
}

impl<const MTU: usize> OutboundLease<'_, MTU> {
    #[allow(clippy::expect_used)]
    pub fn packet(&mut self) -> &[u8] {
        let slot = self
            .drain
            .consumer
            .try_receive()
            .expect("a lease exists only while its packet is queued");
        &slot.bytes[..slot.len as usize]
    }

    /// Consume the leased packet, freeing its ring slot.
    pub fn complete(self) {
        self.drain.consumer.receive_done();
    }

    /// Leave the packet queued; the next lease returns it again.
    pub fn keep(self) {}
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
    type InboundSink = EmbassyInboundSink<MTU>;
    type OutboundDrain = EmbassyOutboundDrain<MTU>;
    type Control = EmbassyControlEndpoint;
}

pub struct EmbassyInterfaceHandle<const MTU: usize> {
    id: InterfaceId,
    inbound: zerocopy_channel::Receiver<'static, CriticalSectionRawMutex, InboundSlot<MTU>>,
    outbound: zerocopy_channel::Sender<'static, CriticalSectionRawMutex, OutboundSlot<MTU>>,
    commands: Sender<'static, CriticalSectionRawMutex, ControlCommand, CONTROL_DEPTH>,
    reports: Receiver<'static, CriticalSectionRawMutex, ControlReport, CONTROL_DEPTH>,
}

impl<const MTU: usize> InterfaceHandle for EmbassyInterfaceHandle<MTU> {
    fn drain_inbound(&mut self, mut f: impl FnMut(InboundPacket<'_>)) -> usize {
        let mut drained = 0;
        while let Some(slot) = self.inbound.try_receive() {
            f(InboundPacket {
                arrived_at: slot.arrived_at,
                source_interface: self.id,
                bytes: &mut slot.bytes[..slot.len as usize],
            });
            self.inbound.receive_done();
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
    pub runtime_handle: EmbassyInterfaceHandle<MTU>,
}

impl<const MTU: usize, const MAX_BUFFERED_PACKETS: usize>
    EmbassyInterfaceSeam<MTU, MAX_BUFFERED_PACKETS>
{
    pub fn split(
        id: InterfaceId,
        channels: &'static EmbassyInterfaceChannels<MTU, MAX_BUFFERED_PACKETS>,
        wake: &'static WakeSignal,
    ) -> Self {
        let inbound = channels.inbound.init(zerocopy_channel::Channel::new(
            channels.inbound_slots.take(),
        ));
        let (inbound_sender, inbound_receiver) = inbound.split();
        let outbound = channels.outbound.init(zerocopy_channel::Channel::new(
            channels.outbound_slots.take(),
        ));
        let (outbound_sender, outbound_receiver) = outbound.split();
        Self {
            worker_context: InterfaceWorkerContext {
                inbound: EmbassyInboundSink {
                    producer: inbound_sender,
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
                inbound: inbound_receiver,
                outbound: outbound_sender,
                commands: channels.command.sender(),
                reports: channels.report.receiver(),
            },
        }
    }

    pub fn start_interface<I>(
        self,
        interface: I,
    ) -> StartedInterface<EmbassyInterfaceHandle<MTU>, I::Worker>
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::SendError;

    const MTU: usize = 64;

    fn id() -> InterfaceId {
        InterfaceId::new([0x22; 16])
    }

    fn grant(handle: &mut EmbassyInterfaceHandle<MTU>, byte: u8, len: usize) {
        let written = handle
            .acquire_send_grant(|buf| {
                buf[..len].fill(byte);
                len
            })
            .unwrap();
        assert_eq!(written, len);
    }

    #[test]
    fn leases_consume_packets_in_order_and_byte_exact() {
        static CH: EmbassyInterfaceChannels<MTU, 4> = EmbassyInterfaceChannels::new();
        static WAKE: WakeSignal = new_wake_signal();
        let EmbassyInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = EmbassyInterfaceSeam::split(id(), &CH, &WAKE);

        grant(&mut runtime_handle, 0xAA, 3);
        grant(&mut runtime_handle, 0xBB, 5);

        let mut lease = worker_context.outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0xAA, 0xAA, 0xAA]);
        lease.complete();

        let mut lease = worker_context.outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0xBB; 5]);
        lease.complete();

        assert!(worker_context.outbound.lease().is_none());
    }

    #[test]
    fn a_kept_lease_returns_the_same_packet_next_time() {
        static CH: EmbassyInterfaceChannels<MTU, 4> = EmbassyInterfaceChannels::new();
        static WAKE: WakeSignal = new_wake_signal();
        let EmbassyInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = EmbassyInterfaceSeam::split(id(), &CH, &WAKE);

        grant(&mut runtime_handle, 0xAA, 2);
        grant(&mut runtime_handle, 0xBB, 2);

        let lease = worker_context.outbound.lease().unwrap();
        lease.keep();

        let mut lease = worker_context.outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0xAA, 0xAA]);
        lease.complete();

        let mut lease = worker_context.outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0xBB, 0xBB]);
        lease.complete();
    }

    #[test]
    fn a_dropped_lease_keeps_the_packet_queued() {
        static CH: EmbassyInterfaceChannels<MTU, 4> = EmbassyInterfaceChannels::new();
        static WAKE: WakeSignal = new_wake_signal();
        let EmbassyInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = EmbassyInterfaceSeam::split(id(), &CH, &WAKE);

        grant(&mut runtime_handle, 0xCC, 4);

        {
            let mut lease = worker_context.outbound.lease().unwrap();
            let _ = lease.packet();
        }

        let mut lease = worker_context.outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0xCC; 4]);
        lease.complete();
        assert!(worker_context.outbound.lease().is_none());
    }

    #[test]
    fn completing_a_lease_frees_the_slot_for_the_producer_across_wraparound() {
        static CH: EmbassyInterfaceChannels<MTU, 2> = EmbassyInterfaceChannels::new();
        static WAKE: WakeSignal = new_wake_signal();
        let EmbassyInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = EmbassyInterfaceSeam::split(id(), &CH, &WAKE);

        for round in 0u8..5 {
            grant(&mut runtime_handle, round, 1);
            grant(&mut runtime_handle, round + 100, 1);
            assert_eq!(
                runtime_handle.acquire_send_grant(|_buf| panic!("ring is full, fill must not run")),
                Err(SendError::QueueFull)
            );

            let mut lease = worker_context.outbound.lease().unwrap();
            assert_eq!(lease.packet(), &[round]);
            lease.complete();
            let mut lease = worker_context.outbound.lease().unwrap();
            assert_eq!(lease.packet(), &[round + 100]);
            lease.complete();
        }
        assert!(worker_context.outbound.lease().is_none());
    }

    #[test]
    fn a_full_inbound_ring_reports_queue_full_without_running_the_fill() {
        static CH: EmbassyInterfaceChannels<MTU, 1> = EmbassyInterfaceChannels::new();
        static WAKE: WakeSignal = new_wake_signal();
        let EmbassyInterfaceSeam {
            mut worker_context,
            runtime_handle: _runtime_handle,
        } = EmbassyInterfaceSeam::split(id(), &CH, &WAKE);

        worker_context
            .inbound
            .submit(|buf| {
                buf[0] = 0x01;
                1
            })
            .unwrap();
        let _ = WAKE.try_take();
        assert_eq!(
            worker_context
                .inbound
                .submit(|_buf| panic!("no grant was available, fill must not run")),
            Err(QueueFull)
        );
        assert!(WAKE.try_take().is_some());
    }

    #[test]
    fn inbound_packets_drain_borrowed_and_byte_exact() {
        static CH: EmbassyInterfaceChannels<MTU, 4> = EmbassyInterfaceChannels::new();
        static WAKE: WakeSignal = new_wake_signal();
        let EmbassyInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = EmbassyInterfaceSeam::split(id(), &CH, &WAKE);

        worker_context
            .inbound
            .submit(|buf| {
                buf[..3].copy_from_slice(&[1, 2, 3]);
                3
            })
            .unwrap();
        worker_context
            .inbound
            .submit(|buf| {
                buf[..2].copy_from_slice(&[4, 5]);
                2
            })
            .unwrap();

        let mut seen: std::vec::Vec<(std::vec::Vec<u8>, InterfaceId)> = std::vec::Vec::new();
        let drained = runtime_handle.drain_inbound(|packet| {
            seen.push((packet.bytes.to_vec(), packet.source_interface));
        });

        assert_eq!(drained, 2);
        assert_eq!(
            seen,
            std::vec![(std::vec![1, 2, 3], id()), (std::vec![4, 5], id()),]
        );
        assert_eq!(runtime_handle.drain_inbound(|_packet| {}), 0);
    }

    #[test]
    fn drain_each_consumes_unconditionally_in_order() {
        static CH: EmbassyInterfaceChannels<MTU, 4> = EmbassyInterfaceChannels::new();
        static WAKE: WakeSignal = new_wake_signal();
        let EmbassyInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = EmbassyInterfaceSeam::split(id(), &CH, &WAKE);

        grant(&mut runtime_handle, 0x11, 1);
        grant(&mut runtime_handle, 0x22, 2);

        let mut seen: std::vec::Vec<std::vec::Vec<u8>> = std::vec::Vec::new();
        let drained = worker_context
            .outbound
            .drain_each(|packet| seen.push(packet.bytes.to_vec()));

        assert_eq!(drained, 2);
        assert_eq!(seen, std::vec![std::vec![0x11], std::vec![0x22, 0x22]]);
        assert!(worker_context.outbound.lease().is_none());
    }
}
