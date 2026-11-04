use core::future::poll_fn;
use core::task::Poll;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
use portable_atomic::{AtomicU32, Ordering};

use super::driver::{EmbassyInterfaceStatus, InterfaceQueueWakeSlot};
use crate::interfaces::FrameSink;
use crate::interfaces::PacketPhyStats;
use crate::manifold::grant::{
    FrameSlot, FrameTarget, GrantConsumer, GrantProducer, LaneWriteOutcome, ManifoldLaneReader,
    ManifoldLaneWriter,
};

/// Splits caller-owned storage into a zero-copy frame lane.
pub fn embassy_grant_lane<'a, M: RawMutex, const FRAME: usize>(
    channel: &'a mut zerocopy_channel::Channel<'a, M, FrameSlot<FRAME>>,
) -> (
    EmbassyGrantProducer<'a, M, FRAME>,
    EmbassyGrantConsumer<'a, M, FRAME>,
) {
    let (sender, receiver) = channel.split();
    (
        EmbassyGrantProducer {
            sender,
            granted: false,
            grant_generation: None,
            status: None,
            wake: None,
            pressure_events: None,
        },
        EmbassyGrantConsumer {
            receiver,
            peeked: false,
            status: None,
        },
    )
}

/// Direction of one physical interface's lane relative to the manifold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterfaceQueueDirection {
    Ingress,
    Egress,
}

/// Split a lane whose grants and queued frames follow administrative retirement.
/// Use at most one lane of each direction for a given status: each endpoint has
/// an independent wake slot, so no queue wait consumes the driver's power signal.
pub fn embassy_grant_lane_with_status<'a, M: RawMutex, const FRAME: usize>(
    channel: &'a mut zerocopy_channel::Channel<'a, M, FrameSlot<FRAME>>,
    status: &'a EmbassyInterfaceStatus,
    direction: InterfaceQueueDirection,
) -> (
    EmbassyGrantProducer<'a, M, FRAME>,
    EmbassyGrantConsumer<'a, M, FRAME>,
) {
    let (mut producer, mut consumer) = embassy_grant_lane(channel);
    let (producer_slot, consumer_slot) = match direction {
        InterfaceQueueDirection::Ingress => (
            InterfaceQueueWakeSlot::IngressProducer,
            InterfaceQueueWakeSlot::IngressConsumer,
        ),
        InterfaceQueueDirection::Egress => (
            InterfaceQueueWakeSlot::EgressProducer,
            InterfaceQueueWakeSlot::EgressConsumer,
        ),
    };
    producer.set_status(status, producer_slot);
    consumer.set_status(status, consumer_slot);
    (producer, consumer)
}

pub struct EmbassyGrantProducer<'a, M: RawMutex, const FRAME: usize> {
    sender: zerocopy_channel::Sender<'a, M, FrameSlot<FRAME>>,
    granted: bool,
    grant_generation: Option<u32>,
    status: Option<(&'a EmbassyInterfaceStatus, InterfaceQueueWakeSlot)>,
    wake: Option<&'a Signal<M, ()>>,
    pressure_events: Option<&'a AtomicU32>,
}

impl<'a, M: RawMutex, const FRAME: usize> EmbassyGrantProducer<'a, M, FRAME> {
    /// Bind before the endpoint is used. Each endpoint needs its own wake slot.
    pub(crate) fn set_status(
        &mut self,
        status: &'a EmbassyInterfaceStatus,
        wake_slot: InterfaceQueueWakeSlot,
    ) {
        self.status = Some((status, wake_slot));
    }

    pub fn set_outbound_wake(&mut self, wake: &'a Signal<M, ()>) {
        self.wake = Some(wake);
    }

    pub fn set_pressure_counter(&mut self, pressure_events: &'a AtomicU32) {
        self.pressure_events = Some(pressure_events);
    }

    pub(crate) fn note_pressure(&self) {
        if let Some(pressure_events) = self.pressure_events {
            let _ = pressure_events.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_add(1))
            });
        }
    }
}

