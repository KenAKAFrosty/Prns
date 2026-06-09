use std::sync::mpsc::SyncSender;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::engine::InstantMillis;
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundPacket, InboundSink, Interface,
    InterfaceHandle, InterfaceId, InterfaceWorkerContext, OutboundDrain, OutboundPacket, QueueFull,
    SendError, StartedInterface, Substrate,
};

const CONTROL_DEPTH: usize = 4;

#[derive(Clone, Default)]
pub struct WorkerWake(Arc<OnceLock<Box<dyn Fn() + Send + Sync>>>);

impl WorkerWake {
    pub fn arm(&self, notify: impl Fn() + Send + Sync + 'static) {
        let _ = self.0.set(Box::new(notify));
    }

    fn signal(&self) {
        if let Some(notify) = self.0.get() {
            notify();
        }
    }
}

struct InboundSlot<const MTU: usize> {
    arrived_at: InstantMillis,
    len: u16,
    bytes: [u8; MTU],
}

impl<const MTU: usize> Default for InboundSlot<MTU> {
    fn default() -> Self {
        Self {
            arrived_at: InstantMillis(0),
            len: 0,
            bytes: [0u8; MTU],
        }
    }
}

struct OutboundSlot<const MTU: usize> {
    len: u16,
    bytes: [u8; MTU],
}

impl<const MTU: usize> Default for OutboundSlot<MTU> {
    fn default() -> Self {
        Self {
            len: 0,
            bytes: [0u8; MTU],
        }
    }
}

pub struct StdInboundSink<const MTU: usize> {
    ring: Producer<InboundSlot<MTU>>,
    wake: SyncSender<()>,
    clock_base: Instant,
}

impl<const MTU: usize> InboundSink for StdInboundSink<MTU> {
    fn submit(&mut self, fill: impl FnOnce(&mut [u8]) -> usize) -> Result<(), QueueFull> {
        // The wake fires on both paths — a full ring is exactly when the host
        // should wake to drain. Coalesced by the host-side channel.
        let Ok(mut chunk) = self.ring.write_chunk(1) else {
            let _ = self.wake.try_send(());
            return Err(QueueFull);
        };
        let (granted, _) = chunk.as_mut_slices();
        let slot = &mut granted[0];
        slot.arrived_at = InstantMillis(self.clock_base.elapsed().as_millis() as u64);
        slot.len = fill(&mut slot.bytes) as u16;
        chunk.commit(1);
        let _ = self.wake.try_send(());
        Ok(())
    }
}

pub struct StdOutboundDrain<const MTU: usize> {
    consumer: Consumer<OutboundSlot<MTU>>,
    worker_wake: WorkerWake,
}

impl<const MTU: usize> StdOutboundDrain<MTU> {
    pub fn arm_wake(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker_wake.arm(notify);
    }

    pub fn drain_one(&mut self, write: impl FnOnce(OutboundPacket<'_>)) -> bool {
        let Ok(chunk) = self.consumer.read_chunk(1) else {
            return false;
        };
        let (read, _) = chunk.as_slices();
        let slot = &read[0];
        write(OutboundPacket::new(&slot.bytes[..slot.len as usize]));
        chunk.commit(1);
        true
    }
}

impl<const MTU: usize> OutboundDrain for StdOutboundDrain<MTU> {
    fn drain_each(&mut self, mut write: impl FnMut(OutboundPacket<'_>)) -> usize {
        let mut drained = 0;
        while let Ok(chunk) = self.consumer.read_chunk(1) {
            let (read, _) = chunk.as_slices();
            let slot = &read[0];
            write(OutboundPacket::new(&slot.bytes[..slot.len as usize]));
            chunk.commit(1);
            drained += 1;
        }
        drained
    }
}

pub struct StdControlEndpoint {
    commands: Consumer<ControlCommand>,
    reports: Producer<ControlReport>,
    wake: SyncSender<()>,
}

impl ControlEndpoint for StdControlEndpoint {
    fn next_command(&mut self) -> Option<ControlCommand> {
        self.commands.pop().ok()
    }

