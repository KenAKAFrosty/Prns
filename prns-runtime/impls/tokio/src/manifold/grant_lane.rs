use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use prns_core::interfaces::{FrameSink, FrameSinkError, PacketPhyStats};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use tokio::sync::Notify;

pub fn tokio_grant_lane(slot_cap: usize, depth: usize) -> (TokioGrantProducer, TokioGrantConsumer) {
    let depth = depth.max(1);
    let (expedited, expedited_slots) = RingBuffer::new(depth);
    let (regular, regular_slots) = RingBuffer::new(depth);
    let (recycled_slots, recycled) = RingBuffer::new(depth);
    let filled_ready = Arc::new(Notify::new());
    let free_ready = Arc::new(Notify::new());
    let producer_parked = Arc::new(AtomicBool::new(false));
    let consumer_parked = Arc::new(AtomicBool::new(false));
    let announced = Arc::new(AtomicBool::new(false));
    (
        TokioGrantProducer {
            slot_cap,
            vacant_slots: depth,
            recycled,
            expedited,
            regular,
            capacity: depth,
            granted: None,
            filled_ready: filled_ready.clone(),
            free_ready: free_ready.clone(),
            producer_parked: producer_parked.clone(),
            consumer_parked: consumer_parked.clone(),
            announced: announced.clone(),
        },
        TokioGrantConsumer {
            expedited: expedited_slots,
            regular: regular_slots,
            recycled: recycled_slots,
            peeked: None,
            filled_ready,
            free_ready,
            producer_parked,
            consumer_parked,
            announced,
            expedited_streak: 0,
        },
    )
}

pub struct HeapFrameSlot {
    pub len: usize,
    pub cap: usize,
    pub bytes: Vec<u8>,
    pub packet_phy: PacketPhyStats,
}

impl HeapFrameSlot {
    fn empty(cap: usize) -> Self {
        Self {
            len: 0,
            cap,
            bytes: Vec::new(),
            packet_phy: PacketPhyStats::default(),
        }
    }

    pub fn fill(&mut self, frame: &[u8]) {
        self.packet_phy = PacketPhyStats::default();
        if self.bytes.len() < frame.len() {
            self.bytes.clear();
            self.bytes.extend_from_slice(frame);
        } else {
            self.bytes[..frame.len()].copy_from_slice(frame);
        }
        self.len = frame.len();
    }

    pub fn frame(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn frame_mut(&mut self) -> &mut [u8] {
        let len = self.len;
        &mut self.bytes[..len]
    }
}

impl FrameSink for HeapFrameSlot {
    fn clear(&mut self) {
        self.bytes.clear();
        self.len = 0;
        self.packet_phy = PacketPhyStats::default();
    }

    fn frame_len(&self) -> usize {
        self.bytes.len()
    }

    fn free_capacity(&self) -> usize {
        self.cap.saturating_sub(self.bytes.len())
    }

    fn push(&mut self, byte: u8) -> Result<(), FrameSinkError> {
        if self.bytes.len() >= self.cap {
            return Err(FrameSinkError::Full);
        }
        self.bytes.push(byte);
        Ok(())
    }

    fn extend_from_slice(&mut self, run: &[u8]) -> Result<(), FrameSinkError> {
        if run.len() > self.cap.saturating_sub(self.bytes.len()) {
            return Err(FrameSinkError::Full);
        }
        self.bytes.extend_from_slice(run);
        Ok(())
    }
}

pub struct TokioGrantProducer {
    slot_cap: usize,
    vacant_slots: usize,
    recycled: Consumer<HeapFrameSlot>,
    expedited: Producer<HeapFrameSlot>,
    regular: Producer<HeapFrameSlot>,
    capacity: usize,
    pub(super) granted: Option<HeapFrameSlot>,
    filled_ready: Arc<Notify>,
    free_ready: Arc<Notify>,
    producer_parked: Arc<AtomicBool>,
    consumer_parked: Arc<AtomicBool>,
    announced: Arc<AtomicBool>,
}

impl TokioGrantProducer {
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn occupancy(&self) -> usize {
        self.capacity()
            .saturating_sub(self.vacant_slots.saturating_add(self.recycled.slots()))
    }

    pub fn try_grant(&mut self) -> Option<&mut HeapFrameSlot> {
        if self.granted.is_none() {
            self.granted = self.pop_free_slot();
        }
        self.granted.as_mut()
    }

    fn pop_free_slot(&mut self) -> Option<HeapFrameSlot> {
        self.recycled.pop().ok().or_else(|| {
            self.vacant_slots = self.vacant_slots.checked_sub(1)?;
            Some(HeapFrameSlot::empty(self.slot_cap))
        })
    }

