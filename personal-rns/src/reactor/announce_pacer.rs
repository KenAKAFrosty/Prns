use crate::engine::InstantMillis;
use crate::interfaces::AnnounceBandwidthCap;
use crate::wire::MTU;
use heapless::{Deque, Vec as HeaplessVec};

pub trait PacerQueue: Default {
    fn push_back(&mut self, bytes: &[u8]);
    fn pop_front_with<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<R>;
    fn is_empty(&self) -> bool;
}

pub struct FixedPacerQueue<const DEPTH: usize> {
    frames: Deque<HeaplessVec<u8, MTU>, DEPTH>,
}

impl<const DEPTH: usize> Default for FixedPacerQueue<DEPTH> {
    fn default() -> Self {
        Self {
            frames: Deque::new(),
        }
    }
}

impl<const DEPTH: usize> PacerQueue for FixedPacerQueue<DEPTH> {
    fn push_back(&mut self, bytes: &[u8]) {
        let mut slot = HeaplessVec::new();
        if slot.extend_from_slice(bytes).is_err() {
            return;
        }
        if self.frames.is_full() {
            let _ = self.frames.pop_front();
        }
        let _ = self.frames.push_back(slot);
    }

    fn pop_front_with<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        self.frames.pop_front().map(|slot| f(slot.as_slice()))
    }

    fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

#[cfg(feature = "alloc")]
pub use heap::HeapPacerQueue;

#[cfg(feature = "alloc")]
mod heap {
    use super::PacerQueue;
    use alloc::collections::VecDeque;
    use alloc::vec::Vec;

    #[derive(Default)]
    pub struct HeapPacerQueue {
        frames: VecDeque<Vec<u8>>,
    }

    impl PacerQueue for HeapPacerQueue {
        fn push_back(&mut self, bytes: &[u8]) {
            self.frames.push_back(bytes.to_vec());
        }

        fn pop_front_with<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
            self.frames.pop_front().map(|frame| f(&frame))
        }

        fn is_empty(&self) -> bool {
            self.frames.is_empty()
        }
    }
}

pub struct AnnouncePacer<Q: PacerQueue> {
    cap: AnnounceBandwidthCap,
    allowed_at: InstantMillis,
    queue: Q,
}

impl<Q: PacerQueue> AnnouncePacer<Q> {
    pub fn new(cap: AnnounceBandwidthCap) -> Self {
        Self {
            cap,
            allowed_at: InstantMillis(0),
            queue: Q::default(),
        }
    }

    pub fn offer(&mut self, bytes: &[u8], now: InstantMillis, send: impl FnOnce(&[u8])) {
        if self.queue.is_empty() && self.allowed_at.0 <= now.0 {
            send(bytes);
            self.allowed_at = InstantMillis(now.0.saturating_add(self.cap.spacing_ms(bytes.len())));
        } else {
            self.queue.push_back(bytes);
        }
    }

    pub fn release_due(&mut self, now: InstantMillis, send: impl FnOnce(&[u8])) -> bool {
        if self.allowed_at.0 > now.0 {
            return false;
        }
        let cap = self.cap;
        match self.queue.pop_front_with(|bytes| {
            send(bytes);
            cap.spacing_ms(bytes.len())
        }) {
            Some(spacing) => {
                self.allowed_at = InstantMillis(now.0.saturating_add(spacing));
                true
            }
            None => false,
        }
    }

    pub fn next_release(&self) -> Option<InstantMillis> {
        (!self.queue.is_empty()).then_some(self.allowed_at)
    }

    pub fn is_idle(&self) -> bool {
        self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOW: AnnounceBandwidthCap = AnnounceBandwidthCap::Limited {
        bitrate_bps: 5_000,
        cap_per_mille: 20,
    };
    const ANNOUNCE: &[u8] = &[0xAB; 167];
    const SPACING_MS: u64 = 13_360;

    fn capture() -> std::vec::Vec<std::vec::Vec<u8>> {
        std::vec::Vec::new()
    }

    #[test]
    fn an_unlimited_link_emits_immediately_and_never_queues() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4>>::new(AnnounceBandwidthCap::Unlimited);
        let mut sent = capture();
        for at in [0, 1, 2, 3] {
            pacer.offer(ANNOUNCE, InstantMillis(at), |b| sent.push(b.to_vec()));
        }
        assert_eq!(sent.len(), 4);
        assert!(pacer.is_idle());
        assert_eq!(pacer.next_release(), None);
    }

