//! Std implementation of the per-interface runtime lanes. Inbound and outbound
//! use lock-free [`rtrb`] SPSC rings; control uses separate command/report rings.
//! The interface loop holds the [`InterfaceWorkerContext`], and the runtime holds
//! the [`StdInterfaceHandle`].

use std::sync::mpsc::SyncSender;
use std::time::Instant;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::engine::{InboundPacket, InstantMillis, OutboundPacket};
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundSink, InterfaceHandle, InterfaceId,
    InterfaceWorkerContext, OutboundDrain, QueueFull, Substrate,
};

/// Control-lane depth. Lifecycle signals are rare, so a few slots is ample.
const CONTROL_DEPTH: usize = 4;

/// One inbound slot: the worker fills `bytes[..len]` in place and the sink stamps
/// `arrived_at`. Rides the ring by value — an MTU-bounded `memcpy`, never a heap
/// allocation.
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

/// Producer wrapper that wakes the host after every publish attempt. The wake is
/// coalesced by the host-side channel, and it fires even when the ring is full so
/// backpressure wakes the runtime to drain.
struct WakingProducer<T> {
    ring: Producer<T>,
    wake: SyncSender<()>,
}

impl<T> WakingProducer<T> {
    /// Publish `value`, returning whether it fit, and wake the host either way.
    fn push(&mut self, value: T) -> bool {
        let pushed = self.ring.push(value).is_ok();
        let _ = self.wake.try_send(());
        pushed
    }
}

/// The worker-held producer end of one interface's inbound ring. It stamps
/// arrival times from the host's shared monotonic origin.
pub struct StdInboundSink<const MTU: usize> {
    producer: WakingProducer<InboundSlot<MTU>>,
    clock_base: Instant,
}

impl<const MTU: usize> InboundSink for StdInboundSink<MTU> {
    fn submit(&mut self, fill: impl FnOnce(&mut [u8]) -> usize) -> Result<(), QueueFull> {
        let mut slot = InboundSlot {
            arrived_at: InstantMillis(self.clock_base.elapsed().as_millis() as u64),
            len: 0,
            bytes: [0u8; MTU],
        };
        slot.len = fill(&mut slot.bytes) as u16;
        if self.producer.push(slot) {
            Ok(())
        } else {
            Err(QueueFull)
        }
    }
}

/// The consumer end of one interface's outbound ring (held by the worker loop).
pub struct StdOutboundDrain<const MTU: usize> {
    consumer: Consumer<OutboundSlot<MTU>>,
}

impl<const MTU: usize> OutboundDrain for StdOutboundDrain<MTU> {
    fn drain_each(&mut self, mut write: impl FnMut(OutboundPacket<'_>)) -> usize {
        let mut drained = 0;
        while let Ok(slot) = self.consumer.pop() {
            write(OutboundPacket::new(&slot.bytes[..slot.len as usize]));
            drained += 1;
        }
        drained
    }
}

/// The worker's end of the control lane: pulls commands the runtime issued, pushes
/// reports back. Tiny copy enums over their own SPSC lanes.
pub struct StdControlEndpoint {
    commands: Consumer<ControlCommand>,
    reports: WakingProducer<ControlReport>,
}

impl ControlEndpoint for StdControlEndpoint {
    fn next_command(&mut self) -> Option<ControlCommand> {
        self.commands.pop().ok()
    }