    fn report(&mut self, report: ControlReport) {
        // A full report lane means the runtime hasn't drained yet; dropping a
        // duplicate lifecycle signal is benign (the runtime re-checks each cycle).
        let _ = self.reports.push(report);
        let _ = self.wake.try_send(());
    }
}

pub struct StdHostSubstrate<const MTU: usize>;

impl<const MTU: usize> Substrate for StdHostSubstrate<MTU> {
    type InboundSink = StdInboundSink<MTU>;
    type OutboundDrain = StdOutboundDrain<MTU>;
    type Control = StdControlEndpoint;
}

pub struct StdInterfaceHandle<const MTU: usize> {
    id: InterfaceId,
    inbound: Consumer<InboundSlot<MTU>>,
    outbound: Producer<OutboundSlot<MTU>>,
    commands: Producer<ControlCommand>,
    reports: Consumer<ControlReport>,
    worker_wake: WorkerWake,
}

impl<const MTU: usize> InterfaceHandle for StdInterfaceHandle<MTU> {
    fn next_inbound<R>(&mut self, f: impl FnOnce(InboundPacket<'_>) -> R) -> Option<R> {
        let Ok(mut chunk) = self.inbound.read_chunk(1) else {
            return None;
        };
        let (read, _) = chunk.as_mut_slices();
        let slot = &mut read[0];
        let result = f(InboundPacket {
            arrived_at: slot.arrived_at,
            source_interface: self.id,
            bytes: &mut slot.bytes[..slot.len as usize],
        });
        chunk.commit(1);
        Some(result)
    }

    fn acquire_send_grant(
        &mut self,
        fill: impl FnOnce(&mut [u8]) -> usize,
    ) -> Result<usize, SendError> {
        let Ok(mut chunk) = self.outbound.write_chunk(1) else {
            return Err(SendError::QueueFull);
        };
        let (granted, _) = chunk.as_mut_slices();
        let slot = &mut granted[0];
        let written = fill(&mut slot.bytes);
        slot.len = written as u16;
        chunk.commit(1);
        self.worker_wake.signal();
        Ok(written)
    }

    fn request_stop(&mut self) {
        let _ = self.commands.push(ControlCommand::Stop);
        self.worker_wake.signal();
    }

    fn next_report(&mut self) -> Option<ControlReport> {
        self.reports.pop().ok()
    }
}

pub struct StdInterfaceSeam<const MTU: usize> {
    pub worker_context: InterfaceWorkerContext<StdHostSubstrate<MTU>>,
    pub runtime_handle: StdInterfaceHandle<MTU>,
}

impl<const MTU: usize> StdInterfaceSeam<MTU> {
    pub fn new(
        id: InterfaceId,
        clock_base: Instant,
        max_buffered_packets: usize,
        wake: SyncSender<()>,
    ) -> Self {
        let (in_producer, in_consumer) = RingBuffer::<InboundSlot<MTU>>::new(max_buffered_packets);
        let (out_producer, out_consumer) =
            RingBuffer::<OutboundSlot<MTU>>::new(max_buffered_packets);
        let (cmd_producer, cmd_consumer) = RingBuffer::<ControlCommand>::new(CONTROL_DEPTH);
        let (report_producer, report_consumer) = RingBuffer::<ControlReport>::new(CONTROL_DEPTH);
        let worker_wake = WorkerWake::default();

        StdInterfaceSeam {
            worker_context: InterfaceWorkerContext {
                inbound: StdInboundSink {
                    ring: in_producer,
                    wake: wake.clone(),
                    clock_base,
                },
                outbound: StdOutboundDrain {
                    consumer: out_consumer,
                    worker_wake: worker_wake.clone(),
                },
                control: StdControlEndpoint {
                    commands: cmd_consumer,
                    reports: report_producer,
                    wake,
                },
            },
            runtime_handle: StdInterfaceHandle {
                id,
                inbound: in_consumer,
                outbound: out_producer,
                commands: cmd_producer,
                reports: report_consumer,
                worker_wake,
            },
        }
    }

