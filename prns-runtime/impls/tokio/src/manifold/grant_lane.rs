use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use prns_core::interfaces::{FrameSink, FrameSinkError, PacketPhyStats};
use rtrb::{Consumer, PopError, Producer, PushError, RingBuffer};
use tokio::sync::Notify;

pub fn tokio_grant_lane(slot_cap: usize, depth: usize) -> (TokioGrantProducer, TokioGrantConsumer) {
    let depth = depth.max(1);
    let (filled, filled_slots) = RingBuffer::new(depth);
    let (mut free_slots, free) = RingBuffer::new(depth);
    for _ in 0..depth {
        let _ = free_slots.push(HeapFrameSlot::empty(slot_cap));
    }
    let filled_ready = Arc::new(Notify::new());
    let free_ready = Arc::new(Notify::new());
    let announced = Arc::new(AtomicBool::new(false));
    (
        TokioGrantProducer {
            free,
            filled,
            capacity: depth,
            granted: None,
            filled_ready: filled_ready.clone(),
            free_ready: free_ready.clone(),
            announced: announced.clone(),
        },
        TokioGrantConsumer {
            filled: filled_slots,
            free: free_slots,
            peeked: None,
            filled_ready,
            free_ready,
            announced,
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
    free: Consumer<HeapFrameSlot>,
    filled: Producer<HeapFrameSlot>,
    capacity: usize,
    pub(super) granted: Option<HeapFrameSlot>,
    filled_ready: Arc<Notify>,
    free_ready: Arc<Notify>,
    announced: Arc<AtomicBool>,
}

impl TokioGrantProducer {
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn occupancy(&self) -> usize {
        self.capacity().saturating_sub(self.free.slots())
    }

    pub fn try_grant(&mut self) -> Option<&mut HeapFrameSlot> {
        if self.granted.is_none() {
            self.granted = self.free.pop().ok();
        }
        self.granted.as_mut()
    }

    pub async fn grant(&mut self) -> &mut HeapFrameSlot {
        loop {
            if let Some(slot) = self.granted.take() {
                return self.granted.insert(slot);
            }
            match self.free.pop() {
                Ok(slot) => self.granted = Some(slot),
                Err(PopError::Empty) => self.free_ready.notified().await,
            }
        }
    }

    pub fn commit(&mut self) {
        if let Some(slot) = self.granted.take() {
            match self.filled.push(slot) {
                Ok(()) => self.filled_ready.notify_one(),
                Err(PushError::Full(_)) => {}
            }
        }
    }

    pub fn needs_announce(&self) -> bool {
        !self.announced.swap(true, Ordering::AcqRel)
    }
}

pub struct TokioGrantConsumer {
    filled: Consumer<HeapFrameSlot>,
    free: Producer<HeapFrameSlot>,
    peeked: Option<HeapFrameSlot>,
    filled_ready: Arc<Notify>,
    free_ready: Arc<Notify>,
    announced: Arc<AtomicBool>,
}

impl TokioGrantConsumer {
    pub fn try_peek(&mut self) -> Option<&mut HeapFrameSlot> {
        if self.peeked.is_none() {
            self.peeked = self.filled.pop().ok();
        }
        self.peeked.as_mut()
    }

    pub async fn peek(&mut self) -> &mut HeapFrameSlot {
        loop {
            if let Some(slot) = self.peeked.take() {
                return self.peeked.insert(slot);
            }
            match self.filled.pop() {
                Ok(slot) => self.peeked = Some(slot),
                Err(PopError::Empty) => self.filled_ready.notified().await,
            }
        }
    }

    pub fn release(&mut self) {
        if let Some(slot) = self.peeked.take() {
            match self.free.push(slot) {
                Ok(()) => self.free_ready.notify_one(),
                Err(PushError::Full(_)) => {}
            }
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