impl<M: RawMutex, const FRAME: usize> GrantProducer<FRAME> for EmbassyGrantProducer<'_, M, FRAME> {
    fn try_grant(&mut self) -> Option<&mut FrameSlot<FRAME>> {
        let generation = match self.status {
            Some((status, _)) => Some(status.available_generation()?),
            None => None,
        };
        let granted = &mut self.granted;
        let slot = self.sender.try_send()?;
        if !*granted {
            self.grant_generation = generation;
            *granted = true;
        }
        Some(slot)
    }

    async fn grant(&mut self) -> &mut FrameSlot<FRAME> {
        // The delivery attempt starts before there is necessarily queue space.
        // Keep that epoch while waiting: a pre-disable driver delivery must not
        // become fresh work just because its slot is available only after On.
        let generation = self.status.map(|(status, _)| status.disable_generation());
        poll_fn(|cx| {
            if let Some((status, wake_slot)) = self.status {
                status.register_queue_waker(wake_slot, cx.waker());
                if status.available_generation().is_none() {
                    return Poll::Pending;
                }
            }
            self.sender.poll_send(cx).map(|_| ())
        })
        .await;
        // There is one producer, so a ready send slot cannot be consumed by
        // another sender before this borrow. Keep the generation captured above
        // even if the interface is disabled between readiness and commit.
        let slot = self.sender.send().await;
        if !self.granted {
            self.grant_generation = generation;
            self.granted = true;
        }
        slot
    }

    fn commit(&mut self) {
        if self.granted {
            let valid = match self.status {
                Some((status, _)) => {
                    status.available_generation() == self.grant_generation
                        && self.grant_generation.is_some()
                }
                None => true,
            };
            // Stamp once at first lane admission, never at dequeue/driver handoff.
            if let Some(slot) = self.sender.try_send() {
                if !valid {
                    slot.clear();
                    self.granted = false;
                    self.grant_generation = None;
                    return;
                }
                slot.lane_generation = self.grant_generation;
                slot.enqueued_at.get_or_insert(crate::engine::InstantMillis(
                    embassy_time::Instant::now().as_millis(),
                ));
            }
            self.granted = false;
            self.grant_generation = None;
            self.sender.send_done();
            if let Some(wake) = self.wake {
                wake.signal(());
            }
        }
    }
}

impl<M: RawMutex + Sync, const FRAME: usize> ManifoldLaneWriter
    for EmbassyGrantProducer<'_, M, FRAME>
{
    fn try_write(&mut self, target: FrameTarget, frame: &[u8]) -> LaneWriteOutcome {
        if frame.len() > FRAME {
            return LaneWriteOutcome::FrameTooLarge {
                frame_len: frame.len(),
                capacity: FRAME,
            };
        }
        let Some(slot) = GrantProducer::try_grant(self) else {
            self.note_pressure();
            return LaneWriteOutcome::Full;
        };
        match target {
            FrameTarget::Direct(interface_id) => slot.fill_for(interface_id, frame),
            FrameTarget::Fan(fan) => slot.fill_for_fan(fan, frame),
        }
        GrantProducer::commit(self);
        LaneWriteOutcome::Written
    }
}

pub struct EmbassyGrantConsumer<'a, M: RawMutex, const FRAME: usize> {
    receiver: zerocopy_channel::Receiver<'a, M, FrameSlot<FRAME>>,
    peeked: bool,
    status: Option<(&'a EmbassyInterfaceStatus, InterfaceQueueWakeSlot)>,
}

impl<'a, M: RawMutex, const FRAME: usize> EmbassyGrantConsumer<'a, M, FRAME> {
    /// Bind before the endpoint is used. Each endpoint needs its own wake slot.
    pub(crate) fn set_status(
        &mut self,
        status: &'a EmbassyInterfaceStatus,
        wake_slot: InterfaceQueueWakeSlot,
    ) {
        self.status = Some((status, wake_slot));
    }
}

