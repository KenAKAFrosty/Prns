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
    use core::cell::RefCell;
    use core::fmt;

    use super::{RouteExpiryIndex, ROUTE_EXPIRY_QUANTUM_MS};
    use crate::routing::temporal_index::HeapTemporalIndex;
    use crate::units::InstantMillis;

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

    impl RouteExpiryIndex for RoaringRouteExpiryIndex {
        const INDEXED: bool = true;

        fn invalidate(&self) {
            self.index.borrow_mut().invalidate();
        }

        fn insert(&self, row: usize, expiry: InstantMillis) {
            self.index.borrow_mut().insert(row, Some(expiry));
        }

        fn update(&self, row: usize, expiry: InstantMillis) {
            self.index.borrow_mut().update(row, Some(expiry));
        }

        fn swap_remove(&self, removed: usize, last: usize) {
            self.index.borrow_mut().swap_remove(removed, last);
        }

        fn prefers_linear_cull(&self, row_count: usize, now: InstantMillis) -> bool {
            self.index.borrow().prefers_linear_cull(row_count, now)
        }

        fn earliest_exact<F>(&self, row_count: usize, mut expiry_of: F) -> Option<InstantMillis>
        where
            F: FnMut(usize) -> InstantMillis,
        {
            self.index
                .borrow_mut()
                .earliest_exact(row_count, |row| Some(expiry_of(row)))
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
            self.index
                .borrow_mut()
                .first_due(row_count, now, |row| Some(expiry_of(row)))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn route_adapter_preserves_exact_queries_and_lazy_rebuilds() {
            let index = RoaringRouteExpiryIndex::default();
            let mut values = vec![
                InstantMillis(599_000),
                InstantMillis(301_000),
                InstantMillis(300_500),
                InstantMillis(900_000),
            ];
            for (row, expiry) in values.iter().copied().enumerate() {
                index.insert(row, expiry);
            }
            assert_eq!(
                index.earliest_exact(values.len(), |row| values[row]),
                Some(InstantMillis(300_500))
            );
            values[2] = InstantMillis(1_200_000);
            index.invalidate();
            assert_eq!(
                index.earliest_exact(values.len(), |row| values[row]),
                Some(InstantMillis(301_000))
            );
        }
    }
}

#[cfg(feature = "std")]
pub use roaring_index::RoaringRouteExpiryIndex;
