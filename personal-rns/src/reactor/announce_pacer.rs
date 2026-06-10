use crate::engine::InstantMillis;
use crate::interfaces::AnnounceBandwidthCap;
use crate::wire::MTU;
use heapless::{Deque, Vec as HeaplessVec};

pub struct AnnouncePacer<const DEPTH: usize> {
    cap: AnnounceBandwidthCap,
    allowed_at: InstantMillis,
    queue: Deque<HeaplessVec<u8, MTU>, DEPTH>,
}

impl<const DEPTH: usize> AnnouncePacer<DEPTH> {
    pub fn new(cap: AnnounceBandwidthCap) -> Self {
        Self {
            cap,
            allowed_at: InstantMillis(0),
            queue: Deque::new(),
        }
    }

    pub fn offer(&mut self, bytes: &[u8], now: InstantMillis, send: impl FnOnce(&[u8])) {
        if self.queue.is_empty() && self.allowed_at.0 <= now.0 {
            self.emit(bytes, now, send);
        } else {
            self.enqueue(bytes);
        }
    }

    pub fn release_due(&mut self, now: InstantMillis, send: impl FnOnce(&[u8])) -> bool {
        if self.allowed_at.0 <= now.0 {
            if let Some(front) = self.queue.pop_front() {
                self.emit(&front, now, send);
                return true;
            }
        }
        false
    }

    pub fn next_release(&self) -> Option<InstantMillis> {
        (!self.queue.is_empty()).then_some(self.allowed_at)
    }

    pub fn is_idle(&self) -> bool {
        self.queue.is_empty()
    }

    fn emit(&mut self, bytes: &[u8], now: InstantMillis, send: impl FnOnce(&[u8])) {
        send(bytes);
        self.allowed_at = InstantMillis(now.0.saturating_add(self.cap.spacing_ms(bytes.len())));
    }

    fn enqueue(&mut self, bytes: &[u8]) {
        let mut slot = HeaplessVec::new();
        if slot.extend_from_slice(bytes).is_err() {
            return;
        }
        if self.queue.is_full() {
            let _ = self.queue.pop_front();
        }
        let _ = self.queue.push_back(slot);
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
        let mut pacer = AnnouncePacer::<4>::new(AnnounceBandwidthCap::Unlimited);
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
        let mut pacer = AnnouncePacer::<4>::new(SLOW);
        let mut sent = capture();
        pacer.offer(ANNOUNCE, InstantMillis(1_000), |b| sent.push(b.to_vec()));
        assert_eq!(sent.len(), 1);
        assert_eq!(pacer.next_release(), None);
    }

    #[test]
    fn a_second_announce_within_the_window_queues() {
        let mut pacer = AnnouncePacer::<4>::new(SLOW);
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
        let mut pacer = AnnouncePacer::<4>::new(SLOW);
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
        let mut pacer = AnnouncePacer::<8>::new(SLOW);
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
    fn a_full_queue_drops_the_oldest() {
        let mut pacer = AnnouncePacer::<2>::new(SLOW);
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
}
