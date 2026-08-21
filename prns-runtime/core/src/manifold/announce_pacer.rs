//! Ownership-preserving announce pacing.
//!
//! A pacer owns an announce from the time it is accepted until egress admits it
//! or reports a terminal discard. Backpressure never removes and reinserts the
//! queued frame, so retries preserve both allocation identity and queue age.

use crate::engine::InstantMillis;
use crate::interfaces::{AnnounceBandwidthCap, BitrateBps};
use crate::wire::BROADCAST_MTU;
use core::cmp::Reverse;
use heapless::Vec as HeaplessVec;

const QUEUED_ANNOUNCE_LIFE_MS: u64 = 24 * 60 * 60 * 1_000;
const TERMINAL_DISCARD_YIELD_MS: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacerReject {
    FrameTooLarge,
    QueueFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacerDelivery {
    Admitted,
    Backpressured,
    Discarded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacerHold {
    Bandwidth,
    Congestion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacerBackpressure {
    Deferred,
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacerOffer {
    Admitted,
    Held(PacerHold),
    Deferred,
    Discarded,
    Rejected(PacerReject),
}

/// Result of attempting to release the next queued announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacerRelease {
    Admitted,
    Backpressured(PacerBackpressure),
    Discarded,
    NotDue,
    Idle,
}

/// Runtime-specific retry timing for transient egress congestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacerRetryPolicy {
    initial_delay_ms: u64,
    maximum_delay_ms: u64,
}

impl PacerRetryPolicy {
    /// Creates a non-zero exponential retry policy.
    pub const fn new(initial_delay_ms: u64, maximum_delay_ms: u64) -> Self {
        assert!(initial_delay_ms > 0, "pacer retry delay must be non-zero");
        assert!(
            initial_delay_ms <= maximum_delay_ms,
            "pacer retry ceiling must cover its initial delay"
        );
        Self {
            initial_delay_ms,
            maximum_delay_ms,
        }
    }

    pub const fn initial_delay_ms(self) -> u64 {
        self.initial_delay_ms
    }

    pub const fn maximum_delay_ms(self) -> u64 {
        self.maximum_delay_ms
    }
}

/// Stable metadata describing one entry owned by a pacer queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacerEntry<M> {
    pub metadata: M,
    pub frame_bytes: usize,
    pub hops: u8,
    pub queued_at: InstantMillis,
    pub deferred_since: Option<InstantMillis>,
}

/// A lifecycle event emitted while the pacer retains or sheds an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacerEvent<M> {
    Deferred(PacerEntry<M>),
    Retry(PacerEntry<M>),
    Recovered(PacerEntry<M>),
    Evicted(PacerEntry<M>),
    Expired(PacerEntry<M>),
}

/// Result of inserting an entry into a pacer queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacerQueueInsert<M> {
    Inserted,
    Replaced(PacerEntry<M>),
    Rejected(PacerReject),
}

/// Result of conditionally attempting the highest-priority queued entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacerQueueAttempt<M> {
    pub delivery: PacerDelivery,
    pub entry: PacerEntry<M>,
    pub was_deferred: bool,
}

/// Storage contract used by [`AnnouncePacer`].
///
/// `attempt_next_with` must leave the selected entry in place when the callback
/// returns [`PacerDelivery::Backpressured`]. Implementations must not clone,
/// allocate, remove, or reinsert the frame on that path.
pub trait PacerQueue<M = ()>: Default {
    fn insert(
        &mut self,
        bytes: &[u8],
        hops: u8,
        now: InstantMillis,
        metadata: M,
        deferred_since: Option<InstantMillis>,
    ) -> PacerQueueInsert<M>;

    fn attempt_next_with(
        &mut self,
        now: InstantMillis,
        f: impl FnOnce(&[u8], u8, M) -> PacerDelivery,
    ) -> Option<PacerQueueAttempt<M>>;

    fn evict_stale(
        &mut self,
        now: InstantMillis,
        life_ms: u64,
        on_evict: impl FnMut(PacerEntry<M>),
    );

    fn next_hops(&self) -> Option<u8>;
    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;
    fn deferred_len(&self) -> usize;
    fn oldest_deferred_at(&self) -> Option<InstantMillis>;

    fn clear(&mut self) -> usize {
        let mut removed = 0;
        while self
            .attempt_next_with(InstantMillis(0), |_, _, _| PacerDelivery::Discarded)
            .is_some()
        {
            removed += 1;
        }
        removed
    }
}

struct Queued<F, M> {
    hops: u8,
    queued_at: InstantMillis,
    deferred_since: Option<InstantMillis>,
    frame: F,
    metadata: M,
}

