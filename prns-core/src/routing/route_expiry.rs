use core::fmt;

use crate::units::InstantMillis;

pub const ROUTE_EXPIRY_QUANTUM_MS: u64 = 5 * 60 * 1_000;

pub trait RouteExpiryIndex: Default + Clone + fmt::Debug + PartialEq + Eq {
    const INDEXED: bool;

    fn invalidate(&self);
    fn insert(&self, row: usize, expiry: InstantMillis);
    fn update(&self, row: usize, expiry: InstantMillis);
    fn swap_remove(&self, removed: usize, last: usize);

    fn prefers_linear_cull(&self, row_count: usize, now: InstantMillis) -> bool;

    fn earliest_exact<F>(&self, row_count: usize, expiry_of: F) -> Option<InstantMillis>
    where
        F: FnMut(usize) -> InstantMillis;

    fn first_expired<F>(&self, row_count: usize, now: InstantMillis, expiry_of: F) -> Option<usize>
    where
        F: FnMut(usize) -> InstantMillis;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LinearRouteExpiryIndex;

impl RouteExpiryIndex for LinearRouteExpiryIndex {
    const INDEXED: bool = false;

    fn invalidate(&self) {}

    fn insert(&self, _row: usize, _expiry: InstantMillis) {}

    fn update(&self, _row: usize, _expiry: InstantMillis) {}

    fn swap_remove(&self, _removed: usize, _last: usize) {}

    fn prefers_linear_cull(&self, _row_count: usize, _now: InstantMillis) -> bool {
        true
    }

    fn earliest_exact<F>(&self, row_count: usize, expiry_of: F) -> Option<InstantMillis>
    where
        F: FnMut(usize) -> InstantMillis,
    {
        (0..row_count).map(expiry_of).min()
    }

    fn first_expired<F>(
        &self,
        row_count: usize,
        now: InstantMillis,
        mut expiry_of: F,
    ) -> Option<usize>
    where
        F: FnMut(usize) -> InstantMillis,
    {
        (0..row_count).find(|&row| expiry_of(row) <= now)
    }
}

#[cfg(feature = "std")]
mod roaring_index {
    use alloc::vec::Vec;
    use core::cell::RefCell;
    use core::fmt;

    use roaring::RoaringTreemap;

    use super::{RouteExpiryIndex, ROUTE_EXPIRY_QUANTUM_MS};
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

    struct HeapTemporalIndex<const QUANTUM_MS: u64> {
        readiness: IndexReadiness,
        keys: RoaringTreemap,
        row_buckets: Vec<TemporalBucket>,
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

    impl<const QUANTUM_MS: u64> HeapTemporalIndex<QUANTUM_MS> {
        fn invalid() -> Self {
            Self {
                readiness: IndexReadiness::Invalid,
                ..Self::default()
            }
        }

        fn invalidate(&mut self) {
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

        fn rebuild<F>(&mut self, row_count: usize, expiry_of: &mut F)
        where
            F: FnMut(usize) -> InstantMillis,
        {
            self.keys.clear();
            self.row_buckets.clear();
            self.readiness = IndexReadiness::Invalid;
            self.row_buckets.reserve(row_count);

            for row in 0..row_count {
                let Some(row) = TemporalRow::from_usize(row) else {
                    self.use_linear_fallback();
                    return;
                };
                let Some(bucket) =
                    TemporalBucket::containing::<QUANTUM_MS>(expiry_of(row.0 as usize))
                else {
                    self.use_linear_fallback();
                    return;
                };
                self.keys.insert(TemporalKey::new(bucket, row).0);
                self.row_buckets.push(bucket);
            }
            self.readiness = IndexReadiness::Ready;
        }

        fn insert(&mut self, row: usize, expiry: InstantMillis) {
            if self.readiness != IndexReadiness::Ready || row != self.row_buckets.len() {
                self.invalidate();
                return;
            }
            let (Some(row), Some(bucket)) = (
                TemporalRow::from_usize(row),
                TemporalBucket::containing::<QUANTUM_MS>(expiry),
            ) else {
                self.use_linear_fallback();
                return;
            };
            self.keys.insert(TemporalKey::new(bucket, row).0);
            self.row_buckets.push(bucket);
        }

        fn update(&mut self, row: usize, expiry: InstantMillis) {
            if self.readiness != IndexReadiness::Ready || row >= self.row_buckets.len() {
                self.invalidate();
                return;
            }
            let (Some(row_id), Some(bucket)) = (
                TemporalRow::from_usize(row),
                TemporalBucket::containing::<QUANTUM_MS>(expiry),
            ) else {
                self.use_linear_fallback();
                return;
            };
            let previous = self.row_buckets[row];
            if previous == bucket {
                return;
            }
            self.keys.remove(TemporalKey::new(previous, row_id).0);
            self.keys.insert(TemporalKey::new(bucket, row_id).0);
            self.row_buckets[row] = bucket;
        }

        fn swap_remove(&mut self, removed: usize, last: usize) {
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
            let removed_bucket = self.row_buckets[removed];
            self.keys
                .remove(TemporalKey::new(removed_bucket, removed_id).0);

            if removed != last {
                let Some(last_id) = TemporalRow::from_usize(last) else {
                    self.invalidate();
                    return;
                };
                let moved_bucket = self.row_buckets[last];
                self.keys.remove(TemporalKey::new(moved_bucket, last_id).0);
                self.keys
                    .insert(TemporalKey::new(moved_bucket, removed_id).0);
                self.row_buckets[removed] = moved_bucket;
            }
            self.row_buckets.pop();
        }
    }

