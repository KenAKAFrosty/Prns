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
        mut deadline_of: F,
    ) -> Option<usize>
    where
        F: FnMut(usize) -> Option<InstantMillis>,
    {
        self.prepare(row_count, &mut deadline_of);
        if self.readiness == IndexReadiness::LinearFallback {
            return (0..row_count).find(|&row| deadline_of(row).is_some_and(|at| at <= now));
        }
        let (_, rows) = self.keys.bitmaps().next()?;
        rows.iter()
            .map(|row| row as usize)
            .find(|&row| deadline_of(row).is_some_and(|at| at <= now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn mix(mut value: u64) -> u64 {
        value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }
}