impl<F: AsRef<[u8]>, M: Copy> Queued<F, M> {
    fn snapshot(&self) -> PacerEntry<M> {
        PacerEntry {
            metadata: self.metadata,
            frame_bytes: self.frame.as_ref().len(),
            hops: self.hops,
            queued_at: self.queued_at,
            deferred_since: self.deferred_since,
        }
    }
}

fn next_index<F, M>(entries: &[Queued<F, M>]) -> Option<usize> {
    entries
        .iter()
        .enumerate()
        .min_by_key(|(_, entry)| (entry.hops, entry.queued_at.0))
        .map(|(index, _)| index)
}

fn worst_index<F, M>(entries: &[Queued<F, M>]) -> Option<(usize, u8)> {
    entries
        .iter()
        .enumerate()
        .max_by_key(|(_, entry)| (entry.hops, Reverse(entry.queued_at.0)))
        .map(|(index, entry)| (index, entry.hops))
}

/// A fixed-storage pacer queue suitable for embedded runtimes.
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
        deferred_since: Option<InstantMillis>,
    ) -> PacerQueueInsert<M> {
        let mut frame = HeaplessVec::new();
        if frame.extend_from_slice(bytes).is_err() {
            return PacerQueueInsert::Rejected(PacerReject::FrameTooLarge);
        }

        let replaced = if self.entries.is_full() {
            match worst_index(self.entries.as_slice()) {
                Some((index, worst_hops)) if hops < worst_hops => {
                    Some(self.entries.swap_remove(index).snapshot())
                }
                _ => return PacerQueueInsert::Rejected(PacerReject::QueueFull),
            }
        } else {
            None
        };

        if self
            .entries
            .push(Queued {
                hops,
                queued_at: now,
                deferred_since,
                frame,
                metadata,
            })
            .is_err()
        {
            return PacerQueueInsert::Rejected(PacerReject::QueueFull);
        }
        replaced.map_or(PacerQueueInsert::Inserted, PacerQueueInsert::Replaced)
    }

    fn attempt_next_with(
        &mut self,
        now: InstantMillis,
        f: impl FnOnce(&[u8], u8, M) -> PacerDelivery,
    ) -> Option<PacerQueueAttempt<M>> {
        let index = next_index(self.entries.as_slice())?;
        let was_deferred = self.entries[index].deferred_since.is_some();
        let delivery = {
            let entry = &self.entries[index];
            f(entry.frame.as_slice(), entry.hops, entry.metadata)
        };
        if delivery == PacerDelivery::Backpressured && self.entries[index].deferred_since.is_none()
        {
            self.entries[index].deferred_since = Some(now);
        }
        let entry = self.entries[index].snapshot();
        if delivery != PacerDelivery::Backpressured {
            self.entries.swap_remove(index);
        }
        Some(PacerQueueAttempt {
            delivery,
            entry,
            was_deferred,
        })
    }

    fn evict_stale(
        &mut self,
        now: InstantMillis,
        life_ms: u64,
        mut on_evict: impl FnMut(PacerEntry<M>),
    ) {
        let mut index = 0;
        while index < self.entries.len() {
            if now.0.saturating_sub(self.entries[index].queued_at.0) > life_ms {
                let entry = self.entries.swap_remove(index);
                on_evict(entry.snapshot());
            } else {
                index += 1;
            }
        }
    }

    fn next_hops(&self) -> Option<u8> {
        next_index(self.entries.as_slice()).map(|index| self.entries[index].hops)
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn deferred_len(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.deferred_since.is_some())
            .count()
    }

    fn oldest_deferred_at(&self) -> Option<InstantMillis> {
        self.entries
            .iter()
            .filter_map(|entry| entry.deferred_since)
            .min_by_key(|at| at.0)
    }
}

#[cfg(feature = "alloc")]
pub use heap::{BoundedHeapPacerQueue, HeapPacerQueue};

#[cfg(feature = "alloc")]
mod heap {
    use super::{
        next_index, worst_index, PacerDelivery, PacerEntry, PacerQueue, PacerQueueAttempt,
        PacerQueueInsert, PacerReject, Queued, BROADCAST_MTU,
    };
    use crate::engine::InstantMillis;
    use alloc::vec::Vec;

    /// An allocation-backed pacer queue with no entry-count bound.
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

    /// An allocation-backed pacer queue with a compile-time entry limit.
    pub struct BoundedHeapPacerQueue<const DEPTH: usize, M = ()> {
        entries: Vec<Queued<Vec<u8>, M>>,
    }

    impl<const DEPTH: usize, M> Default for BoundedHeapPacerQueue<DEPTH, M> {
        fn default() -> Self {
            Self {
                entries: Vec::with_capacity(DEPTH),
            }
        }
    }