fn slot_is_current<const FRAME: usize>(
    status: Option<(&EmbassyInterfaceStatus, InterfaceQueueWakeSlot)>,
    slot: &FrameSlot<FRAME>,
) -> bool {
    match status {
        Some((status, _)) => {
            slot.lane_generation.is_some() && slot.lane_generation == status.available_generation()
        }
        None => true,
    }
}

impl<M: RawMutex, const FRAME: usize> GrantConsumer<FRAME> for EmbassyGrantConsumer<'_, M, FRAME> {
    fn try_peek(&mut self) -> Option<&mut FrameSlot<FRAME>> {
        loop {
            let slot = self.receiver.try_receive()?;
            if slot_is_current(self.status, slot) {
                self.peeked = true;
                return self.receiver.try_receive();
            }
            slot.clear();
            self.peeked = false;
            self.receiver.receive_done();
        }
    }

    async fn peek(&mut self) -> &mut FrameSlot<FRAME> {
        poll_fn(|cx| {
            if let Some((status, wake_slot)) = self.status {
                status.register_queue_waker(wake_slot, cx.waker());
            }
            loop {
                let Poll::Ready(slot) = self.receiver.poll_receive(cx) else {
                    return Poll::Pending;
                };
                if slot_is_current(self.status, slot) {
                    return Poll::Ready(());
                }
                slot.clear();
                self.peeked = false;
                self.receiver.receive_done();
            }
        })
        .await;
        let slot = self.receiver.receive().await;
        self.peeked = true;
        slot
    }

    fn release(&mut self) {
        if self.peeked {
            self.peeked = false;
            self.receiver.receive_done();
        }
    }
}

impl<M: RawMutex + Sync, const FRAME: usize> ManifoldLaneReader
    for EmbassyGrantConsumer<'_, M, FRAME>
{
    fn try_read(&mut self) -> Option<(FrameTarget, PacketPhyStats, &mut [u8])> {
        let slot = GrantConsumer::try_peek(self)?;
        let target = slot.target;
        let packet_phy = slot.packet_phy;
        Some((target, packet_phy, slot.frame_mut()))
    }

    fn release(&mut self) {
        GrantConsumer::release(self);
    }
}

