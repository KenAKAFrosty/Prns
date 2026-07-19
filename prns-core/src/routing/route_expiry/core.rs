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