    fn insert_unbounded<M: Copy>(
        entries: &mut Vec<Queued<Vec<u8>, M>>,
        bytes: &[u8],
        hops: u8,
        now: InstantMillis,
        metadata: M,
        deferred_since: Option<InstantMillis>,
    ) -> PacerQueueInsert<M> {
        if bytes.len() > BROADCAST_MTU {
            return PacerQueueInsert::Rejected(PacerReject::FrameTooLarge);
        }
        entries.push(Queued {
            hops,
            queued_at: now,
            deferred_since,
            frame: bytes.to_vec(),
            metadata,
        });
        PacerQueueInsert::Inserted
    }

    fn insert_bounded<const DEPTH: usize, M: Copy>(
        entries: &mut Vec<Queued<Vec<u8>, M>>,
        bytes: &[u8],
        hops: u8,
        now: InstantMillis,
        metadata: M,
        deferred_since: Option<InstantMillis>,
    ) -> PacerQueueInsert<M> {
        if bytes.len() > BROADCAST_MTU {
            return PacerQueueInsert::Rejected(PacerReject::FrameTooLarge);
        }
        let replaced = if entries.len() == DEPTH {
            match worst_index(entries.as_slice()) {
                Some((index, worst_hops)) if hops < worst_hops => {
                    Some(entries.swap_remove(index).snapshot())
                }
                _ => return PacerQueueInsert::Rejected(PacerReject::QueueFull),
            }
        } else {
            None
        };
        entries.push(Queued {
            hops,
            queued_at: now,
            deferred_since,
            frame: bytes.to_vec(),
            metadata,
        });
        replaced.map_or(PacerQueueInsert::Inserted, PacerQueueInsert::Replaced)
    }

    fn attempt<M: Copy>(
        entries: &mut Vec<Queued<Vec<u8>, M>>,
        now: InstantMillis,
        f: impl FnOnce(&[u8], u8, M) -> PacerDelivery,
    ) -> Option<PacerQueueAttempt<M>> {
        let index = next_index(entries.as_slice())?;
        let was_deferred = entries[index].deferred_since.is_some();
        let delivery = {
            let entry = &entries[index];
            f(entry.frame.as_slice(), entry.hops, entry.metadata)
        };
        if delivery == PacerDelivery::Backpressured && entries[index].deferred_since.is_none() {
            entries[index].deferred_since = Some(now);
        }
        let entry = entries[index].snapshot();
        if delivery != PacerDelivery::Backpressured {
            entries.swap_remove(index);
        }
        Some(PacerQueueAttempt {
            delivery,
            entry,
            was_deferred,
        })
    }

    fn evict<M: Copy>(
        entries: &mut Vec<Queued<Vec<u8>, M>>,
        now: InstantMillis,
        life_ms: u64,
        mut on_evict: impl FnMut(PacerEntry<M>),
    ) {
        let mut index = 0;
        while index < entries.len() {
            if now.0.saturating_sub(entries[index].queued_at.0) > life_ms {
                let entry = entries.swap_remove(index);
                on_evict(entry.snapshot());
            } else {
                index += 1;
            }
        }
    }

    fn next_hops<M>(entries: &[Queued<Vec<u8>, M>]) -> Option<u8> {
        next_index(entries).map(|index| entries[index].hops)
    }

    fn deferred_len<M>(entries: &[Queued<Vec<u8>, M>]) -> usize {
        entries
            .iter()
            .filter(|entry| entry.deferred_since.is_some())
            .count()
    }

    fn oldest_deferred_at<M>(entries: &[Queued<Vec<u8>, M>]) -> Option<InstantMillis> {
        entries
            .iter()
            .filter_map(|entry| entry.deferred_since)
            .min_by_key(|at| at.0)
    }

    macro_rules! impl_pacer_queue {
        ($queue:ty, $insert:expr) => {
            impl<M: Copy> PacerQueue<M> for $queue {
                fn insert(
                    &mut self,
                    bytes: &[u8],
                    hops: u8,
                    now: InstantMillis,
                    metadata: M,
                    deferred_since: Option<InstantMillis>,
                ) -> PacerQueueInsert<M> {
                    $insert(
                        &mut self.entries,
                        bytes,
                        hops,
                        now,
                        metadata,
                        deferred_since,
                    )
                }

                fn attempt_next_with(
                    &mut self,
                    now: InstantMillis,
                    f: impl FnOnce(&[u8], u8, M) -> PacerDelivery,
                ) -> Option<PacerQueueAttempt<M>> {
                    attempt(&mut self.entries, now, f)
                }

                fn evict_stale(
                    &mut self,
                    now: InstantMillis,
                    life_ms: u64,
                    on_evict: impl FnMut(PacerEntry<M>),
                ) {
                    evict(&mut self.entries, now, life_ms, on_evict);
                }

                fn next_hops(&self) -> Option<u8> {
                    next_hops(&self.entries)
                }

                fn is_empty(&self) -> bool {
                    self.entries.is_empty()
                }

                fn len(&self) -> usize {
                    self.entries.len()
                }

                fn deferred_len(&self) -> usize {
                    deferred_len(&self.entries)
                }

                fn oldest_deferred_at(&self) -> Option<InstantMillis> {
                    oldest_deferred_at(&self.entries)
                }
            }
        };
    }