    fn report(&mut self, report: ControlReport) {
        // A full report lane means the runtime hasn't drained yet; dropping a
        // duplicate lifecycle signal is benign (the runtime re-checks each cycle).
        let _ = self.reports.push(report);
    }
}

/// The std platform's worker-side lane-end types, bundled for [`InterfaceWorkerContext`].
pub struct StdHostSubstrate<const MTU: usize>;

impl<const MTU: usize> Substrate for StdHostSubstrate<MTU> {
    type InboundSink = StdInboundSink<MTU>;
    type OutboundDrain = StdOutboundDrain<MTU>;
    type Control = StdControlEndpoint;
}

/// The runtime-held end of one interface's lanes.
pub struct StdInterfaceHandle<const MTU: usize> {
    id: InterfaceId,
    inbound: Consumer<InboundSlot<MTU>>,
    outbound: Producer<OutboundSlot<MTU>>,
    commands: Producer<ControlCommand>,
    reports: Consumer<ControlReport>,
}

impl<const MTU: usize> InterfaceHandle for StdInterfaceHandle<MTU> {
    fn drain_inbound(&mut self, mut f: impl FnMut(InboundPacket<'_>)) -> usize {
        let mut drained = 0;
        while let Ok(slot) = self.inbound.pop() {
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
        self.outbound.push(slot).is_ok()
    }

    fn request_stop(&mut self) {
        let _ = self.commands.push(ControlCommand::Stop);
    }

    fn next_report(&mut self) -> Option<ControlReport> {
        self.reports.pop().ok()
    }
}

/// The two halves of a freshly built interface seam: the [`InterfaceWorkerContext`]
/// the interface's loop is handed, and the [`StdInterfaceHandle`] the runtime keeps.
pub struct StdInterfaceSeam<const MTU: usize> {
    pub worker_context: InterfaceWorkerContext<StdHostSubstrate<MTU>>,
    pub runtime_handle: StdInterfaceHandle<MTU>,
}

impl<const MTU: usize> StdInterfaceSeam<MTU> {
    /// Build one interface's data and control lanes. Pass the same `clock_base`
    /// to every seam so arrivals share the engine's timebase; `wake` is the
    /// host's sleep-notify channel.
    pub fn new(id: InterfaceId, clock_base: Instant, depth: usize, wake: SyncSender<()>) -> Self {
        let (in_producer, in_consumer) = RingBuffer::<InboundSlot<MTU>>::new(depth);
        let (out_producer, out_consumer) = RingBuffer::<OutboundSlot<MTU>>::new(depth);
        let (cmd_producer, cmd_consumer) = RingBuffer::<ControlCommand>::new(CONTROL_DEPTH);
        let (report_producer, report_consumer) = RingBuffer::<ControlReport>::new(CONTROL_DEPTH);

        StdInterfaceSeam {
            worker_context: InterfaceWorkerContext {
                inbound: StdInboundSink {
                    producer: WakingProducer {
                        ring: in_producer,
                        wake: wake.clone(),
                    },
                    clock_base,
                },
                outbound: StdOutboundDrain {
                    consumer: out_consumer,
                },
                control: StdControlEndpoint {
                    commands: cmd_consumer,
                    reports: WakingProducer {
                        ring: report_producer,
                        wake,
                    },
                },
            },
            runtime_handle: StdInterfaceHandle {
                id,
                inbound: in_consumer,
                outbound: out_producer,
                commands: cmd_producer,
                reports: report_consumer,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;
    use std::vec::Vec;

    const MTU: usize = 64;
    const DEPTH: usize = 8;

    fn id() -> InterfaceId {
        InterfaceId::new([0x11; 16])
    }

    #[test]
    fn seam_round_trips_all_three_lanes() {
        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = StdInterfaceSeam::<MTU>::new(id(), Instant::now(), DEPTH, wake_tx);

        // runtime → worker: queue an outbound packet, worker drains it in place.
        assert!(runtime_handle.send(OutboundPacket::new(&[0xAA, 0xBB, 0xCC])));
        let mut transmitted: Vec<Vec<u8>> = Vec::new();
        let drained = worker_context
            .outbound
            .drain_each(|pkt| transmitted.push(pkt.bytes.to_vec()));
        assert_eq!(drained, 1);
        assert_eq!(transmitted, std::vec![std::vec![0xAA, 0xBB, 0xCC]]);

        // worker → runtime: stamp an inbound packet, runtime drains it tagged.
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

        // control: runtime asks to stop, worker sees it and reports back.
        assert!(worker_context.control.next_command().is_none());
        runtime_handle.request_stop();
        assert!(matches!(
            worker_context.control.next_command(),
            Some(ControlCommand::Stop)
        ));
        worker_context.control.report(ControlReport::Stopped);
        assert!(matches!(
            runtime_handle.next_report(),
            Some(ControlReport::Stopped)
        ));
    }

    #[test]
    fn submit_and_report_wake_the_host() {
        let (wake_tx, wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            mut worker_context,
            runtime_handle: _runtime_handle,
        } = StdInterfaceSeam::<MTU>::new(id(), Instant::now(), DEPTH, wake_tx);

        // Nothing pushed yet → the host has no reason to wake.
        assert!(wake_rx.try_recv().is_err());

        // An inbound submit rouses the host automatically — the wake is baked into
        // the producer, so the worker never touches a separate signal.
        worker_context
            .inbound
            .submit(|buf| {
                buf[0] = 1;
                1
            })
            .unwrap();
        assert!(wake_rx.try_recv().is_ok());

        // So does a control report (the report producer wakes the host too).
        worker_context.control.report(ControlReport::Stopped);
        assert!(wake_rx.try_recv().is_ok());
    }

    #[test]
    fn seam_halves_send_across_threads() {
        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            worker_context,
            mut runtime_handle,
        } = StdInterfaceSeam::<MTU>::new(id(), Instant::now(), DEPTH, wake_tx);

        // The worker context moves to its own thread — proves both halves are
        // `Send`, the whole point of the SPSC seam.
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
