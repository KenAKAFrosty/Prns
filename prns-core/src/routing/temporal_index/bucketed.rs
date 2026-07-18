use super::{IndexReadiness, LINEAR_CULL_MIN_CANDIDATES};
use crate::units::InstantMillis;
use alloc::vec::Vec;
use core::fmt;
use roaring::RoaringTreemap;

const LINEAR_CULL_DENSITY_DENOMINATOR: u64 = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
struct TemporalBucket(u32);

impl TemporalBucket {
    fn containing<const QUANTUM_MS: u64>(deadline: InstantMillis) -> Option<Self> {
        u32::try_from(deadline.0 / QUANTUM_MS).ok().map(Self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TemporalRow(u32);

impl TemporalRow {
    fn from_usize(row: usize) -> Option<Self> {
        u32::try_from(row).ok().map(Self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TemporalKey(u64);

impl TemporalKey {
    fn new(bucket: TemporalBucket, row: TemporalRow) -> Self {
        Self((u64::from(bucket.0) << 32) | u64::from(row.0))
    }
}

pub(crate) struct HeapTemporalIndex<const QUANTUM_MS: u64> {
    readiness: IndexReadiness,
    keys: RoaringTreemap,
    row_buckets: Vec<Option<TemporalBucket>>,
}

impl<const QUANTUM_MS: u64> Default for HeapTemporalIndex<QUANTUM_MS> {
    fn default() -> Self {
        Self {
            readiness: IndexReadiness::Ready,
            keys: RoaringTreemap::new(),
            row_buckets: Vec::new(),
        }
    }
}

impl<const QUANTUM_MS: u64> fmt::Debug for HeapTemporalIndex<QUANTUM_MS> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeapTemporalIndex")
            .field("quantum_ms", &QUANTUM_MS)
            .field("rows", &self.row_buckets.len())
            .field("indexed", &self.keys.len())
            .finish()
    }
}

impl<const QUANTUM_MS: u64> HeapTemporalIndex<QUANTUM_MS> {
    pub(crate) fn invalid() -> Self {
        Self {
            readiness: IndexReadiness::Invalid,
            ..Self::default()
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.readiness = IndexReadiness::Invalid;
    }

    fn needs_rebuild(&self, row_count: usize) -> bool {
        match self.readiness {
            IndexReadiness::Ready => self.row_buckets.len() != row_count,
            IndexReadiness::Invalid => true,
            IndexReadiness::LinearFallback => false,
        }
    }

    fn use_linear_fallback(&mut self) {
        self.keys.clear();
        self.row_buckets.clear();
        self.readiness = IndexReadiness::LinearFallback;
    }

    fn rebuild<F>(&mut self, row_count: usize, deadline_of: &mut F)
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        self.keys.clear();
        self.row_buckets.clear();
        self.readiness = IndexReadiness::Invalid;
        self.row_buckets.reserve(row_count);

        for row in 0..row_count {
            let Some(row_id) = TemporalRow::from_usize(row) else {
                self.use_linear_fallback();
                return;
            };
            let bucket = match deadline_of(row) {
                Some(deadline) => {
                    let Some(bucket) = TemporalBucket::containing::<QUANTUM_MS>(deadline) else {
                        self.use_linear_fallback();
                        return;
                    };
                    self.keys.insert(TemporalKey::new(bucket, row_id).0);
                    Some(bucket)
                }
                None => None,
            };
            self.row_buckets.push(bucket);
        }
        self.readiness = IndexReadiness::Ready;
    }

    fn prepare<F>(&mut self, row_count: usize, deadline_of: &mut F)
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        if self.needs_rebuild(row_count) {
            self.rebuild(row_count, deadline_of);
        }
    }

    pub(crate) fn insert(&mut self, row: usize, deadline: Option<InstantMillis>) {
        if self.readiness != IndexReadiness::Ready || row != self.row_buckets.len() {
            self.invalidate();
            return;
        }
        let Some(row_id) = TemporalRow::from_usize(row) else {
            self.use_linear_fallback();
            return;
        };
        let bucket = match deadline {
            Some(deadline) => {
                let Some(bucket) = TemporalBucket::containing::<QUANTUM_MS>(deadline) else {
                    self.use_linear_fallback();
                    return;
                };
                self.keys.insert(TemporalKey::new(bucket, row_id).0);
                Some(bucket)
            }
            None => None,
        };
        self.row_buckets.push(bucket);
    }

    pub(crate) fn update(&mut self, row: usize, deadline: Option<InstantMillis>) {
        if self.readiness != IndexReadiness::Ready || row >= self.row_buckets.len() {
            self.invalidate();
            return;
        }
        let Some(row_id) = TemporalRow::from_usize(row) else {
            self.use_linear_fallback();
            return;
        };
        let next = match deadline {
            Some(deadline) => {
                let Some(bucket) = TemporalBucket::containing::<QUANTUM_MS>(deadline) else {
                    self.use_linear_fallback();
                    return;
                };
                Some(bucket)
            }
            None => None,
        };
        let previous = self.row_buckets[row];
        if previous == next {
            return;
        }
        if let Some(previous) = previous {
            self.keys.remove(TemporalKey::new(previous, row_id).0);
        }
        if let Some(next) = next {
            self.keys.insert(TemporalKey::new(next, row_id).0);
        }
        self.row_buckets[row] = next;
    }

    pub(crate) fn swap_remove(&mut self, removed: usize, last: usize) {
        if self.readiness != IndexReadiness::Ready
            || last >= self.row_buckets.len()
            || removed > last
        {
            self.invalidate();
            return;
        }
        let Some(removed_id) = TemporalRow::from_usize(removed) else {
            self.invalidate();
            return;
        };
        if let Some(removed_bucket) = self.row_buckets[removed] {
            self.keys
                .remove(TemporalKey::new(removed_bucket, removed_id).0);
        }

        if removed != last {
            let Some(last_id) = TemporalRow::from_usize(last) else {
                self.invalidate();
                return;
            };
            let moved_bucket = self.row_buckets[last];
            if let Some(moved_bucket) = moved_bucket {
                self.keys.remove(TemporalKey::new(moved_bucket, last_id).0);
                self.keys
                    .insert(TemporalKey::new(moved_bucket, removed_id).0);
            }
            self.row_buckets[removed] = moved_bucket;
        }
        self.row_buckets.pop();
    }

    fn due_candidate_count(&self, now: InstantMillis) -> u64 {
        if self.readiness != IndexReadiness::Ready {
            return u64::MAX;
        }
        let Some(bucket) = TemporalBucket::containing::<QUANTUM_MS>(now) else {
            return self.keys.len();
        };
        self.keys
            .range_cardinality(..=TemporalKey::new(bucket, TemporalRow(u32::MAX)).0)
    }

    pub(crate) fn prefers_linear_cull(&self, row_count: usize, now: InstantMillis) -> bool {
        let candidates = self.due_candidate_count(now);
        candidates == u64::MAX
            || (candidates > LINEAR_CULL_MIN_CANDIDATES
                && candidates.saturating_mul(LINEAR_CULL_DENSITY_DENOMINATOR) > row_count as u64)
    }

    pub(crate) fn earliest_exact<F>(
        &mut self,
        row_count: usize,
        mut deadline_of: F,
    ) -> Option<InstantMillis>
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        self.prepare(row_count, &mut deadline_of);
        if self.readiness == IndexReadiness::LinearFallback {
            return (0..row_count).filter_map(deadline_of).min();
        }
        let (_, rows) = self.keys.bitmaps().next()?;
        rows.iter()
            .filter_map(|row| deadline_of(row as usize))
            .min()
    }

    pub(crate) fn first_due<F>(
        &mut self,
        row_count: usize,
        now: InstantMillis,
        deadline_of: F,
    ) -> Option<usize>
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        self.first_due_matching(row_count, now, deadline_of, |_| true)
    }

    pub(crate) fn first_due_matching<F, P>(
        &mut self,
        row_count: usize,
        now: InstantMillis,
        mut deadline_of: F,
        mut predicate: P,
    ) -> Option<usize>
    where
        F: FnMut(usize) -> Option<InstantMillis>,
        P: FnMut(usize) -> bool,
    {
        self.prepare(row_count, &mut deadline_of);
        if self.readiness == IndexReadiness::LinearFallback {
            return (0..row_count)
                .find(|&row| deadline_of(row).is_some_and(|at| at <= now) && predicate(row));
        }
        let end = TemporalBucket::containing::<QUANTUM_MS>(now)
            .map(|bucket| TemporalKey::new(bucket, TemporalRow(u32::MAX)).0)
            .unwrap_or(u64::MAX);
        self.keys
            .iter()
            .take_while(|key| *key <= end)
            .map(|key| key as u32 as usize)
            .find(|&row| deadline_of(row).is_some_and(|at| at <= now) && predicate(row))
    }

    #[cfg(test)]
    pub(super) fn storage_bytes(&self) -> usize {
        self.keys.len() as usize * core::mem::size_of::<u64>()
            + self.row_buckets.capacity() * core::mem::size_of::<Option<TemporalBucket>>()
    }
}
