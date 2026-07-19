use super::super::core::RouteExpiryIndex;
use crate::units::InstantMillis;

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