    pub async fn grant(&mut self) -> &mut HeapFrameSlot {
        loop {
            if let Some(slot) = self.granted.take() {
                return self.granted.insert(slot);
            }
            match self.pop_free_slot() {
                Some(slot) => self.granted = Some(slot),
                None => {
                    // The ring is authoritative. Advertise the cold waiter, then recheck it so a
                    // release racing this arm either wakes us or is observed synchronously.
                    self.producer_parked.store(true, Ordering::Release);
                    match self.pop_free_slot() {
                        Some(slot) => {
                            self.producer_parked.store(false, Ordering::Release);
                            self.granted = Some(slot);
                        }
                        None => self.free_ready.notified().await,
                    }
                }
            }
        }
    }

    pub fn commit(&mut self) {
        self.commit_to(GrantQueue::Regular);
    }

    pub(super) fn commit_expedited(&mut self) {
        self.commit_to(GrantQueue::Expedited);
    }

    fn commit_to(&mut self, queue: GrantQueue) {
        if let Some(slot) = self.granted.take() {
            let committed = match queue {
                GrantQueue::Expedited => self.expedited.push(slot).is_ok(),
                GrantQueue::Regular => self.regular.push(slot).is_ok(),
            };
            if committed
                && self.consumer_parked.load(Ordering::Acquire)
                && self.consumer_parked.swap(false, Ordering::AcqRel)
            {
                self.filled_ready.notify_one();
            }
        }
    }

    pub fn needs_announce(&self) -> bool {
        !self.announced.swap(true, Ordering::AcqRel)
    }
}

pub struct TokioGrantConsumer {
    expedited: Consumer<HeapFrameSlot>,
    regular: Consumer<HeapFrameSlot>,
    recycled: Producer<HeapFrameSlot>,
    peeked: Option<HeapFrameSlot>,
    filled_ready: Arc<Notify>,
    free_ready: Arc<Notify>,
    producer_parked: Arc<AtomicBool>,
    consumer_parked: Arc<AtomicBool>,
    announced: Arc<AtomicBool>,
    expedited_streak: usize,
}

const EXPEDITED_BURST: usize = 8;

enum GrantQueue {
    Expedited,
    Regular,
}

impl TokioGrantConsumer {
    pub fn try_peek(&mut self) -> Option<&mut HeapFrameSlot> {
        if self.peeked.is_none() {
            self.peeked = self.pop_next();
        }
        self.peeked.as_mut()
    }

    pub async fn peek(&mut self) -> &mut HeapFrameSlot {
        loop {
            if let Some(slot) = self.peeked.take() {
                return self.peeked.insert(slot);
            }
            match self.pop_next() {
                Some(slot) => self.peeked = Some(slot),
                None => {
                    // See `grant`: the post-arm recheck closes the empty-to-filled race without
                    // paying Tokio's wake machinery while this consumer is actively draining.
                    self.consumer_parked.store(true, Ordering::Release);
                    match self.pop_next() {
                        Some(slot) => {
                            self.consumer_parked.store(false, Ordering::Release);
                            self.peeked = Some(slot);
                        }
                        None => self.filled_ready.notified().await,
                    }
                }
            }
        }
    }

    fn pop_next(&mut self) -> Option<HeapFrameSlot> {
        if self.expedited_streak < EXPEDITED_BURST {
            if let Ok(slot) = self.expedited.pop() {
                self.expedited_streak += 1;
                return Some(slot);
            }
        }
        if let Ok(slot) = self.regular.pop() {
            self.expedited_streak = 0;
            return Some(slot);
        }
        let slot = self.expedited.pop().ok()?;
        self.expedited_streak = EXPEDITED_BURST;
        Some(slot)
    }

    pub fn release(&mut self) {
        if let Some(slot) = self.peeked.take() {
            self.return_slot(slot);
        }
    }

    pub(crate) fn take_peeked(&mut self) -> Option<HeapFrameSlot> {
        self.peeked.take()
    }

    pub(crate) fn return_slot(&mut self, slot: HeapFrameSlot) {
        match self.recycled.push(slot) {
            Ok(()) => {
                if self.producer_parked.load(Ordering::Acquire)
                    && self.producer_parked.swap(false, Ordering::AcqRel)
                {
                    self.free_ready.notify_one();
                }
            }
            Err(PushError::Full(_)) => {}
        }
    }

    pub fn acknowledge(&mut self) {
        self.announced.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn depth_is_exact_and_frames_remain_fifo() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 3);

