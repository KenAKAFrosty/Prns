use alloc::vec::Vec;
use core::fmt;

use roaring::RoaringTreemap;

use crate::units::InstantMillis;

const LINEAR_CULL_MIN_CANDIDATES: u64 = 4_096;
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
        deadline_of(row as usize).unwrap()
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
        self.heap.first().and_then(|row| deadline_of(*row as usize))
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
        self.heap.first().and_then(|row| deadline_of(*row as usize))
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
        self.heap
            .first()
            .map(|row| *row as usize)
            .filter(|row| deadline_of(*row).is_some_and(|at| at <= now))
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
        if self
            .heap
            .first()
            .is_none_or(|row| deadline_of(*row as usize).is_none_or(|at| at > now))
        {
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
mod tests {
    use super::*;
    use core::hint::black_box;
    use core::mem::size_of;
    use std::time::Instant;

    fn deadlines(values: &[Option<u64>]) -> Vec<Option<InstantMillis>> {
        values
            .iter()
            .map(|value| value.map(InstantMillis))
            .collect()
    }

    #[test]
    fn exact_queries_only_walk_the_first_populated_bucket() {
        let mut index = HeapTemporalIndex::<300_000>::default();
        let values = deadlines(&[
            Some(599_000),
            None,
            Some(301_000),
            Some(300_500),
            Some(900_000),
        ]);
        for (row, deadline) in values.iter().copied().enumerate() {
            index.insert(row, deadline);
        }
        assert_eq!(
            index.earliest_exact(values.len(), |row| values[row]),
            Some(InstantMillis(300_500))
        );
        assert_eq!(
            index.first_due(values.len(), InstantMillis(300_750), |row| values[row]),
            Some(3)
        );
        assert_eq!(
            index.first_due(values.len(), InstantMillis(300_499), |row| values[row]),
            None
        );
    }

    #[test]
    fn optional_updates_and_swap_removals_keep_row_ids_aligned() {
        let mut index = HeapTemporalIndex::<1_000>::default();
        let mut values = deadlines(&[Some(1_000), None, Some(3_000), Some(4_000)]);
        for (row, deadline) in values.iter().copied().enumerate() {
            index.insert(row, deadline);
        }

        values[0] = None;
        index.update(0, None);
        values[1] = Some(InstantMillis(2_000));
        index.update(1, values[1]);
        assert_eq!(
            index.earliest_exact(values.len(), |row| values[row]),
            Some(InstantMillis(2_000))
        );

        let last = values.len() - 1;
        values.swap_remove(1);
        index.swap_remove(1, last);
        assert_eq!(
            index.earliest_exact(values.len(), |row| values[row]),
            Some(InstantMillis(3_000))
        );
    }

    #[test]
    fn invalidation_rebuilds_and_unrepresentable_deadlines_scan_exactly() {
        let mut index = HeapTemporalIndex::<1>::default();
        let mut values = deadlines(&[Some(100), Some(200)]);
        for (row, deadline) in values.iter().copied().enumerate() {
            index.insert(row, deadline);
        }
        values[1] = Some(InstantMillis(1));
        index.invalidate();
        assert_eq!(
            index.earliest_exact(values.len(), |row| values[row]),
            Some(InstantMillis(1))
        );

        values[0] = Some(InstantMillis(u64::from(u32::MAX) + 1));
        index.invalidate();
        assert_eq!(
            index.earliest_exact(values.len(), |row| values[row]),
            Some(InstantMillis(1))
        );
    }

    #[test]
    fn dense_due_sets_choose_the_scan_path() {
        let mut index = HeapTemporalIndex::<1_000>::default();
        let values = vec![Some(InstantMillis(1)); 5_000];
        for (row, deadline) in values.iter().copied().enumerate() {
            index.insert(row, deadline);
        }
        assert!(index.prefers_linear_cull(values.len(), InstantMillis(1)));
        index.invalidate();
        assert!(index.prefers_linear_cull(values.len(), InstantMillis(0)));
    }

    #[test]
    fn matching_queries_continue_across_due_buckets() {
        let mut index = HeapTemporalIndex::<1_000>::default();
        let values = deadlines(&[Some(100), Some(1_100), Some(2_100)]);
        for (row, deadline) in values.iter().copied().enumerate() {
            index.insert(row, deadline);
        }
        assert_eq!(
            index.first_due_matching(
                values.len(),
                InstantMillis(2_500),
                |row| values[row],
                |row| row == 2,
            ),
            Some(2)
        );
    }

    #[test]
    fn randomized_mutations_match_a_linear_oracle() {
        let mut index = HeapTemporalIndex::<1_000>::default();
        let mut values = (0..1_000u64)
            .map(|row| (row % 5 != 0).then(|| InstantMillis(mix(row) % 2_000_000_000)))
            .collect::<Vec<_>>();
        for (row, deadline) in values.iter().copied().enumerate() {
            index.insert(row, deadline);
        }

        for step in 0..10_000u64 {
            let mixed = mix(step + 10_000);
            match mixed % 5 {
                0 if !values.is_empty() => {
                    let row = mixed as usize % values.len();
                    values[row] =
                        (mixed & 8 != 0).then(|| InstantMillis(mix(mixed) % 2_000_000_000));
                    index.update(row, values[row]);
                }
                1 if values.len() > 1 => {
                    let row = mixed as usize % values.len();
                    let last = values.len() - 1;
                    values.swap_remove(row);
                    index.swap_remove(row, last);
                }
                2 => {
                    let deadline =
                        (mixed & 16 != 0).then(|| InstantMillis(mix(mixed) % 2_000_000_000));
                    index.insert(values.len(), deadline);
                    values.push(deadline);
                }
                3 => index.invalidate(),
                _ => {}
            }

            assert_eq!(
                index.earliest_exact(values.len(), |row| values[row]),
                values.iter().flatten().copied().min()
            );
            let now = InstantMillis(mix(mixed + 1) % 2_000_000_000);
            let candidate = index.first_due(values.len(), now, |row| values[row]);
            assert_eq!(
                candidate.is_some(),
                values.iter().flatten().any(|deadline| *deadline <= now)
            );
            if let Some(row) = candidate {
                assert!(values[row].is_some_and(|deadline| deadline <= now));
            }
        }
    }

    #[test]
    fn randomized_deadline_heap_mutations_match_a_linear_oracle() {
        let mut index = HeapDeadlineIndex::default();
        let mut values = (0..1_000u64)
            .map(|row| (row % 5 != 0).then(|| InstantMillis(mix(row) % 2_000_000_000)))
            .collect::<Vec<_>>();
        for (row, deadline) in values.iter().copied().enumerate() {
            index.insert(row, deadline, |row| values[row]);
        }

        for step in 0..10_000u64 {
            let mixed = mix(step + 30_000);
            match mixed % 4 {
                0 if !values.is_empty() => {
                    let row = mixed as usize % values.len();
                    values[row] =
                        (mixed & 8 != 0).then(|| InstantMillis(mix(mixed) % 2_000_000_000));
                    index.update(row, values[row], |row| values[row]);
                }
                1 if values.len() > 1 => {
                    let row = mixed as usize % values.len();
                    let last = values.len() - 1;
                    index.swap_remove(row, last, |row| values[row]);
                    values.swap_remove(row);
                }
                2 => {
                    let deadline =
                        (mixed & 16 != 0).then(|| InstantMillis(mix(mixed) % 2_000_000_000));
                    values.push(deadline);
                    index.insert(values.len() - 1, deadline, |row| values[row]);
                }
                _ => index.invalidate(),
            }

            assert_eq!(
                index.earliest_exact(values.len(), |row| values[row]),
                values.iter().flatten().copied().min()
            );
            let now = InstantMillis(mix(mixed + 1) % 2_000_000_000);
            let candidate = index.first_due(values.len(), now, |row| values[row]);
            assert_eq!(
                candidate.is_some(),
                values.iter().flatten().any(|deadline| *deadline <= now)
            );
            if let Some(row) = candidate {
                assert!(values[row].is_some_and(|deadline| deadline <= now));
            }
            for (position, row) in index.heap.iter().copied().enumerate() {
                assert_eq!(index.positions[row as usize], position as u32);
                if position > 0 {
                    let parent = index.heap[(position - 1) / 2] as usize;
                    assert!(values[parent] <= values[row as usize]);
                }
            }
        }
    }

    fn profile_values(rows: usize, horizon_ms: u64) -> Vec<Option<InstantMillis>> {
        (0..rows)
            .map(|row| {
                let mixed = mix(row as u64);
                Some(InstantMillis(1_000_000 + mixed % horizon_ms))
            })
            .collect()
    }

    fn profile_roaring<const QUANTUM_MS: u64>(
        label: &str,
        rows: usize,
        horizon_ms: u64,
        query_iterations: usize,
        mutation_iterations: usize,
    ) {
        let mut values = profile_values(rows, horizon_ms);
        let build_started = Instant::now();
        let mut index = HeapTemporalIndex::<QUANTUM_MS>::default();
        for (row, deadline) in values.iter().copied().enumerate() {
            index.insert(row, deadline);
        }
        let build = build_started.elapsed();

        let query_started = Instant::now();
        for _ in 0..query_iterations {
            black_box(index.earliest_exact(values.len(), |row| values[row]));
        }
        let query = query_started.elapsed().as_nanos() as f64 / query_iterations as f64;

        let update_started = Instant::now();
        for step in 0..mutation_iterations {
            let row = mix(step as u64) as usize % values.len();
            let previous = values[row].unwrap();
            let bucket = previous.0 / QUANTUM_MS;
            let deadline = InstantMillis(
                bucket
                    .saturating_add((step & 1) as u64)
                    .saturating_mul(QUANTUM_MS)
                    .saturating_add(QUANTUM_MS / 2),
            );
            values[row] = Some(deadline);
            index.update(row, Some(deadline));
        }
        let update = update_started.elapsed().as_nanos() as f64 / mutation_iterations as f64;

        let churn_started = Instant::now();
        for step in 0..mutation_iterations {
            let removed = mix((step + mutation_iterations) as u64) as usize % values.len();
            let last = values.len() - 1;
            values.swap_remove(removed);
            index.swap_remove(removed, last);
            let deadline = Some(InstantMillis(
                1_000_000 + mix((step + rows) as u64) % horizon_ms,
            ));
            index.insert(values.len(), deadline);
            values.push(deadline);
        }
        let churn = churn_started.elapsed().as_nanos() as f64 / mutation_iterations as f64;
        let storage = index.keys.len() as usize * size_of::<u64>()
            + index.row_buckets.capacity() * size_of::<Option<TemporalBucket>>();

        eprintln!(
            "{label} rows={rows} q={QUANTUM_MS} build_ms={:.3} exact_ns={query:.1} update_ns={update:.1} churn_ns={churn:.1} bytes_upper={storage}",
            build.as_secs_f64() * 1_000.0,
        );
    }

    fn profile_heap(
        label: &str,
        rows: usize,
        horizon_ms: u64,
        query_iterations: usize,
        mutation_iterations: usize,
    ) {
        let mut values = profile_values(rows, horizon_ms);
        let build_started = Instant::now();
        let mut index = HeapDeadlineIndex::invalid();
        black_box(index.earliest_exact(values.len(), |row| values[row]));
        let build = build_started.elapsed();

        let query_started = Instant::now();
        for _ in 0..query_iterations {
            black_box(index.earliest_exact(values.len(), |row| values[row]));
        }
        let query = query_started.elapsed().as_nanos() as f64 / query_iterations as f64;

        let update_started = Instant::now();
        for step in 0..mutation_iterations {
            let row = mix(step as u64) as usize % values.len();
            let previous = values[row];
            let deadline = InstantMillis(previous.unwrap().0.saturating_add(if step & 1 == 0 {
                1
            } else {
                horizon_ms / 2
            }));
            values[row] = Some(deadline);
            index.update(row, Some(deadline), |row| values[row]);
        }
        let update = update_started.elapsed().as_nanos() as f64 / mutation_iterations as f64;

        let churn_started = Instant::now();
        for step in 0..mutation_iterations {
            let removed = mix((step + mutation_iterations) as u64) as usize % values.len();
            let last = values.len() - 1;
            index.swap_remove(removed, last, |row| values[row]);
            values.swap_remove(removed);
            let deadline = Some(InstantMillis(
                1_000_000 + mix((step + rows) as u64) % horizon_ms,
            ));
            values.push(deadline);
            index.insert(values.len() - 1, deadline, |row| values[row]);
        }
        let churn = churn_started.elapsed().as_nanos() as f64 / mutation_iterations as f64;

        eprintln!(
            "{label} rows={rows} heap build_ms={:.3} exact_ns={query:.1} update_ns={update:.1} churn_ns={churn:.1} bytes={}",
            build.as_secs_f64() * 1_000.0,
            index.storage_bytes(),
        );
    }

    fn profile_family(label: &str, horizon_ms: u64, quanta: [u64; 3]) {
        for rows in [10_000, 100_000, 1_000_000] {
            let query_iterations = if rows < 100_000 { 2_000 } else { 200 };
            let mutation_iterations = if rows < 1_000_000 { 20_000 } else { 50_000 };
            match quanta {
                [100, 250, 1_000] => {
                    profile_roaring::<100>(
                        label,
                        rows,
                        horizon_ms,
                        query_iterations,
                        mutation_iterations,
                    );
                    profile_roaring::<250>(
                        label,
                        rows,
                        horizon_ms,
                        query_iterations,
                        mutation_iterations,
                    );
                    profile_roaring::<1_000>(
                        label,
                        rows,
                        horizon_ms,
                        query_iterations,
                        mutation_iterations,
                    );
                }
                [1_000, 5_000, 30_000] => {
                    profile_roaring::<1_000>(
                        label,
                        rows,
                        horizon_ms,
                        query_iterations,
                        mutation_iterations,
                    );
                    profile_roaring::<5_000>(
                        label,
                        rows,
                        horizon_ms,
                        query_iterations,
                        mutation_iterations,
                    );
                    profile_roaring::<30_000>(
                        label,
                        rows,
                        horizon_ms,
                        query_iterations,
                        mutation_iterations,
                    );
                }
                _ => unreachable!(),
            }
            profile_heap(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
        }
    }

    #[test]
    #[ignore]
    fn profile_tranche_one_candidates() {
        profile_family("reverse", 480_000, [1_000, 5_000, 30_000]);
        profile_family("transported", 900_000, [1_000, 5_000, 30_000]);
        profile_family("local", 60_000, [100, 250, 1_000]);
    }

    fn mix(mut value: u64) -> u64 {
        value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }
}
