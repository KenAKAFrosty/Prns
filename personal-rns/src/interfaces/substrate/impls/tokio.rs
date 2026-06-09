use std::sync::Arc;
use std::time::Instant;

use rtrb::chunks::ReadChunk;
use rtrb::{Consumer, Producer, RingBuffer};
use tokio::sync::Notify;

use crate::engine::InstantMillis;
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InboundPacket, InboundSink, Interface,
    InterfaceHandle, InterfaceId, InterfaceWorkerContext, OutboundDrain, OutboundPacket, QueueFull,
    SendError, StartedInterface, Substrate,
};

const CONTROL_DEPTH: usize = 4;

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

pub struct TokioInboundSink<const MTU: usize> {
    ring: Producer<InboundSlot<MTU>>,
    wake: Arc<Notify>,
    clock_base: Instant,
}

impl<const MTU: usize> InboundSink for TokioInboundSink<MTU> {
    fn submit(&mut self, fill: impl FnOnce(&mut [u8]) -> usize) -> Result<(), QueueFull> {
        // The wake fires on both paths. A full ring is exactly when the host
        // should wake to drain. `Notify` coalesces: one stored permit.
        let Ok(mut chunk) = self.ring.write_chunk(1) else {
            self.wake.notify_one();
            return Err(QueueFull);
        };
        let (granted, _) = chunk.as_mut_slices();
        let slot = &mut granted[0];
        slot.arrived_at = InstantMillis(self.clock_base.elapsed().as_millis() as u64);
        slot.len = fill(&mut slot.bytes) as u16;
        chunk.commit(1);
        self.wake.notify_one();
        Ok(())
    }
}

pub struct TokioOutboundDrain<const MTU: usize> {
    consumer: Consumer<OutboundSlot<MTU>>,
    ready: Arc<Notify>,
}

impl<const MTU: usize> OutboundDrain for TokioOutboundDrain<MTU> {
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

impl<const MTU: usize> TokioOutboundDrain<MTU> {
    /// Resolves once at least one outbound packet is queued, without dequeuing
    /// it (only completing a lease consumes).
    pub async fn ready(&mut self) {
        while self.consumer.is_empty() {
            self.ready.notified().await;
        }
    }

    pub fn lease(&mut self) -> Option<TokioOutboundLease<'_, MTU>> {
        let chunk = self.consumer.read_chunk(1).ok()?;
        Some(TokioOutboundLease { chunk })
    }
}

/// A queued outbound packet, borrowed in place from its ring slot. The packet
/// stays queued until [`complete`](Self::complete). Dropping the lease
/// instead (including by a cancelled send future, which `tokio::select!`
/// produces on every loop iteration) leaves it at the head of the queue, so an
/// interrupted transmission retries rather than losing the packet.
/// [`keep`](Self::keep) is the same outcome as a named act.
///
/// Every example below is a `compile_fail` doctest, not sample code. The
/// test suite asserts that the compiler *rejects* it, pinned to the expected
/// error code, so each misuse shown is a checked guarantee.
///
/// Consuming is by value, so a packet can never be completed twice:
///
/// ```compile_fail,E0382
/// fn double_complete(drain: &mut personal_rns::interfaces::substrate::TokioOutboundDrain<500>) {
///     let lease = drain.lease().unwrap();
///     lease.complete();
///     lease.complete();
/// }
/// ```
///
/// ...its bytes are unreachable once consumed:
///
/// ```compile_fail,E0382
/// fn packet_after_complete(drain: &mut personal_rns::interfaces::substrate::TokioOutboundDrain<500>) {
///     let lease = drain.lease().unwrap();
///     lease.complete();
///     let _ = lease.packet();
/// }
/// ```
///
/// ...and the lease holds the drain exclusively, so a second packet can't be
/// leased while one is in hand:
///
/// ```compile_fail,E0499
/// fn two_leases(drain: &mut personal_rns::interfaces::substrate::TokioOutboundDrain<500>) {
///     let first = drain.lease().unwrap();
///     let second = drain.lease();
///     let _ = first.packet();
/// }
/// ```
#[must_use = "dropping a lease leaves the packet queued; complete() consumes it; keep() names the retry"]
pub struct TokioOutboundLease<'d, const MTU: usize> {
    chunk: ReadChunk<'d, OutboundSlot<MTU>>,
}