        for frame in [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()] {
            producer.try_grant().expect("slot available").fill(frame);
            producer.commit();
        }
        assert!(producer.try_grant().is_none());

        for frame in [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()] {
            assert_eq!(consumer.try_peek().expect("frame available").frame(), frame);
            consumer.release();
        }
        assert!(consumer.try_peek().is_none());
    }

    #[test]
    fn a_taken_slot_returns_with_its_allocation() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 1);
        producer.try_grant().unwrap().fill(&[7; 48]);
        let allocation = producer.granted.as_ref().unwrap().bytes.as_ptr();
        producer.commit();

        assert!(consumer.try_peek().is_some());
        let slot = consumer.take_peeked().unwrap();
        assert_eq!(slot.bytes.as_ptr(), allocation);
        assert!(producer.try_grant().is_none());

        consumer.return_slot(slot);
        assert_eq!(producer.try_grant().unwrap().bytes.as_ptr(), allocation);
    }

    #[test]
    fn a_returned_slot_is_reused_before_untouched_burst_capacity() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 3);
        producer.try_grant().unwrap().fill(&[7; 48]);
        let allocation = producer.granted.as_ref().unwrap().bytes.as_ptr();
        producer.commit();
        consumer.try_peek().unwrap();
        consumer.release();

        assert_eq!(producer.try_grant().unwrap().bytes.as_ptr(), allocation);
    }

    #[test]
    fn sequential_frames_grow_only_the_recycled_high_water_slot() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 3);
        let mut allocation = None;

        for _ in 0..32 {
            let slot = producer.try_grant().unwrap();
            slot.fill(&[7; 48]);
            match allocation {
                Some(allocation) => assert_eq!(slot.bytes.as_ptr(), allocation),
                None => allocation = Some(slot.bytes.as_ptr()),
            }
            producer.commit();
            consumer.try_peek().unwrap();
            consumer.release();
        }

        assert_eq!(producer.vacant_slots, 2);
        assert_eq!(producer.recycled.slots(), 1);
        assert_eq!(producer.occupancy(), 0);
    }

    #[test]
    fn expedited_frames_overtake_regular_frames_with_a_bounded_streak() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 10);
        producer.try_grant().unwrap().fill(b"regular");
        producer.commit();
        for index in 0..9u8 {
            producer.try_grant().unwrap().fill(&[index]);
            producer.commit_expedited();
        }

        for index in 0..8u8 {
            assert_eq!(consumer.try_peek().unwrap().frame(), &[index]);
            consumer.release();
        }
        assert_eq!(consumer.try_peek().unwrap().frame(), b"regular");
        consumer.release();
        assert_eq!(consumer.try_peek().unwrap().frame(), &[8]);
        consumer.release();
        assert!(consumer.try_peek().is_none());
    }

    #[test]
    fn slot_storage_survives_a_complete_recycle() {
        let (mut producer, mut consumer) = tokio_grant_lane(512, 1);

        let slot = producer.try_grant().expect("slot available");
        slot.fill(&[0xA5; 384]);
        let allocation = slot.bytes.as_ptr();
        let capacity = slot.bytes.capacity();
        producer.commit();
        assert_eq!(
            consumer.try_peek().expect("frame available").bytes.as_ptr(),
            allocation
        );
        consumer.release();

        let recycled = producer.try_grant().expect("slot recycled");
        recycled.fill(b"small");
        assert_eq!(recycled.bytes.as_ptr(), allocation);
        assert_eq!(recycled.bytes.capacity(), capacity);
        assert_eq!(recycled.bytes.len(), 384);
        assert_eq!(recycled.frame(), b"small");
    }

    #[tokio::test]
    async fn commit_wakes_a_parked_consumer() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 1);

        let receive = async { consumer.peek().await.frame().to_vec() };
        let send = async {
            tokio::task::yield_now().await;
            producer.try_grant().expect("slot available").fill(b"ready");
            producer.commit();
        };
        let (frame, ()) = tokio::join!(receive, send);

        assert_eq!(frame, b"ready");
    }

    #[tokio::test]
    async fn release_wakes_a_parked_producer() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 1);
        producer.try_grant().expect("slot available").fill(b"full");
        producer.commit();

        let grant = async {
            producer.grant().await.fill(b"next");
        };
        let release = async {
            tokio::task::yield_now().await;
            assert_eq!(consumer.peek().await.frame(), b"full");
            consumer.release();
        };
        tokio::join!(grant, release);

        producer.commit();
        assert_eq!(consumer.peek().await.frame(), b"next");
    }

    #[tokio::test]
    async fn active_peers_do_not_publish_tokio_wake_permits() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 1);
        producer.try_grant().expect("slot available").fill(b"hot");
        producer.commit();

        assert!(
            tokio::time::timeout(Duration::from_millis(20), consumer.filled_ready.notified())
                .await
                .is_err(),
            "a synchronously visible frame does not need a stored wake permit",
        );
        assert_eq!(
            consumer.try_peek().expect("frame available").frame(),
            b"hot"
        );
        consumer.release();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), producer.free_ready.notified())
                .await
                .is_err(),
            "a synchronously visible free slot does not need a stored wake permit",
        );
    }

    #[tokio::test]
    async fn cancelled_parks_do_not_consume_or_strand_wakes() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 1);

        let consumer_resolved = tokio::select! {
            biased;
            _ = consumer.peek() => true,
            _ = tokio::task::yield_now() => false,
        };
        assert!(!consumer_resolved);
        producer
            .try_grant()
            .expect("slot available")
            .fill(b"after cancel");
        producer.commit();
        let frame = tokio::time::timeout(Duration::from_secs(1), consumer.peek())
            .await
            .expect("consumer wakes");
        assert_eq!(frame.frame(), b"after cancel");
        consumer.release();

        producer
            .try_grant()
            .expect("slot available")
            .fill(b"full again");
        producer.commit();
        let producer_resolved = tokio::select! {
            biased;
            _ = producer.grant() => true,
            _ = tokio::task::yield_now() => false,
        };
        assert!(!producer_resolved);
        consumer.peek().await;
        consumer.release();
        let slot = tokio::time::timeout(Duration::from_secs(1), producer.grant())
            .await
            .expect("producer wakes");
        slot.fill(b"after second cancel");
    }

    #[tokio::test]
    async fn exhausted_lane_parks_after_its_peer_is_dropped() {
        let (producer, mut consumer) = tokio_grant_lane(64, 1);
        drop(producer);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), consumer.peek())
                .await
                .is_err()
        );

        let (mut producer, consumer) = tokio_grant_lane(64, 1);
        producer.try_grant().expect("slot available").fill(b"held");
        producer.commit();
        drop(consumer);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), producer.grant())
                .await
                .is_err()
        );
    }

    #[test]
    fn commit_and_release_without_a_loan_are_noops() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 1);

        producer.commit();
        consumer.release();
        producer
            .try_grant()
            .expect("slot remains available")
            .fill(b"frame");
        producer.commit();
        assert_eq!(
            consumer.try_peek().expect("frame available").frame(),
            b"frame"
        );
    }

    #[tokio::test]
    async fn a_filled_grant_is_read_in_place_without_a_copy() {
        let (mut producer, mut consumer) = tokio_grant_lane(512, 2);

        let granted = producer.grant().await;
        granted.fill(b"the frame is written once");
        let written_at = granted.bytes.as_ptr() as usize;
        producer.commit();

        let received = consumer.peek().await;
        assert_eq!(received.frame(), b"the frame is written once");
        assert_eq!(
            received.bytes.as_ptr() as usize,
            written_at,
            "the consumer reads the very slot the producer filled",
        );
        received.frame_mut()[0] ^= 0x20;
        assert_eq!(&received.frame()[..3], b"The");
        consumer.release();
    }

    #[test]
    fn a_burst_earns_one_announcement_until_the_consumer_acknowledges() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 8);

        producer.try_grant().expect("lane grants").fill(b"one");
        producer.commit();
        assert!(producer.needs_announce(), "the first commit announces");

        producer.try_grant().expect("lane grants").fill(b"two");
        producer.commit();
        assert!(
            !producer.needs_announce(),
            "a burst behind an unconsumed announcement stays silent",
        );

        consumer.acknowledge();
        while consumer.try_peek().is_some() {
            consumer.release();
        }

        producer.try_grant().expect("lane grants").fill(b"three");
        producer.commit();
        assert!(
            producer.needs_announce(),
            "a commit after the acknowledge announces again",
        );
    }

    #[tokio::test]
    async fn a_full_lane_refuses_grants_until_the_consumer_releases() {
        let (mut producer, mut consumer) = tokio_grant_lane(64, 1);

        producer
            .try_grant()
            .expect("an empty lane grants")
            .fill(b"one");
        producer.commit();
        assert!(producer.try_grant().is_none(), "a depth-one lane is full");

        consumer.try_peek().expect("the committed frame is there");
        consumer.release();
        assert!(
            producer.try_grant().is_some(),
            "the release frees the slot for the next grant",
        );
    }
}