/// A heap-backed grant lane for host-side tests of Embassy interfaces.
#[cfg(any(test, feature = "std"))]
pub fn leaked_grant_lane<const FRAME: usize>(
    depth: usize,
) -> (
    EmbassyGrantProducer<
        'static,
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        FRAME,
    >,
    EmbassyGrantConsumer<
        'static,
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        FRAME,
    >,
) {
    let slots: std::vec::Vec<FrameSlot<FRAME>> = (0..depth).map(|_| FrameSlot::empty()).collect();
    let channel = std::boxed::Box::leak(std::boxed::Box::new(zerocopy_channel::Channel::new(
        std::boxed::Box::leak(slots.into_boxed_slice()),
    )));
    embassy_grant_lane(channel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::Future;
    use core::pin::pin;
    use core::task::Context;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use std::sync::atomic::{AtomicUsize, Ordering as StdOrdering};
    use std::sync::Arc;
    use std::task::{Wake, Waker};

    use crate::interfaces::{ConnectionState, InterfaceId};

    type Producer = EmbassyGrantProducer<'static, CriticalSectionRawMutex, 64>;
    type Consumer = EmbassyGrantConsumer<'static, CriticalSectionRawMutex, 64>;

    fn status() -> &'static EmbassyInterfaceStatus {
        Box::leak(Box::new(EmbassyInterfaceStatus::new_unaccounted(
            InterfaceId::new(*b"radio000"),
            ConnectionState::Connected,
        )))
    }

    fn bound_lane(
        status: &'static EmbassyInterfaceStatus,
        depth: usize,
        ingress: bool,
    ) -> (Producer, Consumer) {
        let slots: std::vec::Vec<FrameSlot<64>> = (0..depth).map(|_| FrameSlot::empty()).collect();
        let channel = Box::leak(Box::new(zerocopy_channel::Channel::new(Box::leak(
            slots.into_boxed_slice(),
        ))));
        let direction = if ingress {
            InterfaceQueueDirection::Ingress
        } else {
            InterfaceQueueDirection::Egress
        };
        embassy_grant_lane_with_status(channel, status, direction)
    }

    fn write(producer: &mut Producer, value: u8) {
        let slot = producer.try_grant().expect("available queue slot");
        slot.fill_for(InterfaceId::new(*b"radio000"), &[value]);
        producer.commit();
    }

    fn retire(status: &EmbassyInterfaceStatus) {
        status.acknowledge_availability(status.is_enabled(), status.disable_generation());
    }

    #[test]
    fn both_lane_directions_discard_old_queues_and_preserve_fresh_post_on_work() {
        for ingress in [false, true] {
            let status = status();
            let (mut producer, mut consumer) = bound_lane(status, 3, ingress);
            write(&mut producer, 1);
            write(&mut producer, 2);
            status.disable();
            assert!(producer.try_grant().is_none());
            status.enable();
            assert!(producer.try_grant().is_none(), "retirement is still owed");
            retire(status);
            write(&mut producer, 3);
            let slot = consumer.try_peek().expect("only the fresh frame survives");
            assert_eq!(slot.frame(), &[3]);
            assert_eq!(slot.lane_generation, Some(1));
            GrantConsumer::release(&mut consumer);
            assert!(consumer.try_peek().is_none());
            write(&mut producer, 4);
            assert_eq!(consumer.try_peek().unwrap().frame(), &[4]);
        }
    }

    #[test]
    fn commit_never_relabels_an_in_flight_grant_after_off_on() {
        let status = status();
        let (mut producer, mut consumer) = bound_lane(status, 1, false);
        let slot = producer.try_grant().unwrap();
        slot.fill_for(InterfaceId::new(*b"radio000"), &[1]);
        // fill clears metadata, but cannot overwrite the producer's admission
        // generation. Re-acquiring the same uncommitted grant cannot renew it.
        status.disable();
        status.enable();
        retire(status);
        assert_eq!(producer.try_grant().unwrap().frame(), &[1]);
        producer.commit();
        assert!(consumer.try_peek().is_none());
        let slot = producer.try_grant().unwrap();
        assert_eq!(slot.frame_len(), 0);
        assert_eq!(slot.lane_generation, None);
        assert_eq!(slot.enqueued_at, None);
        slot.fill_for(InterfaceId::new(*b"radio000"), &[2]);
        producer.commit();
        let slot = consumer.try_peek().unwrap();
        assert_eq!(slot.frame(), &[2]);
        assert_eq!(slot.lane_generation, Some(1));
        assert!(slot.enqueued_at.is_some());
    }

    #[test]
    fn consumer_drains_disabled_frames_and_reclaims_full_queue_capacity() {
        let status = status();
        let (mut producer, mut consumer) = bound_lane(status, 2, true);
        write(&mut producer, 1);
        write(&mut producer, 2);
        assert_eq!(consumer.try_peek().unwrap().frame(), &[1]);
        status.disable();
        assert!(consumer.try_peek().is_none());
        GrantConsumer::release(&mut consumer); // no stale peek may release a new frame
        status.enable();
        retire(status);
        write(&mut producer, 3);
        write(&mut producer, 4);
        assert_eq!(consumer.try_peek().unwrap().frame(), &[3]);
        GrantConsumer::release(&mut consumer);
        assert_eq!(consumer.try_peek().unwrap().frame(), &[4]);
    }

    #[test]
    fn async_grant_waiting_for_capacity_keeps_its_original_generation_across_off_on() {
        for ingress in [false, true] {
            let status = status();
            let (mut producer, mut consumer) = bound_lane(status, 1, ingress);
            write(&mut producer, 1);
            let mut cx = Context::from_waker(Waker::noop());
            {
                let mut grant = pin!(producer.grant());
                assert!(grant.as_mut().poll(&mut cx).is_pending());
                status.disable();
                status.enable();
                assert!(consumer.try_peek().is_none());
                assert!(grant.as_mut().poll(&mut cx).is_pending());
                retire(status);
                let Poll::Ready(slot) = grant.as_mut().poll(&mut cx) else {
                    panic!("retirement and slot reclamation must unblock the old attempt")
                };
                slot.fill_for(InterfaceId::new(*b"radio000"), &[2]);
            }
            producer.commit();
            assert!(
                consumer.try_peek().is_none(),
                "a capacity wait must not renew a pre-disable frame's generation"
            );
            {
                let mut grant = pin!(producer.grant());
                let Poll::Ready(slot) = grant.as_mut().poll(&mut cx) else {
                    panic!("a fresh attempt may use the resumed generation")
                };
                assert_eq!(slot.frame_len(), 0);
                assert_eq!(slot.lane_generation, None);
                assert_eq!(slot.enqueued_at, None);
                slot.fill_for(InterfaceId::new(*b"radio000"), &[3]);
            }
            producer.commit();
            let slot = consumer.try_peek().expect("fresh attempt must survive");
            assert_eq!(slot.frame(), &[3]);
            assert_eq!(slot.lane_generation, Some(1));
        }
    }

    #[test]
    fn async_consumer_discards_old_generation_then_waits_for_fresh_frame() {
        let status = status();
        let (mut producer, mut consumer) = bound_lane(status, 1, true);
        write(&mut producer, 1);
        status.disable();
        status.enable();
        retire(status);
        let mut cx = Context::from_waker(Waker::noop());
        let mut peek = pin!(consumer.peek());
        assert!(peek.as_mut().poll(&mut cx).is_pending());
        write(&mut producer, 2);
        let Poll::Ready(slot) = peek.as_mut().poll(&mut cx) else {
            panic!("fresh frame must wake the waiting consumer")
        };
        assert_eq!(slot.frame(), &[2]);
        assert_eq!(slot.lane_generation, Some(1));
    }

    #[derive(Default)]
    struct WakeCount(AtomicUsize);

    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, StdOrdering::Relaxed);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, StdOrdering::Relaxed);
        }
    }

    #[test]
    fn four_async_endpoints_receive_independent_availability_wakes() {
        let status = status();
        let (mut inbound_producer, mut inbound_consumer) = bound_lane(status, 1, true);
        let (mut outbound_producer, mut outbound_consumer) = bound_lane(status, 1, false);
        status.disable();
        let counts = [(); 4].map(|_| Arc::new(WakeCount::default()));
        let wakers = counts
            .each_ref()
            .map(|count| Waker::from(Arc::clone(count)));
        let mut inbound_grant = pin!(inbound_producer.grant());
        let mut inbound_peek = pin!(inbound_consumer.peek());
        let mut outbound_grant = pin!(outbound_producer.grant());
        let mut outbound_peek = pin!(outbound_consumer.peek());
        assert!(inbound_grant
            .as_mut()
            .poll(&mut Context::from_waker(&wakers[0]))
            .is_pending());
        assert!(inbound_peek
            .as_mut()
            .poll(&mut Context::from_waker(&wakers[1]))
            .is_pending());
        assert!(outbound_grant
            .as_mut()
            .poll(&mut Context::from_waker(&wakers[2]))
            .is_pending());
        assert!(outbound_peek
            .as_mut()
            .poll(&mut Context::from_waker(&wakers[3]))
            .is_pending());
        status.enable();
        assert!(counts
            .iter()
            .all(|count| count.0.load(StdOrdering::Relaxed) == 1));
        retire(status);
        assert!(counts
            .iter()
            .all(|count| count.0.load(StdOrdering::Relaxed) == 2));
        assert!(inbound_grant
            .as_mut()
            .poll(&mut Context::from_waker(&wakers[0]))
            .is_ready());
        assert!(outbound_grant
            .as_mut()
            .poll(&mut Context::from_waker(&wakers[2]))
            .is_ready());
    }
}
