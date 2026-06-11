use crate::engine::InstantMillis;
use crate::interfaces::AnnounceBandwidthCap;
use crate::wire::BROADCAST_MTU;
use heapless::Vec as HeaplessVec;

pub trait PacerQueue: Default {
    fn insert(&mut self, bytes: &[u8], hops: u8);
    fn pop_priority_with<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<R>;
    fn is_empty(&self) -> bool;
}

struct Queued<F> {
    hops: u8,
    frame: F,
}

#[derive(Default)]
pub struct FixedPacerQueue<const DEPTH: usize> {
    entries: HeaplessVec<Queued<HeaplessVec<u8, BROADCAST_MTU>>, DEPTH>,
}

impl<const DEPTH: usize> PacerQueue for FixedPacerQueue<DEPTH> {
    fn insert(&mut self, bytes: &[u8], hops: u8) {
        let mut frame = HeaplessVec::new();
        if frame.extend_from_slice(bytes).is_err() {
            return;
        }
        if self.entries.is_full() {
            match self
                .entries
                .iter()
                .enumerate()
                .max_by_key(|(_, entry)| entry.hops)
                .map(|(index, entry)| (index, entry.hops))
            {
                Some((index, worst_hops)) if hops < worst_hops => {
                    self.entries.swap_remove(index);
                }
                _ => return,
            }
        }
        let _ = self.entries.push(Queued { hops, frame });
    }

    fn pop_priority_with<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        let index = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.hops)
            .map(|(index, _)| index)?;
        let entry = self.entries.swap_remove(index);
        Some(f(entry.frame.as_slice()))
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(feature = "alloc")]
pub use heap::HeapPacerQueue;

#[cfg(feature = "alloc")]
mod heap {
    use super::{PacerQueue, Queued};
    use alloc::vec::Vec;

    #[derive(Default)]
    pub struct HeapPacerQueue {
        entries: Vec<Queued<Vec<u8>>>,
    }

    impl PacerQueue for HeapPacerQueue {
        fn insert(&mut self, bytes: &[u8], hops: u8) {
            self.entries.push(Queued {
                hops,
                frame: bytes.to_vec(),
            });
        }

        fn pop_priority_with<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
            let index = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.hops)
                .map(|(index, _)| index)?;
            let entry = self.entries.swap_remove(index);
            Some(f(&entry.frame))
        }

        fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }
    }
}

pub struct AnnouncePacer<Q: PacerQueue> {
    cap: AnnounceBandwidthCap,
    bitrate_bps: Option<u32>,
    allowed_at: InstantMillis,
    queue: Q,
}

impl<Q: PacerQueue> AnnouncePacer<Q> {
    pub fn new(cap: AnnounceBandwidthCap, bitrate_bps: Option<u32>) -> Self {
        Self {
            cap,
            bitrate_bps,
            allowed_at: InstantMillis(0),
            queue: Q::default(),
        }
    }

    pub fn offer(&mut self, bytes: &[u8], hops: u8, now: InstantMillis, send: impl FnOnce(&[u8])) {
        if self.queue.is_empty() && self.allowed_at.0 <= now.0 {
            send(bytes);
            self.allowed_at = InstantMillis(
                now.0.saturating_add(
                    self.cap
                        .cooldown_after_send_ms(self.bitrate_bps, bytes.len()),
                ),
            );
        } else {
            self.queue.insert(bytes, hops);
        }
    }