    #[test]
    fn an_idle_pacer_emits_the_first_announce_now() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4>>::new(SLOW);
        let mut sent = capture();
        pacer.offer(ANNOUNCE, InstantMillis(1_000), |b| sent.push(b.to_vec()));
        assert_eq!(sent.len(), 1);
        assert_eq!(pacer.next_release(), None);
    }

    #[test]
    fn a_second_announce_within_the_window_queues() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4>>::new(SLOW);
        let mut sent = capture();
        pacer.offer(ANNOUNCE, InstantMillis(1_000), |b| sent.push(b.to_vec()));
        pacer.offer(ANNOUNCE, InstantMillis(1_500), |b| sent.push(b.to_vec()));
        assert_eq!(sent.len(), 1, "the second is held, not emitted");
        assert_eq!(
            pacer.next_release(),
            Some(InstantMillis(1_000 + SPACING_MS))
        );
    }

    #[test]
    fn release_emits_the_queued_announce_once_its_window_passes() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4>>::new(SLOW);
        let mut sent = capture();
        pacer.offer(ANNOUNCE, InstantMillis(1_000), |b| sent.push(b.to_vec()));
        pacer.offer(ANNOUNCE, InstantMillis(1_500), |b| sent.push(b.to_vec()));

        let before = pacer.release_due(InstantMillis(1_000 + SPACING_MS - 1), |b| {
            sent.push(b.to_vec())
        });
        assert!(!before, "nothing releases before the window");
        assert_eq!(sent.len(), 1);

        let at = pacer.release_due(InstantMillis(1_000 + SPACING_MS), |b| sent.push(b.to_vec()));
        assert!(at, "the queued announce releases at the window");
        assert_eq!(sent.len(), 2);
        assert!(pacer.is_idle());
    }

    #[test]
    fn a_burst_drains_one_per_spacing_interval() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<8>>::new(SLOW);
        let mut sent = capture();
        for _ in 0..4 {
            pacer.offer(ANNOUNCE, InstantMillis(0), |b| sent.push(b.to_vec()));
        }
        assert_eq!(sent.len(), 1, "first goes now, the rest queue");

        let mut now = 0;
        for expected in 2..=4 {
            now += SPACING_MS;
            let released = pacer.release_due(InstantMillis(now), |b| sent.push(b.to_vec()));
            assert!(released);
            assert_eq!(sent.len(), expected);
            let extra = pacer.release_due(InstantMillis(now), |b| sent.push(b.to_vec()));
            assert!(!extra, "only one releases per interval");
        }
        assert!(pacer.is_idle());
    }

    #[test]
    fn a_full_fixed_queue_drops_the_oldest() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<2>>::new(SLOW);
        let mut sent = capture();
        pacer.offer(&[1; 10], InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&[2; 10], InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&[3; 10], InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&[4; 10], InstantMillis(0), |b| sent.push(b.to_vec()));

        let mut now = 0;
        while !pacer.is_idle() {
            now += SPACING_MS;
            pacer.release_due(InstantMillis(now), |b| sent.push(b.to_vec()));
        }
        assert_eq!(sent[0], std::vec![1; 10]);
        assert_eq!(sent[1], std::vec![3; 10]);
        assert_eq!(sent[2], std::vec![4; 10]);
    }

    #[test]
    fn a_heap_queue_grows_without_dropping() {
        let mut pacer = AnnouncePacer::<HeapPacerQueue>::new(SLOW);
        let mut sent = capture();
        for n in 0..64u8 {
            pacer.offer(&[n; 10], InstantMillis(0), |b| sent.push(b.to_vec()));
        }
        assert_eq!(sent.len(), 1, "first goes now, 63 queue and none drop");

        let mut now = 0;
        let mut released = 0;
        while !pacer.is_idle() {
            now += SPACING_MS;
            if pacer.release_due(InstantMillis(now), |b| sent.push(b.to_vec())) {
                released += 1;
            }
        }
        assert_eq!(released, 63);
        assert_eq!(sent.len(), 64);
        assert_eq!(sent[63], std::vec![63; 10]);
    }
}
