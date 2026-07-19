use core::cell::RefCell;
use core::fmt;

use super::super::core::{RouteExpiryIndex, ROUTE_EXPIRY_QUANTUM_MS};
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
