//! Zero-initialized ownership flags for fixed packet pools. Keeping a pool's empty state
//! all-zero lets embedded linkers place its zero-filled payload storage in `.bss`.

use portable_atomic::{AtomicBool, Ordering};

pub struct SlotClaims<const CAPACITY: usize> {
    claimed: [AtomicBool; CAPACITY],
}

impl<const CAPACITY: usize> Default for SlotClaims<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAPACITY: usize> SlotClaims<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            claimed: [const { AtomicBool::new(false) }; CAPACITY],
        }
    }

    /// Claim one exclusive index until its owner releases it. No fairness is promised.
    pub fn claim(&self) -> Option<usize> {
        self.claimed.iter().position(|claimed| {
            claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        })
    }

    /// The caller must own this index and release it exactly once, after finishing all
    /// payload access. A later claimant acquires those writes through the ownership flag.
    pub fn release(&self, index: usize) {
        self.claimed[index].store(false, Ordering::Release);
    }

    /// An observation for a separate wait queue, not a reservation.
    pub fn any_available(&self) -> bool {
        self.claimed
            .iter()
            .any(|claimed| !claimed.load(Ordering::Acquire))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn check_capacity<const N: usize>() {
        let claims = SlotClaims::<N>::new();
        assert!(claims
            .claimed
            .iter()
            .all(|flag| !flag.load(Ordering::Relaxed)));
        assert_eq!(core::mem::size_of_val(&claims), N);
        for index in 0..N {
            assert!(claims.any_available());
            assert_eq!(claims.claim(), Some(index));
        }
        assert!(!claims.any_available());
        assert_eq!(claims.claim(), None);
        for index in 0..N {
            claims.release(index);
            assert!(claims.any_available());
            assert_eq!(claims.claim(), Some(index));
            assert_eq!(claims.claim(), None);
        }
        for index in 0..N {
            claims.release(index);
        }
        assert!(claims.any_available());
    }

    #[test]
    fn both_native_pool_capacities_start_empty_exhaust_and_reuse_exact_slots() {
        check_capacity::<4>();
        check_capacity::<9>();
    }

    #[test]
    fn an_empty_pool_never_admits_a_slot() {
        let claims = SlotClaims::<0>::new();
        assert_eq!(claims.claim(), None);
        assert!(!claims.any_available());
    }

    #[test]
    fn concurrent_claimants_cannot_share_or_exceed_the_pool() {
        let claims = Arc::new(SlotClaims::<4>::new());
        let start = Arc::new(Barrier::new(8));
        let threads: [_; 8] = core::array::from_fn(|_| {
            let claims = Arc::clone(&claims);
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                claims.claim()
            })
        });
        let mut admitted = [false; 4];
        for thread in threads {
            if let Some(index) = thread.join().unwrap() {
                assert!(!admitted[index]);
                admitted[index] = true;
            }
        }
        assert_eq!(admitted, [true; 4]);
        assert!(!claims.any_available());
        for index in 0..4 {
            claims.release(index);
        }
        assert!(claims.any_available());
    }
}
