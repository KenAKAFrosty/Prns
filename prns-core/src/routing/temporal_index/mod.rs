use alloc::vec::Vec;
use core::fmt;

use roaring::RoaringTreemap;

use crate::units::InstantMillis;

const LINEAR_CULL_MIN_CANDIDATES: u64 = 4_096;
const LINEAR_CULL_DENSITY_DENOMINATOR: u64 = 4;
const HEAP_LINEAR_CULL_DENSITY_DENOMINATOR: usize = 25;

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum IndexReadiness {
    Ready,
    Invalid,
    LinearFallback,
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
}

pub(crate) struct HeapDeadlineIndex {
    readiness: IndexReadiness,
    heap: Vec<u32>,
    positions: Vec<u32>,
}

impl Default for HeapDeadlineIndex {
    fn default() -> Self {
        Self {
            readiness: IndexReadiness::Ready,
            heap: Vec::new(),
            positions: Vec::new(),
        }
    }
}

impl fmt::Debug for HeapDeadlineIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeapDeadlineIndex")
            .field("rows", &self.positions.len())
            .field("indexed", &self.heap.len())
            .finish()
    }
}

impl HeapDeadlineIndex {
    const ABSENT: u32 = u32::MAX;

    #[cfg(test)]
    pub(crate) fn invalid() -> Self {
        Self {
            readiness: IndexReadiness::Invalid,
            ..Self::default()
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.readiness = IndexReadiness::Invalid;
    }

    fn use_linear_fallback(&mut self) {
        self.heap.clear();
        self.positions.clear();
        self.readiness = IndexReadiness::LinearFallback;
    }

    fn needs_rebuild(&self, row_count: usize) -> bool {
        match self.readiness {
            IndexReadiness::Ready => self.positions.len() != row_count,
            IndexReadiness::Invalid => true,
            IndexReadiness::LinearFallback => false,
        }
    }

    fn deadline<F>(row: u32, deadline_of: &mut F) -> InstantMillis
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        deadline_of(row as usize).unwrap_or(InstantMillis(u64::MAX))
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.heap.swap(a, b);
        self.positions[self.heap[a] as usize] = a as u32;
        self.positions[self.heap[b] as usize] = b as u32;
    }