    pub fn start_interface<I>(
        self,
        interface: I,
    ) -> StartedInterface<StdInterfaceHandle<MTU>, I::Worker>
    where
        I: Interface<StdHostSubstrate<MTU>>,
    {
        let descriptor = interface.descriptor();
        let drive = interface.start(self.worker_context);
        StartedInterface {
            descriptor,
            connection: crate::interfaces::ConnectionState::Initializing,
            handle: self.runtime_handle,
            drive,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;
    use std::vec::Vec;

    use crate::interfaces::ConnectionState;

    const MTU: usize = 64;
    const MAX_BUFFERED_PACKETS: usize = 8;

    fn id() -> InterfaceId {
        InterfaceId::new([0x11; 16])
    }

    #[test]
    fn seam_round_trips_all_three_lanes() {
        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = StdInterfaceSeam::<MTU>::new(id(), Instant::now(), MAX_BUFFERED_PACKETS, wake_tx);

        assert_eq!(
            runtime_handle.acquire_send_grant(|buf| {
                buf[..3].copy_from_slice(&[0xAA, 0xBB, 0xCC]);
                3
            }),
            Ok(3)
        );
        let mut transmitted: Vec<Vec<u8>> = Vec::new();
        let drained = worker_context
            .outbound
            .drain_each(|pkt| transmitted.push(pkt.bytes.to_vec()));
        assert_eq!(drained, 1);
        assert_eq!(transmitted, std::vec![std::vec![0xAA, 0xBB, 0xCC]]);

        worker_context
            .inbound
            .submit(|buf| {
                buf[..2].copy_from_slice(&[0xDE, 0xAD]);
                2
            })
            .unwrap();
        let mut received: Vec<Vec<u8>> = Vec::new();
        let drained = runtime_handle.drain_inbound(|pkt| {
            assert_eq!(pkt.source_interface, id());
            received.push(pkt.bytes.to_vec());
        });
        assert_eq!(drained, 1);
        assert_eq!(received, std::vec![std::vec![0xDE, 0xAD]]);

        assert!(worker_context.control.next_command().is_none());
        runtime_handle.request_stop();
        assert!(matches!(
            worker_context.control.next_command(),
            Some(ControlCommand::Stop)
        ));
        worker_context
            .control
            .report(ControlReport::ConnectionState(ConnectionState::Degraded));
        assert!(matches!(
            runtime_handle.next_report(),
            Some(ControlReport::ConnectionState(ConnectionState::Degraded))
        ));
        worker_context.control.report(ControlReport::Stopped);
        assert!(matches!(
            runtime_handle.next_report(),
            Some(ControlReport::Stopped)
        ));
    }

    #[test]
    fn a_packet_can_be_answered_on_its_interface_between_inbound_steps() {
        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = StdInterfaceSeam::<MTU>::new(id(), Instant::now(), MAX_BUFFERED_PACKETS, wake_tx);

        for byte in [0x01u8, 0x02] {
            worker_context
                .inbound
                .submit(|buf| {
                    buf[0] = byte;
                    1
                })
                .unwrap();
        }

        let first = runtime_handle
            .next_inbound(|pkt| pkt.bytes.to_vec())
            .unwrap();
        assert_eq!(first, std::vec![0x01]);
        assert_eq!(
            runtime_handle.acquire_send_grant(|buf| {
                buf[..2].copy_from_slice(&[0xF0, first[0]]);
                2
            }),
            Ok(2)
        );

        let second = runtime_handle
            .next_inbound(|pkt| pkt.bytes.to_vec())
            .unwrap();
        assert_eq!(second, std::vec![0x02]);
        assert!(runtime_handle.next_inbound(|_| ()).is_none());

        let mut replies: Vec<Vec<u8>> = Vec::new();
        worker_context
            .outbound
            .drain_each(|pkt| replies.push(pkt.bytes.to_vec()));
        assert_eq!(replies, std::vec![std::vec![0xF0, 0x01]]);
    }

    #[test]
    fn submit_and_report_wake_the_host() {
        let (wake_tx, wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            mut worker_context,
            runtime_handle: _runtime_handle,
        } = StdInterfaceSeam::<MTU>::new(id(), Instant::now(), MAX_BUFFERED_PACKETS, wake_tx);

        assert!(wake_rx.try_recv().is_err());

        worker_context
            .inbound
            .submit(|buf| {
                buf[0] = 1;
                1
            })
            .unwrap();
        assert!(wake_rx.try_recv().is_ok());

        worker_context.control.report(ControlReport::Stopped);
        assert!(wake_rx.try_recv().is_ok());
    }

    #[test]
    fn a_full_inbound_ring_reports_queue_full_without_running_the_fill() {
        let (wake_tx, wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            mut worker_context,
            runtime_handle: _runtime_handle,
        } = StdInterfaceSeam::<MTU>::new(id(), Instant::now(), 1, wake_tx);

        worker_context
            .inbound
            .submit(|buf| {
                buf[0] = 0x01;
                1
            })
            .unwrap();
        let _ = wake_rx.try_recv();
        assert_eq!(
            worker_context
                .inbound
                .submit(|_buf| panic!("no grant was available, fill must not run")),
            Err(QueueFull)
        );
        assert!(wake_rx.try_recv().is_ok());
    }

    #[test]
    fn a_full_queue_reports_queue_full_without_running_the_fill() {
        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            worker_context: _worker_context,
            mut runtime_handle,
        } = StdInterfaceSeam::<MTU>::new(id(), Instant::now(), 1, wake_tx);

        assert_eq!(
            runtime_handle.acquire_send_grant(|buf| {
                buf[0] = 0x01;
                1
            }),
            Ok(1)
        );
        assert_eq!(
            runtime_handle
                .acquire_send_grant(|_buf| panic!("no grant was available, fill must not run")),
            Err(SendError::QueueFull)
        );
    }

    #[test]
    fn seam_halves_send_across_threads() {
        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            worker_context,
            mut runtime_handle,
        } = StdInterfaceSeam::<MTU>::new(id(), Instant::now(), MAX_BUFFERED_PACKETS, wake_tx);

        let worker = std::thread::spawn(move || {
            let mut worker_context = worker_context;
            worker_context
                .inbound
                .submit(|buf| {
                    buf[0] = 0x42;
                    1
                })
                .unwrap();
            worker_context.control.report(ControlReport::Stopped);
        });
        worker.join().unwrap();

        // `join` establishes happens-before, so the worker's stamps are visible.
        let mut received: Vec<Vec<u8>> = Vec::new();
        runtime_handle.drain_inbound(|pkt| received.push(pkt.bytes.to_vec()));
        assert_eq!(received, std::vec![std::vec![0x42]]);
        assert!(matches!(
            runtime_handle.next_report(),
            Some(ControlReport::Stopped)
        ));
    }
}
