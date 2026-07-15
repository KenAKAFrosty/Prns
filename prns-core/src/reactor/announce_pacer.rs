use crate::engine::InstantMillis;
use crate::interfaces::{AnnounceBandwidthCap, BitrateBps};
use crate::wire::BROADCAST_MTU;
use core::cmp::Reverse;
use heapless::Vec as HeaplessVec;

const QUEUED_ANNOUNCE_LIFE_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacerReject {
    FrameTooLarge,
    QueueFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacerOffer {
    Sent,
    Queued,
    Rejected(PacerReject),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacerRelease {
    Released,
    NotDue,
    Idle,
}

pub trait PacerQueue<M = ()>: Default {
    fn insert(
        &mut self,
        bytes: &[u8],
        hops: u8,
        now: InstantMillis,
        metadata: M,
    ) -> Result<(), PacerReject>;
    fn take_next_with<R>(&mut self, f: impl FnOnce(&[u8], M) -> R) -> Option<R>;
    fn evict_stale(&mut self, now: InstantMillis, life_ms: u64);
    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;
}

struct Queued<F, M> {
    hops: u8,
    queued_at: InstantMillis,
    frame: F,
    metadata: M,
}

pub struct FixedPacerQueue<const DEPTH: usize, M = ()> {
    entries: HeaplessVec<Queued<HeaplessVec<u8, BROADCAST_MTU>, M>, DEPTH>,
}

impl<const DEPTH: usize, M> Default for FixedPacerQueue<DEPTH, M> {
    fn default() -> Self {
        Self {
            entries: HeaplessVec::new(),
        }
    }
}

impl<const DEPTH: usize, M: Copy> PacerQueue<M> for FixedPacerQueue<DEPTH, M> {
    fn insert(
        &mut self,
        bytes: &[u8],
        hops: u8,
        now: InstantMillis,
        metadata: M,
    ) -> Result<(), PacerReject> {
        let mut frame = HeaplessVec::new();
        if frame.extend_from_slice(bytes).is_err() {
            return Err(PacerReject::FrameTooLarge);
        }
        if self.entries.is_full() {
            match self
                .entries
                .iter()
                .enumerate()
                .max_by_key(|(_, entry)| (entry.hops, Reverse(entry.queued_at.0)))
                .map(|(index, entry)| (index, entry.hops))
            {
                Some((index, worst_hops)) if hops < worst_hops => {
                    self.entries.swap_remove(index);
                }
                _ => return Err(PacerReject::QueueFull),
            }
        }
        self.entries
            .push(Queued {
                hops,
                queued_at: now,
                frame,
                metadata,
            })
            .map_err(|_| PacerReject::QueueFull)
    }

    fn take_next_with<R>(&mut self, f: impl FnOnce(&[u8], M) -> R) -> Option<R> {
        let index = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| (entry.hops, entry.queued_at.0))
            .map(|(index, _)| index)?;
        let entry = self.entries.swap_remove(index);
        Some(f(entry.frame.as_slice(), entry.metadata))
    }

    fn evict_stale(&mut self, now: InstantMillis, life_ms: u64) {
        let mut index = 0;
        while index < self.entries.len() {
            if now.0.saturating_sub(self.entries[index].queued_at.0) > life_ms {
                self.entries.swap_remove(index);
            } else {
                index += 1;
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(feature = "alloc")]
pub use heap::HeapPacerQueue;

#[cfg(feature = "alloc")]
mod heap {
    use super::{PacerQueue, PacerReject, Queued};
    use crate::engine::InstantMillis;
    use alloc::vec::Vec;

    pub struct HeapPacerQueue<M = ()> {
        entries: Vec<Queued<Vec<u8>, M>>,
    }

    impl<M> Default for HeapPacerQueue<M> {
        fn default() -> Self {
            Self {
                entries: Vec::new(),
            }
        }
    }

    impl<M: Copy> PacerQueue<M> for HeapPacerQueue<M> {
        fn insert(
            &mut self,
            bytes: &[u8],
            hops: u8,
            now: InstantMillis,
            metadata: M,
        ) -> Result<(), PacerReject> {
            self.entries.push(Queued {
                hops,
                queued_at: now,
                frame: bytes.to_vec(),
                metadata,
            });
            Ok(())
        }

        fn take_next_with<R>(&mut self, f: impl FnOnce(&[u8], M) -> R) -> Option<R> {
            let index = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| (entry.hops, entry.queued_at.0))
                .map(|(index, _)| index)?;
            let entry = self.entries.swap_remove(index);
            Some(f(&entry.frame, entry.metadata))
        }

        fn evict_stale(&mut self, now: InstantMillis, life_ms: u64) {
            self.entries
                .retain(|entry| now.0.saturating_sub(entry.queued_at.0) <= life_ms);
        }

        fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }

        fn len(&self) -> usize {
            self.entries.len()
        }
    }
}

pub struct AnnouncePacer<Q, M = ()>
where
    Q: PacerQueue<M>,
{
    cap: AnnounceBandwidthCap,
    bitrate: BitrateBps,
    allowed_at: InstantMillis,
    queue: Q,
    metadata: core::marker::PhantomData<fn(M)>,
}

impl<Q, M> AnnouncePacer<Q, M>
where
    Q: PacerQueue<M>,
    M: Copy,
{
    pub fn new(cap: AnnounceBandwidthCap, bitrate: BitrateBps) -> Self {
        Self {
            cap,
            bitrate,
            allowed_at: InstantMillis(0),
            queue: Q::default(),
            metadata: core::marker::PhantomData,
        }
    }

    pub fn offer_tagged(
        &mut self,
        bytes: &[u8],
        hops: u8,
        now: InstantMillis,
        metadata: M,
        send: impl FnOnce(&[u8], M),
    ) -> PacerOffer {
        self.queue.evict_stale(now, QUEUED_ANNOUNCE_LIFE_MS);
        if self.queue.is_empty() && self.allowed_at.0 <= now.0 {
            send(bytes, metadata);
            self.allowed_at = InstantMillis(
                now.0
                    .saturating_add(self.cap.cooldown_after_send_ms(self.bitrate, bytes.len())),
            );
            PacerOffer::Sent
        } else {
            match self.queue.insert(bytes, hops, now, metadata) {
                Ok(()) => PacerOffer::Queued,
                Err(reason) => PacerOffer::Rejected(reason),
            }
        }
    }

    pub fn release_due_tagged(
        &mut self,
        now: InstantMillis,
        send: impl FnOnce(&[u8], M),
    ) -> PacerRelease {
        if self.allowed_at.0 > now.0 {
            return PacerRelease::NotDue;
        }
        self.queue.evict_stale(now, QUEUED_ANNOUNCE_LIFE_MS);
        let cap = self.cap;
        let bitrate = self.bitrate;
        match self.queue.take_next_with(|bytes, metadata| {
            send(bytes, metadata);
            cap.cooldown_after_send_ms(bitrate, bytes.len())
        }) {
            Some(spacing) => {
                self.allowed_at = InstantMillis(now.0.saturating_add(spacing));
                PacerRelease::Released
            }
            None => PacerRelease::Idle,
        }
    }

    pub fn next_release(&self) -> Option<InstantMillis> {
        (!self.queue.is_empty()).then_some(self.allowed_at)
    }

    pub fn is_idle(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn queued_len(&self) -> usize {
        self.queue.len()
    }
}

impl<Q> AnnouncePacer<Q>
where
    Q: PacerQueue<()>,
{
    pub fn offer(
        &mut self,
        bytes: &[u8],
        hops: u8,
        now: InstantMillis,
        send: impl FnOnce(&[u8]),
    ) -> PacerOffer {
        self.offer_tagged(bytes, hops, now, (), |frame, ()| send(frame))
    }

    pub fn release_due(&mut self, now: InstantMillis, send: impl FnOnce(&[u8])) -> PacerRelease {
        self.release_due_tagged(now, |frame, ()| send(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOW: AnnounceBandwidthCap = AnnounceBandwidthCap::RNS_DEFAULT;
    const SLOW_BITRATE: BitrateBps = BitrateBps::guess(5_000);
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
            AnnouncePacer::<FixedPacerQueue<4>>::new(AnnounceBandwidthCap::Unlimited, SLOW_BITRATE);
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
            assert_eq!(
                pacer.release_due(InstantMillis(now), |b| sent.push(b.to_vec())),
                PacerRelease::Released
            );
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
            assert_eq!(
                pacer.release_due(InstantMillis(now), |b| sent.push(b.to_vec())),
                PacerRelease::Released
            );
            assert_eq!(sent.len(), expected);
            assert_eq!(
                pacer.release_due(InstantMillis(now), |b| sent.push(b.to_vec())),
                PacerRelease::NotDue,
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
        assert_eq!(
            pacer.offer(&frame(9), 9, InstantMillis(0), |b| sent.push(b.to_vec())),
            PacerOffer::Rejected(PacerReject::QueueFull),
            "hops-9 is worse than every held announce, so the full gate rejects it",
        );

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
            if matches!(
                pacer.release_due(InstantMillis(now), |_| {}),
                PacerRelease::Released
            ) {
                released += 1;
            }
        }
        assert_eq!(released, 63);
    }

    #[test]
    fn equal_hops_release_in_time_order_despite_internal_reordering() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<8>>::new(SLOW, SLOW_BITRATE);
        let mut sent = capture();
        pacer.offer(&frame(0), 2, InstantMillis(0), |b| sent.push(b.to_vec()));
        for (tag, queued_at) in [(1u8, 100u64), (2, 200), (3, 300), (4, 400)] {
            pacer.offer(&frame(tag), 2, InstantMillis(queued_at), |b| {
                sent.push(b.to_vec())
            });
        }
        assert_eq!(
            sent,
            std::vec![frame(0).to_vec()],
            "the first went out idle"
        );

        let mut now = 0;
        for expected in [frame(1), frame(2), frame(3), frame(4)] {
            now += SPACING_MS;
            assert_eq!(
                pacer.release_due(InstantMillis(now), |b| sent.push(b.to_vec())),
                PacerRelease::Released
            );
            assert_eq!(
                *sent.last().unwrap(),
                expected.to_vec(),
                "same-hops announces leave oldest-first even as swap_remove shuffles storage",
            );
        }
        assert!(pacer.is_idle());
    }

    #[test]
    fn a_stale_queued_announce_is_swept_and_a_fresh_one_sends() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<8>>::new(SLOW, SLOW_BITRATE);
        let mut sent = capture();
        pacer.offer(&frame(0), 1, InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&frame(1), 1, InstantMillis(400), |b| sent.push(b.to_vec()));
        assert_eq!(sent, std::vec![frame(0).to_vec()], "the second is held");
        assert!(!pacer.is_idle());

        let long_after = 400 + QUEUED_ANNOUNCE_LIFE_MS + 1;
        pacer.offer(&frame(2), 1, InstantMillis(long_after), |b| {
            sent.push(b.to_vec())
        });
        assert_eq!(
            sent,
            std::vec![frame(0).to_vec(), frame(2).to_vec()],
            "the day-old held announce was swept, never sent; the fresh one goes out",
        );
        assert!(pacer.is_idle());
    }

    #[test]
    fn release_sweeps_a_stale_queue_and_sends_nothing() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<8>>::new(SLOW, SLOW_BITRATE);
        let mut sent = capture();
        pacer.offer(&frame(0), 1, InstantMillis(0), |b| sent.push(b.to_vec()));
        pacer.offer(&frame(1), 1, InstantMillis(400), |b| sent.push(b.to_vec()));

        let long_after = 400 + QUEUED_ANNOUNCE_LIFE_MS + 1;
        assert_eq!(
            pacer.release_due(InstantMillis(long_after), |b| sent.push(b.to_vec())),
            PacerRelease::Idle,
            "the only held announce aged out, so the release finds nothing to send",
        );
        assert_eq!(sent, std::vec![frame(0).to_vec()]);
        assert!(pacer.is_idle(), "the stale entry was swept from the queue");
    }
}