    fn sift_up<F>(&mut self, mut position: usize, deadline_of: &mut F)
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        while position > 0 {
            let parent = (position - 1) / 2;
            if Self::deadline(self.heap[parent], deadline_of)
                <= Self::deadline(self.heap[position], deadline_of)
            {
                break;
            }
            self.swap(parent, position);
            position = parent;
        }
    }

    fn sift_down<F>(&mut self, mut position: usize, deadline_of: &mut F)
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        loop {
            let left = position * 2 + 1;
            if left >= self.heap.len() {
                break;
            }
            let right = left + 1;
            let child = if right < self.heap.len()
                && Self::deadline(self.heap[right], deadline_of)
                    < Self::deadline(self.heap[left], deadline_of)
            {
                right
            } else {
                left
            };
            if Self::deadline(self.heap[position], deadline_of)
                <= Self::deadline(self.heap[child], deadline_of)
            {
                break;
            }
            self.swap(position, child);
            position = child;
        }
    }

    fn repair<F>(&mut self, position: usize, deadline_of: &mut F)
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        if position > 0 {
            let parent = (position - 1) / 2;
            if Self::deadline(self.heap[position], deadline_of)
                < Self::deadline(self.heap[parent], deadline_of)
            {
                self.sift_up(position, deadline_of);
                return;
            }
        }
        self.sift_down(position, deadline_of);
    }

    fn rebuild<F>(&mut self, row_count: usize, deadline_of: &mut F)
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        if row_count > Self::ABSENT as usize {
            self.use_linear_fallback();
            return;
        }
        self.heap.clear();
        self.positions.clear();
        self.positions.resize(row_count, Self::ABSENT);
        for row in 0..row_count {
            if deadline_of(row).is_some() {
                self.positions[row] = self.heap.len() as u32;
                self.heap.push(row as u32);
            }
        }
        for position in (0..self.heap.len() / 2).rev() {
            self.sift_down(position, deadline_of);
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

    pub(crate) fn insert<F>(
        &mut self,
        row: usize,
        deadline: Option<InstantMillis>,
        mut deadline_of: F,
    ) where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        if self.readiness != IndexReadiness::Ready || row != self.positions.len() {
            self.invalidate();
            return;
        }
        if row >= Self::ABSENT as usize {
            self.use_linear_fallback();
            return;
        }
        self.positions.push(Self::ABSENT);
        if deadline.is_some() {
            self.positions[row] = self.heap.len() as u32;
            self.heap.push(row as u32);
            self.sift_up(self.heap.len() - 1, &mut deadline_of);
        }
    }

    fn remove<F>(&mut self, row: usize, deadline_of: &mut F)
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        let position = self.positions[row];
        if position == Self::ABSENT {
            return;
        }
        let position = position as usize;
        let last = self.heap.len() - 1;
        self.positions[row] = Self::ABSENT;
        if position == last {
            self.heap.pop();
            return;
        }
        let moved = self.heap[last];
        self.heap[position] = moved;
        self.positions[moved as usize] = position as u32;
        self.heap.pop();
        self.repair(position, deadline_of);
    }

    pub(crate) fn update<F>(
        &mut self,
        row: usize,
        deadline: Option<InstantMillis>,
        mut deadline_of: F,
    ) where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        if self.readiness != IndexReadiness::Ready || row >= self.positions.len() {
            self.invalidate();
            return;
        }
        let position = self.positions[row];
        match (position == Self::ABSENT, deadline.is_some()) {
            (true, true) => {
                self.positions[row] = self.heap.len() as u32;
                self.heap.push(row as u32);
                self.sift_up(self.heap.len() - 1, &mut deadline_of);
            }
            (false, false) => self.remove(row, &mut deadline_of),
            (false, true) => self.repair(position as usize, &mut deadline_of),
            (true, false) => {}
        }
    }

    pub(crate) fn swap_remove<F>(&mut self, removed: usize, last: usize, mut deadline_of: F)
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        if self.readiness != IndexReadiness::Ready || last >= self.positions.len() || removed > last
        {
            self.invalidate();
            return;
        }
        self.remove(removed, &mut deadline_of);
        if removed != last {
            let position = self.positions[last];
            self.positions[removed] = position;
            if position != Self::ABSENT {
                self.heap[position as usize] = removed as u32;
            }
        }
        self.positions.pop();
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
        let &row = self.heap.first()?;
        match deadline_of(row as usize) {
            Some(deadline) => Some(deadline),
            None => {
                self.invalidate();
                (0..row_count).filter_map(deadline_of).min()
            }
        }
    }

    pub(crate) fn eager_earliest_exact<F>(
        &self,
        row_count: usize,
        mut deadline_of: F,
    ) -> Option<InstantMillis>
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        if self.readiness != IndexReadiness::Ready || self.positions.len() != row_count {
            return (0..row_count).filter_map(deadline_of).min();
        }
        let &row = self.heap.first()?;
        deadline_of(row as usize).or_else(|| (0..row_count).filter_map(deadline_of).min())
    }

    pub(crate) fn eager_first_due<F>(
        &self,
        row_count: usize,
        now: InstantMillis,
        mut deadline_of: F,
    ) -> Option<usize>
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        if self.readiness != IndexReadiness::Ready || self.positions.len() != row_count {
            return (0..row_count).find(|&row| deadline_of(row).is_some_and(|at| at <= now));
        }
        let &row = self.heap.first()?;
        let row = row as usize;
        match deadline_of(row) {
            Some(at) if at <= now => Some(row),
            Some(_) => None,
            None => (0..row_count).find(|&row| deadline_of(row).is_some_and(|at| at <= now)),
        }
    }

    fn next_after_subtree(mut position: usize, len: usize) -> Option<usize> {
        while position > 0 {
            if position % 2 == 1 && position + 1 < len {
                return Some(position + 1);
            }
            position = (position - 1) / 2;
        }
        None
    }

    pub(crate) fn prefers_linear_cull<F>(
        &mut self,
        row_count: usize,
        now: InstantMillis,
        mut deadline_of: F,
    ) -> bool
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        self.prepare(row_count, &mut deadline_of);
        if self.readiness != IndexReadiness::Ready {
            return true;
        }
        let mut position = self.heap.first().map(|_| 0);
        let mut candidates = 0usize;
        while let Some(current) = position {
            let row = self.heap[current] as usize;
            let Some(deadline) = deadline_of(row) else {
                self.invalidate();
                return true;
            };
            if deadline <= now {
                candidates += 1;
                if candidates >= LINEAR_CULL_MIN_CANDIDATES as usize
                    && candidates.saturating_mul(HEAP_LINEAR_CULL_DENSITY_DENOMINATOR) >= row_count
                {
                    return true;
                }
                position = if current < self.heap.len() / 2 {
                    Some(current * 2 + 1)
                } else {
                    Self::next_after_subtree(current, self.heap.len())
                };
            } else {
                position = Self::next_after_subtree(current, self.heap.len());
            }
        }
        false
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
        let &first = self.heap.first()?;
        let Some(first_deadline) = deadline_of(first as usize) else {
            self.invalidate();
            return (0..row_count)
                .find(|&row| deadline_of(row).is_some_and(|at| at <= now) && predicate(row));
        };
        if first_deadline > now {
            return None;
        }
        self.heap
            .iter()
            .copied()
            .map(|row| row as usize)
            .find(|&row| deadline_of(row).is_some_and(|at| at <= now) && predicate(row))
    }

    #[cfg(test)]
    fn storage_bytes(&self) -> usize {
        (self.heap.capacity() + self.positions.capacity()) * core::mem::size_of::<u32>()
    }
}

#[cfg(test)]
mod tests;