    pub fn release_due(&mut self, now: InstantMillis, send: impl FnOnce(&[u8])) -> bool {
        if self.allowed_at.0 > now.0 {
            return false;
        }
        let cap = self.cap;
        let bitrate_bps = self.bitrate_bps;
        match self.queue.pop_priority_with(|bytes| {
            send(bytes);
            cap.cooldown_after_send_ms(bitrate_bps, bytes.len())
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

    const SLOW: AnnounceBandwidthCap = AnnounceBandwidthCap::RNS_DEFAULT;
    const SLOW_BITRATE: Option<u32> = Some(5_000);
    const SPACING_MS: u64 = 800;

    fn frame(tag: u8) -> [u8; 10] {
        [tag; 10]
    }

    fn capture() -> std::vec::Vec<std::vec::Vec<u8>> {
        std::vec::Vec::new()
    }

    #[test]
    fn an_unlimited_link_emits_immediately_and_never_queues() {
        let mut pacer =
            AnnouncePacer::<FixedPacerQueue<4>>::new(AnnounceBandwidthCap::Unlimited, None);
        let mut sent = capture();
        for at in [0, 1, 2, 3] {
            pacer.offer(&frame(at as u8), 1, InstantMillis(at), |b| {
                sent.push(b.to_vec())
            });
        }
        assert_eq!(sent.len(), 4);
        assert!(pacer.is_idle());
        assert_eq!(pacer.next_release(), None);
    }

    #[test]
    fn an_idle_pacer_emits_the_first_announce_now() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4>>::new(SLOW, SLOW_BITRATE);
        let mut sent = capture();
        pacer.offer(&frame(0), 1, InstantMillis(1_000), |b| {
            sent.push(b.to_vec())
        });
        assert_eq!(sent.len(), 1);
        assert_eq!(pacer.next_release(), None);
    }

    #[test]
    fn a_second_announce_within_the_window_queues() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4>>::new(SLOW, SLOW_BITRATE);
        let mut sent = capture();
        pacer.offer(&frame(0), 1, InstantMillis(1_000), |b| {
            sent.push(b.to_vec())
        });
        pacer.offer(&frame(1), 1, InstantMillis(1_500), |b| {
            sent.push(b.to_vec())
        });
        assert_eq!(sent.len(), 1, "the second is held, not emitted");
        assert_eq!(
            pacer.next_release(),
            Some(InstantMillis(1_000 + SPACING_MS))
        );
    }

    #[test]
    fn the_queue_releases_lowest_hops_first() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<8>>::new(SLOW, SLOW_BITRATE);
        let mut sent = capture();
        pacer.offer(&frame(9), 9, InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&frame(5), 5, InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&frame(1), 1, InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&frame(3), 3, InstantMillis(0), |b| sent.push(b.to_vec()));
        assert_eq!(sent, std::vec![frame(9).to_vec()], "hops-9 went out idle");

        let mut now = 0;
        for expected in [frame(1), frame(3), frame(5)] {
            now += SPACING_MS;
            assert!(pacer.release_due(InstantMillis(now), |b| sent.push(b.to_vec())));
            assert_eq!(*sent.last().unwrap(), expected.to_vec());
        }
        assert!(pacer.is_idle());
    }

    #[test]
    fn a_burst_drains_one_per_spacing_interval() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<8>>::new(SLOW, SLOW_BITRATE);
        let mut sent = capture();
        for n in 0..4 {
            pacer.offer(&frame(n), 1, InstantMillis(0), |b| sent.push(b.to_vec()));
        }
        assert_eq!(sent.len(), 1, "first goes now, the rest queue");

        let mut now = 0;
        for expected in 2..=4 {
            now += SPACING_MS;
            assert!(pacer.release_due(InstantMillis(now), |b| sent.push(b.to_vec())));
            assert_eq!(sent.len(), expected);
            assert!(
                !pacer.release_due(InstantMillis(now), |b| sent.push(b.to_vec())),
                "only one releases per interval"
            );
        }
        assert!(pacer.is_idle());
    }

    #[test]
    fn a_full_fixed_queue_evicts_the_worst_hops() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<2>>::new(SLOW, SLOW_BITRATE);
        let mut sent = capture();
        pacer.offer(&frame(5), 5, InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&frame(5), 5, InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&frame(5), 5, InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&frame(1), 1, InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&frame(9), 9, InstantMillis(0), |b| sent.push(b.to_vec()));

        let mut drained = capture();
        let mut now = 0;
        while !pacer.is_idle() {
            now += SPACING_MS;
            pacer.release_due(InstantMillis(now), |b| drained.push(b.to_vec()));
        }
        assert_eq!(
            drained[0],
            frame(1).to_vec(),
            "the best-hops survivor goes first"
        );
        assert_eq!(drained[1], frame(5).to_vec());
        assert!(
            !drained.contains(&frame(9).to_vec()),
            "the worse-than-queued hops-9 was dropped at the full gate"
        );
    }

    #[test]
    fn a_heap_queue_grows_without_dropping() {
        let mut pacer = AnnouncePacer::<HeapPacerQueue>::new(SLOW, SLOW_BITRATE);
        let mut sent = capture();
        for n in 0..64u8 {
            pacer.offer(&frame(n), 1, InstantMillis(0), |b| sent.push(b.to_vec()));
        }
        assert_eq!(sent.len(), 1, "first goes now, 63 queue and none drop");

        let mut released = 0;
        let mut now = 0;
        while !pacer.is_idle() {
            now += SPACING_MS;
            if pacer.release_due(InstantMillis(now), |_| {}) {
                released += 1;
            }
        }
        assert_eq!(released, 63);
    }
}