impl<const MTU: usize> TokioOutboundLease<'_, MTU> {
    pub fn packet(&self) -> &[u8] {
        let (read, _) = self.chunk.as_slices();
        let slot = &read[0];
        &slot.bytes[..slot.len as usize]
    }

    pub fn complete(self) {
        self.chunk.commit(1);
    }

    /// Leave the packet queued; the next lease returns it again.
    pub fn keep(self) {}
}

pub struct TokioControlEndpoint {
    commands: Consumer<ControlCommand>,
    command_ready: Arc<Notify>,
    reports: Producer<ControlReport>,
    wake: Arc<Notify>,
}

impl ControlEndpoint for TokioControlEndpoint {
    fn next_command(&mut self) -> Option<ControlCommand> {
        self.commands.pop().ok()
    }

    fn report(&mut self, report: ControlReport) {
        // A full report lane means the runtime hasn't drained yet; dropping a
        // duplicate lifecycle signal is benign (the runtime re-checks each cycle).
        let _ = self.reports.push(report);
        self.wake.notify_one();
    }
}

impl TokioControlEndpoint {
    pub async fn await_command(&mut self) -> ControlCommand {
        loop {
            if let Ok(command) = self.commands.pop() {
                return command;
            }
            self.command_ready.notified().await;
        }
    }
}

pub struct TokioHostSubstrate<const MTU: usize>;

impl<const MTU: usize> Substrate for TokioHostSubstrate<MTU> {
    type InboundSink = TokioInboundSink<MTU>;
    type OutboundDrain = TokioOutboundDrain<MTU>;
    type Control = TokioControlEndpoint;
}

pub struct TokioInterfaceHandle<const MTU: usize> {
    id: InterfaceId,
    inbound: Consumer<InboundSlot<MTU>>,
    outbound: Producer<OutboundSlot<MTU>>,
    outbound_ready: Arc<Notify>,
    commands: Producer<ControlCommand>,
    command_ready: Arc<Notify>,
    reports: Consumer<ControlReport>,
}

impl<const MTU: usize> InterfaceHandle for TokioInterfaceHandle<MTU> {
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
        self.outbound_ready.notify_one();
        Ok(written)
    }

    fn request_stop(&mut self) {
        let _ = self.commands.push(ControlCommand::Stop);
        self.command_ready.notify_one();
    }

    fn next_report(&mut self) -> Option<ControlReport> {
        self.reports.pop().ok()
    }
}

pub struct TokioInterfaceSeam<const MTU: usize> {
    pub worker_context: InterfaceWorkerContext<TokioHostSubstrate<MTU>>,
    pub runtime_handle: TokioInterfaceHandle<MTU>,
}

impl<const MTU: usize> TokioInterfaceSeam<MTU> {
    pub fn new(
        id: InterfaceId,
        clock_base: Instant,
        max_buffered_packets: usize,
        wake: Arc<Notify>,
    ) -> Self {
        let (in_producer, in_consumer) = RingBuffer::<InboundSlot<MTU>>::new(max_buffered_packets);
        let (out_producer, out_consumer) =
            RingBuffer::<OutboundSlot<MTU>>::new(max_buffered_packets);
        let (cmd_producer, cmd_consumer) = RingBuffer::<ControlCommand>::new(CONTROL_DEPTH);
        let (report_producer, report_consumer) = RingBuffer::<ControlReport>::new(CONTROL_DEPTH);
        let outbound_ready = Arc::new(Notify::new());
        let command_ready = Arc::new(Notify::new());

        TokioInterfaceSeam {
            worker_context: InterfaceWorkerContext {
                inbound: TokioInboundSink {
                    ring: in_producer,
                    wake: wake.clone(),
                    clock_base,
                },
                outbound: TokioOutboundDrain {
                    consumer: out_consumer,
                    ready: outbound_ready.clone(),
                },
                control: TokioControlEndpoint {
                    commands: cmd_consumer,
                    command_ready: command_ready.clone(),
                    reports: report_producer,
                    wake,
                },
            },
            runtime_handle: TokioInterfaceHandle {
                id,
                inbound: in_consumer,
                outbound: out_producer,
                outbound_ready,
                commands: cmd_producer,
                command_ready,
                reports: report_consumer,
            },
        }
    }