    impl_pacer_queue!(HeapPacerQueue<M>, insert_unbounded);

    impl<const DEPTH: usize, M: Copy> PacerQueue<M> for BoundedHeapPacerQueue<DEPTH, M> {
        fn insert(
            &mut self,
            bytes: &[u8],
            hops: u8,
            now: InstantMillis,
            metadata: M,
            deferred_since: Option<InstantMillis>,
        ) -> PacerQueueInsert<M> {
            insert_bounded::<DEPTH, M>(
                &mut self.entries,
                bytes,
                hops,
                now,
                metadata,
                deferred_since,
            )
        }

        fn attempt_next_with(
            &mut self,
            now: InstantMillis,
            f: impl FnOnce(&[u8], u8, M) -> PacerDelivery,
        ) -> Option<PacerQueueAttempt<M>> {
            attempt(&mut self.entries, now, f)
        }

        fn evict_stale(
            &mut self,
            now: InstantMillis,
            life_ms: u64,
            on_evict: impl FnMut(PacerEntry<M>),
        ) {
            evict(&mut self.entries, now, life_ms, on_evict);
        }

        fn next_hops(&self) -> Option<u8> {
            next_hops(&self.entries)
        }

        fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }

        fn len(&self) -> usize {
            self.entries.len()
        }

        fn deferred_len(&self) -> usize {
            deferred_len(&self.entries)
        }

        fn oldest_deferred_at(&self) -> Option<InstantMillis> {
            oldest_deferred_at(&self.entries)
        }
    }
}

/// A bandwidth pacer with lossless deferral under transient backpressure.
pub struct AnnouncePacer<Q, M = ()>
where
    Q: PacerQueue<M>,
{
    cap: AnnounceBandwidthCap,
    bitrate: BitrateBps,
    bandwidth_allowed_at: InstantMillis,
    congestion_allowed_at: InstantMillis,
    retry_policy: PacerRetryPolicy,
    next_retry_delay_ms: u64,
    last_emitted_hops: u8,
    queue: Q,
    metadata: core::marker::PhantomData<fn(M)>,
}