    pub struct RoaringRouteExpiryIndex {
        index: RefCell<HeapTemporalIndex<ROUTE_EXPIRY_QUANTUM_MS>>,
    }

    impl Default for RoaringRouteExpiryIndex {
        fn default() -> Self {
            Self {
                index: RefCell::new(HeapTemporalIndex::default()),
            }
        }
    }

    impl Clone for RoaringRouteExpiryIndex {
        fn clone(&self) -> Self {
            Self {
                index: RefCell::new(HeapTemporalIndex::invalid()),
            }
        }
    }

    impl fmt::Debug for RoaringRouteExpiryIndex {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("RoaringRouteExpiryIndex")
        }
    }

    impl PartialEq for RoaringRouteExpiryIndex {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    impl Eq for RoaringRouteExpiryIndex {}

    impl RoaringRouteExpiryIndex {
        fn prepare<F>(&self, row_count: usize, expiry_of: &mut F)
        where
            F: FnMut(usize) -> InstantMillis,
        {
            let needs_rebuild = self.index.borrow().needs_rebuild(row_count);
            if needs_rebuild {
                self.index.borrow_mut().rebuild(row_count, expiry_of);
            }
        }

        fn uses_linear_fallback(&self) -> bool {
            self.index.borrow().readiness == IndexReadiness::LinearFallback
        }

        fn due_candidate_count(&self, now: InstantMillis) -> u64 {
            let index = self.index.borrow();
            if index.readiness != IndexReadiness::Ready {
                return u64::MAX;
            }
            let Some(bucket) = TemporalBucket::containing::<ROUTE_EXPIRY_QUANTUM_MS>(now) else {
                return index.keys.len();
            };
            index
                .keys
                .range_cardinality(..=TemporalKey::new(bucket, TemporalRow(u32::MAX)).0)
        }
    }

    impl RouteExpiryIndex for RoaringRouteExpiryIndex {
        const INDEXED: bool = true;

        fn invalidate(&self) {
            self.index.borrow_mut().invalidate();
        }

        fn insert(&self, row: usize, expiry: InstantMillis) {
            self.index.borrow_mut().insert(row, expiry);
        }

        fn update(&self, row: usize, expiry: InstantMillis) {
            self.index.borrow_mut().update(row, expiry);
        }

        fn swap_remove(&self, removed: usize, last: usize) {
            self.index.borrow_mut().swap_remove(removed, last);
        }

        fn prefers_linear_cull(&self, row_count: usize, now: InstantMillis) -> bool {
            let candidates = self.due_candidate_count(now);
            candidates == u64::MAX
                || (candidates > LINEAR_CULL_MIN_CANDIDATES
                    && candidates.saturating_mul(LINEAR_CULL_DENSITY_DENOMINATOR)
                        > row_count as u64)
        }

        fn earliest_exact<F>(&self, row_count: usize, mut expiry_of: F) -> Option<InstantMillis>
        where
            F: FnMut(usize) -> InstantMillis,
        {
            self.prepare(row_count, &mut expiry_of);
            if self.uses_linear_fallback() {
                return (0..row_count).map(expiry_of).min();
            }

            let index = self.index.borrow();
            let (_, rows) = index.keys.bitmaps().next()?;
            rows.iter().map(|row| expiry_of(row as usize)).min()
        }