    pub fn start_interface<I>(
        self,
        interface: I,
    ) -> StartedInterface<TokioInterfaceHandle<MTU>, I::Worker>
    where
        I: Interface<TokioHostSubstrate<MTU>>,
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
    use core::future::Future;
    use core::task::{Context, Waker};
    use std::pin::pin;
    use std::sync::Mutex;
    use std::time::Duration;
    use std::vec;
    use std::vec::Vec;

    const MTU: usize = 64;
    const WATCHDOG: Duration = Duration::from_secs(5);

    fn id() -> InterfaceId {
        InterfaceId::new([0x44; 16])
    }

    fn seam(max_buffered_packets: usize) -> TokioInterfaceSeam<MTU> {
        TokioInterfaceSeam::new(
            id(),
            Instant::now(),
            max_buffered_packets,
            Arc::new(Notify::new()),
        )
    }

    fn grant(handle: &mut TokioInterfaceHandle<MTU>, byte: u8, len: usize) {
        let written = handle
            .acquire_send_grant(|buf| {
                buf[..len].fill(byte);
                len
            })
            .unwrap();
        assert_eq!(written, len);
    }

    #[test]
    fn a_packet_can_be_answered_on_its_interface_between_inbound_steps() {
        let TokioInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam(4);

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
        assert_eq!(first, vec![0x01]);
        let written = runtime_handle
            .acquire_send_grant(|buf| {
                buf[..2].copy_from_slice(&[0xF0, first[0]]);
                2
            })
            .unwrap();
        assert_eq!(written, 2);

        let second = runtime_handle
            .next_inbound(|pkt| pkt.bytes.to_vec())
            .unwrap();
        assert_eq!(second, vec![0x02]);
        assert!(runtime_handle.next_inbound(|_| ()).is_none());

        let lease = worker_context.outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0xF0, 0x01]);
        lease.complete();
        assert!(worker_context.outbound.lease().is_none());
    }

    #[test]
    fn leases_consume_packets_in_order_and_byte_exact() {
        let TokioInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam(4);

        grant(&mut runtime_handle, 0xAA, 3);
        grant(&mut runtime_handle, 0xBB, 5);

        let lease = worker_context.outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0xAA, 0xAA, 0xAA]);
        lease.complete();

        let lease = worker_context.outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0xBB; 5]);
        lease.complete();

        assert!(worker_context.outbound.lease().is_none());
    }

    #[test]
    fn a_kept_lease_returns_the_same_packet_next_time() {
        let TokioInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam(4);

        grant(&mut runtime_handle, 0xAA, 2);
        grant(&mut runtime_handle, 0xBB, 2);

        let lease = worker_context.outbound.lease().unwrap();
        lease.keep();

        let lease = worker_context.outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0xAA, 0xAA]);
        lease.complete();

        let lease = worker_context.outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0xBB, 0xBB]);
        lease.complete();
    }

    #[test]
    fn a_cancelled_send_future_keeps_the_packet_for_retry() {
        let TokioInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam(4);

        grant(&mut runtime_handle, 0xEE, 4);

        {
            let send_attempt = async {
                let lease = worker_context.outbound.lease().unwrap();
                let _ = lease.packet();
                core::future::pending::<()>().await;
                lease.complete();
            };
            let mut send_attempt = pin!(send_attempt);
            let mut context = Context::from_waker(Waker::noop());
            assert!(send_attempt.as_mut().poll(&mut context).is_pending());
        }

        let lease = worker_context.outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0xEE; 4]);
        lease.complete();
        assert!(worker_context.outbound.lease().is_none());
    }

    #[test]
    fn completing_a_lease_frees_the_slot_for_the_producer_across_wraparound() {
        let TokioInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam(2);

        for round in 0u8..5 {
            grant(&mut runtime_handle, round, 1);
            grant(&mut runtime_handle, round + 100, 1);
            assert_eq!(
                runtime_handle.acquire_send_grant(|_buf| panic!("ring is full, fill must not run")),
                Err(SendError::QueueFull)
            );

            let lease = worker_context.outbound.lease().unwrap();
            assert_eq!(lease.packet(), &[round]);
            lease.complete();
            let lease = worker_context.outbound.lease().unwrap();
            assert_eq!(lease.packet(), &[round + 100]);
            lease.complete();
        }
        assert!(worker_context.outbound.lease().is_none());
    }

    #[test]
    fn a_full_inbound_ring_reports_queue_full_without_running_the_fill() {
        let TokioInterfaceSeam {
            mut worker_context,
            runtime_handle: _runtime_handle,
        } = seam(1);

        worker_context
            .inbound
            .submit(|buf| {
                buf[0] = 0x01;
                1
            })
            .unwrap();
        assert_eq!(
            worker_context
                .inbound
                .submit(|_buf| panic!("no grant was available, fill must not run")),
            Err(QueueFull)
        );
    }

    #[test]
    fn inbound_packets_drain_borrowed_and_byte_exact() {
        let TokioInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam(4);

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

        let mut seen: Vec<(Vec<u8>, InterfaceId)> = Vec::new();
        let drained = runtime_handle.drain_inbound(|packet| {
            seen.push((packet.bytes.to_vec(), packet.source_interface));
        });

        assert_eq!(drained, 2);
        assert_eq!(seen, vec![(vec![1, 2, 3], id()), (vec![4, 5], id())]);
        assert_eq!(runtime_handle.drain_inbound(|_packet| {}), 0);
    }

    #[test]
    fn drain_each_consumes_unconditionally_in_order() {
        let TokioInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam(4);

        grant(&mut runtime_handle, 0x11, 1);
        grant(&mut runtime_handle, 0x22, 2);

        let mut seen: Vec<Vec<u8>> = Vec::new();
        let drained = worker_context
            .outbound
            .drain_each(|packet| seen.push(packet.bytes.to_vec()));

        assert_eq!(drained, 2);
        assert_eq!(seen, vec![vec![0x11], vec![0x22, 0x22]]);
        assert!(worker_context.outbound.lease().is_none());
    }

    #[tokio::test]
    async fn ready_wakes_when_the_runtime_grants_across_tasks() {
        let TokioInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam(4);

        let granter = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            grant(&mut runtime_handle, 0x77, 2);
            runtime_handle
        });

        tokio::time::timeout(WATCHDOG, worker_context.outbound.ready())
            .await
            .expect("ready must resolve once a packet is granted");

        let mut outbound = worker_context.outbound;
        let lease = outbound.lease().unwrap();
        assert_eq!(lease.packet(), &[0x77, 0x77]);
        lease.complete();
        granter.await.unwrap();
    }

    #[tokio::test]
    async fn await_command_wakes_on_request_stop_across_tasks() {
        let TokioInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam(4);

        let stopper = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            runtime_handle.request_stop();
        });

        let command = tokio::time::timeout(WATCHDOG, worker_context.control.await_command())
            .await
            .expect("await_command must resolve once stop is requested");
        assert!(matches!(command, ControlCommand::Stop));
        stopper.await.unwrap();
    }

    /// The drain-property harness for the tokio seam: a real lease-grammar
    /// worker task whose first transmission attempt fails (kept lease), run
    /// against granted packets — every packet reaches the wire exactly once,
    /// in order, within the watchdog.
    #[tokio::test]
    async fn a_leased_worker_delivers_every_packet_exactly_once_despite_a_failed_attempt() {
        let TokioInterfaceSeam {
            mut worker_context,
            mut runtime_handle,
        } = seam(8);

        for byte in [0xA1u8, 0xB2, 0xC3] {
            grant(&mut runtime_handle, byte, 4);
        }

        let sent: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let wire = sent.clone();
        let worker = tokio::spawn(async move {
            let mut failed_once = false;
            loop {
                worker_context.outbound.ready().await;
                while let Some(lease) = worker_context.outbound.lease() {
                    if !failed_once {
                        failed_once = true;
                        lease.keep();
                        continue;
                    }
                    wire.lock().unwrap().push(lease.packet().to_vec());
                    lease.complete();
                }
            }
        });

        tokio::time::timeout(WATCHDOG, async {
            loop {
                if sent.lock().unwrap().len() >= 3 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        })
        .await
        .expect("worker failed to put 3 packets on the wire before the watchdog");
        worker.abort();

        assert_eq!(
            *sent.lock().unwrap(),
            vec![vec![0xA1; 4], vec![0xB2; 4], vec![0xC3; 4]],
        );
    }
}