impl<Q, M> AnnouncePacer<Q, M>
where
    Q: PacerQueue<M>,
    M: Copy,
{
    pub fn new(
        cap: AnnounceBandwidthCap,
        bitrate: BitrateBps,
        retry_policy: PacerRetryPolicy,
    ) -> Self {
        let bandwidth_allowed_at = match cap {
            AnnounceBandwidthCap::Limited { cap_per_mille: 0 } => InstantMillis(u64::MAX),
            AnnounceBandwidthCap::Unlimited | AnnounceBandwidthCap::Limited { .. } => {
                InstantMillis(0)
            }
        };
        Self {
            cap,
            bitrate,
            bandwidth_allowed_at,
            congestion_allowed_at: InstantMillis(0),
            retry_policy,
            next_retry_delay_ms: retry_policy.initial_delay_ms,
            last_emitted_hops: 0,
            queue: Q::default(),
            metadata: core::marker::PhantomData,
        }
    }

    fn reset_congestion(&mut self) {
        self.congestion_allowed_at = InstantMillis(0);
        self.next_retry_delay_ms = self.retry_policy.initial_delay_ms;
    }

    fn defer_for_backpressure(&mut self, now: InstantMillis) {
        self.congestion_allowed_at = InstantMillis(now.0.saturating_add(self.next_retry_delay_ms));
        self.next_retry_delay_ms = self
            .next_retry_delay_ms
            .saturating_mul(2)
            .min(self.retry_policy.maximum_delay_ms);
    }

    fn yield_after_discard(&mut self, now: InstantMillis) {
        self.reset_congestion();
        self.congestion_allowed_at = InstantMillis(now.0.saturating_add(TERMINAL_DISCARD_YIELD_MS));
    }

    fn charge_bandwidth(&mut self, now: InstantMillis, bytes: usize, hops: u8) {
        self.bandwidth_allowed_at = InstantMillis(
            now.0
                .saturating_add(self.cap.cooldown_after_send_ms(self.bitrate, bytes)),
        );
        self.last_emitted_hops = hops;
        self.reset_congestion();
    }

    fn evict_stale(&mut self, now: InstantMillis, observe: &mut impl FnMut(PacerEvent<M>)) {
        self.queue
            .evict_stale(now, QUEUED_ANNOUNCE_LIFE_MS, |entry| {
                observe(PacerEvent::Expired(entry));
            });
    }

    fn insert(
        &mut self,
        bytes: &[u8],
        hops: u8,
        now: InstantMillis,
        metadata: M,
        deferred_since: Option<InstantMillis>,
        observe: &mut impl FnMut(PacerEvent<M>),
    ) -> Result<(), PacerReject> {
        match self
            .queue
            .insert(bytes, hops, now, metadata, deferred_since)
        {
            PacerQueueInsert::Inserted => Ok(()),
            PacerQueueInsert::Replaced(entry) => {
                observe(PacerEvent::Evicted(entry));
                Ok(())
            }
            PacerQueueInsert::Rejected(reason) => Err(reason),
        }
    }

    pub fn offer_tagged_observed(
        &mut self,
        bytes: &[u8],
        hops: u8,
        now: InstantMillis,
        metadata: M,
        send: impl FnOnce(&[u8], M) -> PacerDelivery,
        mut observe: impl FnMut(PacerEvent<M>),
    ) -> PacerOffer {
        self.evict_stale(now, &mut observe);
        let idle = self.queue.is_empty()
            && self.bandwidth_allowed_at.0 <= now.0
            && self.congestion_allowed_at.0 <= now.0;
        let origin_preempts_forwarded = hops == 0
            && self.last_emitted_hops > 0
            && !self.cap.blocks_all()
            && self.congestion_allowed_at.0 <= now.0;
        if idle || origin_preempts_forwarded {
            match send(bytes, metadata) {
                PacerDelivery::Admitted => {
                    self.charge_bandwidth(now, bytes.len(), hops);
                    PacerOffer::Admitted
                }
                PacerDelivery::Backpressured => {
                    self.defer_for_backpressure(now);
                    match self.insert(bytes, hops, now, metadata, Some(now), &mut observe) {
                        Ok(()) => {
                            observe(PacerEvent::Deferred(PacerEntry {
                                metadata,
                                frame_bytes: bytes.len(),
                                hops,
                                queued_at: now,
                                deferred_since: Some(now),
                            }));
                            PacerOffer::Deferred
                        }
                        Err(reason) => {
                            self.yield_after_discard(now);
                            PacerOffer::Rejected(reason)
                        }
                    }
                }
                PacerDelivery::Discarded => {
                    self.yield_after_discard(now);
                    PacerOffer::Discarded
                }
            }
        } else {
            let hold = if self.congestion_allowed_at.0 > now.0 {
                PacerHold::Congestion
            } else {
                PacerHold::Bandwidth
            };
            match self.insert(bytes, hops, now, metadata, None, &mut observe) {
                Ok(()) => PacerOffer::Held(hold),
                Err(reason) => PacerOffer::Rejected(reason),
            }
        }
    }

    pub fn offer_tagged(
        &mut self,
        bytes: &[u8],
        hops: u8,
        now: InstantMillis,
        metadata: M,
        send: impl FnOnce(&[u8], M) -> PacerDelivery,
    ) -> PacerOffer {
        self.offer_tagged_observed(bytes, hops, now, metadata, send, |_| {})
    }

    pub fn release_due_tagged_observed(
        &mut self,
        now: InstantMillis,
        send: impl FnOnce(&[u8], M) -> PacerDelivery,
        mut observe: impl FnMut(PacerEvent<M>),
    ) -> PacerRelease {
        self.evict_stale(now, &mut observe);
        let Some(due_at) = self.next_release() else {
            return PacerRelease::Idle;
        };
        if due_at.0 > now.0 {
            return PacerRelease::NotDue;
        }

        let Some(attempt) = self
            .queue
            .attempt_next_with(now, |bytes, _, metadata| send(bytes, metadata))
        else {
            return PacerRelease::Idle;
        };
        match attempt.delivery {
            PacerDelivery::Admitted => {
                if attempt.was_deferred {
                    observe(PacerEvent::Recovered(attempt.entry));
                }
                self.charge_bandwidth(now, attempt.entry.frame_bytes, attempt.entry.hops);
                PacerRelease::Admitted
            }
            PacerDelivery::Backpressured => {
                self.defer_for_backpressure(now);
                if attempt.was_deferred {
                    observe(PacerEvent::Retry(attempt.entry));
                    PacerRelease::Backpressured(PacerBackpressure::Retry)
                } else {
                    observe(PacerEvent::Deferred(attempt.entry));
                    PacerRelease::Backpressured(PacerBackpressure::Deferred)
                }
            }
            PacerDelivery::Discarded => {
                self.yield_after_discard(now);
                PacerRelease::Discarded
            }
        }
    }

    pub fn release_due_tagged(
        &mut self,
        now: InstantMillis,
        send: impl FnOnce(&[u8], M) -> PacerDelivery,
    ) -> PacerRelease {
        self.release_due_tagged_observed(now, send, |_| {})
    }

    pub fn next_release(&self) -> Option<InstantMillis> {
        let hops = self.queue.next_hops()?;
        if self.cap.blocks_all() {
            return None;
        }
        let bandwidth_at = if hops == 0 && self.last_emitted_hops > 0 {
            InstantMillis(0)
        } else {
            self.bandwidth_allowed_at
        };
        Some(InstantMillis(
            bandwidth_at.0.max(self.congestion_allowed_at.0),
        ))
    }

    pub fn is_idle(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn queued_len(&self) -> usize {
        self.queue.len()
    }

    pub fn deferred_len(&self) -> usize {
        self.queue.deferred_len()
    }

    pub fn oldest_deferred_at(&self) -> Option<InstantMillis> {
        self.queue.oldest_deferred_at()
    }

    pub fn clear_queue(&mut self) -> usize {
        self.queue.clear()
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
        send: impl FnOnce(&[u8]) -> PacerDelivery,
    ) -> PacerOffer {
        self.offer_tagged(bytes, hops, now, (), |frame, ()| send(frame))
    }

    pub fn release_due(
        &mut self,
        now: InstantMillis,
        send: impl FnOnce(&[u8]) -> PacerDelivery,
    ) -> PacerRelease {
        self.release_due_tagged(now, |frame, ()| send(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOW: AnnounceBandwidthCap = AnnounceBandwidthCap::RNS_DEFAULT;
    const SLOW_BITRATE: BitrateBps = BitrateBps::guess(5_000);
    const SPACING_MS: u64 = 800;
    const RETRY: PacerRetryPolicy = PacerRetryPolicy::new(50, 1_000);

    fn frame(tag: u8) -> [u8; 10] {
        [tag; 10]
    }

    fn capture() -> std::vec::Vec<std::vec::Vec<u8>> {
        std::vec::Vec::new()
    }

    fn admit(bytes: &[u8], sent: &mut std::vec::Vec<std::vec::Vec<u8>>) -> PacerDelivery {
        sent.push(bytes.to_vec());
        PacerDelivery::Admitted
    }

    #[test]
    fn an_unlimited_link_emits_immediately_and_never_queues() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4>>::new(
            AnnounceBandwidthCap::Unlimited,
            SLOW_BITRATE,
            RETRY,
        );
        let mut sent = capture();
        for at in [0, 1, 2, 3] {
            assert_eq!(
                pacer.offer(&frame(at as u8), 1, InstantMillis(at), |bytes| {
                    admit(bytes, &mut sent)
                }),
                PacerOffer::Admitted
            );
        }
        assert_eq!(sent.len(), 4);
        assert!(pacer.is_idle());
        assert_eq!(pacer.next_release(), None);
    }

    #[test]
    fn bandwidth_holds_and_releases_lowest_hops_then_oldest() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<8>>::new(SLOW, SLOW_BITRATE, RETRY);
        let mut sent = capture();
        assert_eq!(
            pacer.offer(&frame(9), 9, InstantMillis(0), |bytes| admit(
                bytes, &mut sent
            )),
            PacerOffer::Admitted
        );
        for (tag, hops, at) in [(5, 5, 1), (2, 2, 2), (1, 2, 1), (3, 3, 3)] {
            assert_eq!(
                pacer.offer(&frame(tag), hops, InstantMillis(at), |bytes| {
                    admit(bytes, &mut sent)
                }),
                PacerOffer::Held(PacerHold::Bandwidth)
            );
        }

        for (index, expected) in [frame(1), frame(2), frame(3), frame(5)]
            .into_iter()
            .enumerate()
        {
            let at = SPACING_MS * (index as u64 + 1);
            assert_eq!(
                pacer.release_due(InstantMillis(at), |bytes| admit(bytes, &mut sent)),
                PacerRelease::Admitted
            );
            assert_eq!(sent.last(), Some(&expected.to_vec()));
        }
    }

    #[test]
    fn immediate_backpressure_retains_without_charging_bandwidth_or_hops() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4, u8>, u8>::new(SLOW, SLOW_BITRATE, RETRY);
        let mut events = std::vec::Vec::new();
        assert_eq!(
            pacer.offer_tagged_observed(
                &frame(1),
                1,
                InstantMillis(100),
                7,
                |_, _| PacerDelivery::Backpressured,
                |event| events.push(event),
            ),
            PacerOffer::Deferred
        );
        assert_eq!(pacer.queued_len(), 1);
        assert_eq!(pacer.deferred_len(), 1);
        assert_eq!(pacer.oldest_deferred_at(), Some(InstantMillis(100)));
        assert_eq!(pacer.next_release(), Some(InstantMillis(150)));
        assert!(matches!(events.as_slice(), [PacerEvent::Deferred(_)]));

        assert_eq!(
            pacer.release_due_tagged(InstantMillis(149), |_, _| PacerDelivery::Admitted),
            PacerRelease::NotDue
        );
        assert_eq!(
            pacer.release_due_tagged(InstantMillis(150), |_, _| PacerDelivery::Admitted),
            PacerRelease::Admitted
        );
        assert_eq!(pacer.next_release(), None);

        assert_eq!(
            pacer.offer_tagged(&frame(0), 1, InstantMillis(151), 8, |_, _| {
                PacerDelivery::Admitted
            }),
            PacerOffer::Held(PacerHold::Bandwidth),
            "only the later admission charged bandwidth"
        );
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn a_backpressured_heap_frame_keeps_the_same_allocation_across_retries() {
        let mut pacer = AnnouncePacer::<HeapPacerQueue>::new(SLOW, SLOW_BITRATE, RETRY);
        assert_eq!(
            pacer.offer(&frame(1), 1, InstantMillis(0), |_| {
                PacerDelivery::Backpressured
            }),
            PacerOffer::Deferred
        );
        let mut pointers = std::vec::Vec::new();
        for at in [50, 150, 350] {
            assert_eq!(
                pacer.release_due(InstantMillis(at), |bytes| {
                    pointers.push(bytes.as_ptr());
                    PacerDelivery::Backpressured
                }),
                PacerRelease::Backpressured(PacerBackpressure::Retry)
            );
        }
        assert!(pointers.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn retries_use_exact_exponential_deadlines_and_reset_after_recovery() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4, u8>, u8>::new(SLOW, SLOW_BITRATE, RETRY);
        let mut events = std::vec::Vec::new();
        pacer.offer_tagged_observed(
            &frame(1),
            1,
            InstantMillis(0),
            1,
            |_, _| PacerDelivery::Backpressured,
            |event| events.push(event),
        );
        for (at, next) in [
            (50, 150),
            (150, 350),
            (350, 750),
            (750, 1_550),
            (1_550, 2_550),
        ] {
            assert_eq!(
                pacer.release_due_tagged_observed(
                    InstantMillis(at),
                    |_, _| PacerDelivery::Backpressured,
                    |event| events.push(event),
                ),
                PacerRelease::Backpressured(PacerBackpressure::Retry)
            );
            assert_eq!(pacer.next_release(), Some(InstantMillis(next)));
        }
        assert_eq!(
            pacer.release_due_tagged_observed(
                InstantMillis(2_550),
                |_, _| PacerDelivery::Admitted,
                |event| events.push(event),
            ),
            PacerRelease::Admitted
        );
        assert!(matches!(events.last(), Some(PacerEvent::Recovered(_))));

        assert_eq!(
            pacer.offer_tagged(&frame(2), 1, InstantMillis(3_350), 2, |_, _| {
                PacerDelivery::Backpressured
            }),
            PacerOffer::Deferred
        );
        assert_eq!(pacer.next_release(), Some(InstantMillis(3_400)));
    }

    #[test]
    fn terminal_discard_does_not_charge_bandwidth_and_yields_one_millisecond() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4>>::new(SLOW, SLOW_BITRATE, RETRY);
        pacer.offer(&frame(0), 1, InstantMillis(0), |_| PacerDelivery::Admitted);
        pacer.offer(&frame(1), 1, InstantMillis(1), |_| PacerDelivery::Admitted);
        pacer.offer(&frame(2), 1, InstantMillis(2), |_| PacerDelivery::Admitted);

        assert_eq!(
            pacer.release_due(InstantMillis(800), |_| PacerDelivery::Discarded),
            PacerRelease::Discarded
        );
        assert_eq!(pacer.queued_len(), 1);
        assert_eq!(pacer.next_release(), Some(InstantMillis(801)));
        assert_eq!(
            pacer.release_due(InstantMillis(800), |_| PacerDelivery::Admitted),
            PacerRelease::NotDue
        );
        assert_eq!(
            pacer.release_due(InstantMillis(801), |_| PacerDelivery::Admitted),
            PacerRelease::Admitted
        );
    }

    #[test]
    fn hop_zero_preemption_survives_backpressure_deferral() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4>>::new(SLOW, SLOW_BITRATE, RETRY);
        assert_eq!(
            pacer.offer(&frame(1), 1, InstantMillis(0), |_| PacerDelivery::Admitted),
            PacerOffer::Admitted
        );
        assert_eq!(
            pacer.offer(&frame(0), 0, InstantMillis(1), |_| {
                PacerDelivery::Backpressured
            }),
            PacerOffer::Deferred
        );
        assert_eq!(pacer.next_release(), Some(InstantMillis(51)));
        assert_eq!(
            pacer.release_due(InstantMillis(51), |_| PacerDelivery::Admitted),
            PacerRelease::Admitted
        );
        assert_eq!(
            pacer.offer(&frame(2), 0, InstantMillis(52), |_| PacerDelivery::Admitted),
            PacerOffer::Held(PacerHold::Bandwidth),
            "admission charges the local announce cooldown exactly once"
        );
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn bounded_256_entry_heap_rejects_or_replaces_with_the_correct_victim() {
        let mut pacer =
            AnnouncePacer::<BoundedHeapPacerQueue<256, u16>, u16>::new(SLOW, SLOW_BITRATE, RETRY);
        let mut events = std::vec::Vec::new();
        pacer.offer_tagged(&frame(0), 1, InstantMillis(0), 0, |_, _| {
            PacerDelivery::Admitted
        });
        for metadata in 1..=256u16 {
            assert_eq!(
                pacer.offer_tagged(
                    &frame(metadata as u8),
                    5,
                    InstantMillis(u64::from(metadata)),
                    metadata,
                    |_, _| PacerDelivery::Admitted,
                ),
                PacerOffer::Held(PacerHold::Bandwidth)
            );
        }
        assert_eq!(pacer.queued_len(), 256);
        assert_eq!(
            pacer.offer_tagged_observed(
                &frame(1),
                1,
                InstantMillis(300),
                300,
                |_, _| PacerDelivery::Admitted,
                |event| events.push(event),
            ),
            PacerOffer::Held(PacerHold::Bandwidth)
        );
        assert!(matches!(
            events.as_slice(),
            [PacerEvent::Evicted(PacerEntry { metadata: 1, .. })]
        ));
        assert_eq!(
            pacer.offer_tagged(&frame(9), 9, InstantMillis(301), 301, |_, _| {
                PacerDelivery::Admitted
            }),
            PacerOffer::Rejected(PacerReject::QueueFull)
        );
    }

    #[test]
    fn deferred_entries_keep_original_age_and_expire_after_twenty_four_hours() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4, u8>, u8>::new(SLOW, SLOW_BITRATE, RETRY);
        let mut events = std::vec::Vec::new();
        pacer.offer_tagged(&frame(1), 1, InstantMillis(100), 7, |_, _| {
            PacerDelivery::Backpressured
        });
        pacer.release_due_tagged(InstantMillis(150), |_, _| PacerDelivery::Backpressured);
        let after_lifetime = 100 + QUEUED_ANNOUNCE_LIFE_MS + 1;
        assert_eq!(
            pacer.release_due_tagged_observed(
                InstantMillis(after_lifetime),
                |_, _| PacerDelivery::Admitted,
                |event| events.push(event),
            ),
            PacerRelease::Idle
        );
        assert!(matches!(
            events.as_slice(),
            [PacerEvent::Expired(PacerEntry {
                metadata: 7,
                queued_at: InstantMillis(100),
                deferred_since: Some(InstantMillis(100)),
                ..
            })]
        ));
    }

    #[test]
    fn zero_cap_accepts_but_never_schedules() {
        let mut pacer = AnnouncePacer::<FixedPacerQueue<4>>::new(
            AnnounceBandwidthCap::Limited { cap_per_mille: 0 },
            SLOW_BITRATE,
            RETRY,
        );
        assert_eq!(
            pacer.offer(&frame(0), 1, InstantMillis(0), |_| PacerDelivery::Admitted),
            PacerOffer::Held(PacerHold::Bandwidth)
        );
        assert_eq!(pacer.next_release(), None);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn clearing_a_queue_reports_removed_entries_without_resetting_cadence() {
        let mut pacer = AnnouncePacer::<HeapPacerQueue>::new(SLOW, SLOW_BITRATE, RETRY);
        pacer.offer(&frame(0), 1, InstantMillis(0), |_| PacerDelivery::Admitted);
        pacer.offer(&frame(1), 1, InstantMillis(100), |_| {
            PacerDelivery::Admitted
        });
        pacer.offer(&frame(2), 1, InstantMillis(200), |_| {
            PacerDelivery::Admitted
        });
        assert_eq!(pacer.clear_queue(), 2);
        assert_eq!(pacer.queued_len(), 0);
        assert_eq!(
            pacer.offer(&frame(3), 1, InstantMillis(300), |_| {
                PacerDelivery::Admitted
            }),
            PacerOffer::Held(PacerHold::Bandwidth)
        );
        assert_eq!(pacer.next_release(), Some(InstantMillis(SPACING_MS)));
    }
}