        fn first_expired<F>(
            &self,
            row_count: usize,
            now: InstantMillis,
            mut expiry_of: F,
        ) -> Option<usize>
        where
            F: FnMut(usize) -> InstantMillis,
        {
            self.prepare(row_count, &mut expiry_of);
            if self.uses_linear_fallback() {
                return (0..row_count).find(|&row| expiry_of(row) <= now);
            }

            let index = self.index.borrow();
            let (_, rows) = index.keys.bitmaps().next()?;
            rows.iter()
                .map(|row| row as usize)
                .find(|&row| expiry_of(row) <= now)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn deadlines(values: &[u64]) -> Vec<InstantMillis> {
            values.iter().copied().map(InstantMillis).collect()
        }

        #[test]
        fn exact_queries_only_walk_the_first_bucket() {
            let index = RoaringRouteExpiryIndex::default();
            let values = deadlines(&[599_000, 301_000, 300_500, 900_000]);
            assert_eq!(
                index.earliest_exact(values.len(), |row| values[row]),
                Some(InstantMillis(300_500))
            );
            assert_eq!(
                index.first_expired(values.len(), InstantMillis(300_750), |row| values[row]),
                Some(2)
            );
            assert_eq!(
                index.first_expired(values.len(), InstantMillis(300_499), |row| values[row]),
                None
            );
        }

        #[test]
        fn updates_and_swap_removals_keep_row_ids_aligned() {
            let index = RoaringRouteExpiryIndex::default();
            let mut values = deadlines(&[300_000, 600_000, 900_000]);
            for (row, expiry) in values.iter().copied().enumerate() {
                index.insert(row, expiry);
            }

            values[0] = InstantMillis(1_200_000);
            index.update(0, values[0]);
            assert_eq!(
                index.earliest_exact(values.len(), |row| values[row]),
                Some(InstantMillis(600_000))
            );

            let last = values.len() - 1;
            values.swap_remove(1);
            index.swap_remove(1, last);
            assert_eq!(
                index.earliest_exact(values.len(), |row| values[row]),
                Some(InstantMillis(900_000))
            );
        }

        #[test]
        fn invalidation_rebuilds_and_unrepresentable_deadlines_fall_back_to_exact_scans() {
            let index = RoaringRouteExpiryIndex::default();
            let mut values = deadlines(&[300_000, 600_000]);
            for (row, expiry) in values.iter().copied().enumerate() {
                index.insert(row, expiry);
            }
            values[1] = InstantMillis(1);
            index.invalidate();
            assert_eq!(
                index.earliest_exact(values.len(), |row| values[row]),
                Some(InstantMillis(1))
            );

            values[0] = InstantMillis((u64::from(u32::MAX) + 1) * ROUTE_EXPIRY_QUANTUM_MS);
            index.invalidate();
            assert_eq!(
                index.earliest_exact(values.len(), |row| values[row]),
                Some(InstantMillis(1))
            );
        }

        #[test]
        fn dense_due_sets_choose_the_scan_path() {
            let index = RoaringRouteExpiryIndex::default();
            let values = vec![InstantMillis(1); 5_000];
            for (row, expiry) in values.iter().copied().enumerate() {
                index.insert(row, expiry);
            }
            assert!(index.prefers_linear_cull(values.len(), InstantMillis(1)));
            index.invalidate();
            assert!(index.prefers_linear_cull(values.len(), InstantMillis(0)));
        }

        #[test]
        fn randomized_row_mutations_preserve_exact_deadlines() {
            let index = RoaringRouteExpiryIndex::default();
            let mut values = (0..1_000u64)
                .map(|row| InstantMillis(mix(row) % 2_000_000_000))
                .collect::<Vec<_>>();
            for (row, expiry) in values.iter().copied().enumerate() {
                index.insert(row, expiry);
            }

            for step in 0..10_000u64 {
                let mixed = mix(step + 10_000);
                match mixed % 4 {
                    0 if !values.is_empty() => {
                        let row = mixed as usize % values.len();
                        values[row] = InstantMillis(mix(mixed) % 2_000_000_000);
                        index.update(row, values[row]);
                    }
                    1 if values.len() > 1 => {
                        let row = mixed as usize % values.len();
                        let last = values.len() - 1;
                        values.swap_remove(row);
                        index.swap_remove(row, last);
                    }
                    2 => {
                        let expiry = InstantMillis(mix(mixed) % 2_000_000_000);
                        index.insert(values.len(), expiry);
                        values.push(expiry);
                    }
                    3 => index.invalidate(),
                    _ => {}
                }

                let expected = values.iter().copied().min();
                assert_eq!(
                    index.earliest_exact(values.len(), |row| values[row]),
                    expected
                );
                let now = InstantMillis(mix(mixed + 1) % 2_000_000_000);
                let candidate = index.first_expired(values.len(), now, |row| values[row]);
                assert_eq!(
                    candidate.is_some(),
                    values.iter().any(|expiry| *expiry <= now)
                );
                if let Some(row) = candidate {
                    assert!(values[row] <= now);
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
}

#[cfg(feature = "std")]
pub use roaring_index::RoaringRouteExpiryIndex;
