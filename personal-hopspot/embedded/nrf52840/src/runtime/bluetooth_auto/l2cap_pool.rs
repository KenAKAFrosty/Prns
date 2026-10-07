use core::cell::UnsafeCell;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

pub(super) struct L2capPool<const MTU: usize, const CAPACITY: usize> {
    buffers: [UnsafeCell<[u8; MTU]>; CAPACITY],
    claimed: [AtomicBool; CAPACITY],
}

// SAFETY: A false -> true AcqRel exchange gives one packet exclusive ownership of
// a fixed slot. That packet releases the slot with Release only after its last
// access. The pool has a static lifetime, so outstanding pointers remain valid.
unsafe impl<const MTU: usize, const CAPACITY: usize> Sync for L2capPool<MTU, CAPACITY> {}

impl<const MTU: usize, const CAPACITY: usize> L2capPool<MTU, CAPACITY> {
    pub(super) const fn new() -> Self {
        assert!(MTU > 0);
        assert!(CAPACITY > 0);
        Self {
            buffers: [const { UnsafeCell::new([0; MTU]) }; CAPACITY],
            claimed: [const { AtomicBool::new(false) }; CAPACITY],
        }
    }

    pub(super) fn claim(&'static self) -> Option<NonNull<u8>> {
        for slot in 0..CAPACITY {
            if !self.claimed[slot].swap(true, Ordering::AcqRel) {
                return NonNull::new(self.buffers[slot].get().cast());
            }
        }
        None
    }

    pub(super) fn release(&self, ptr: NonNull<u8>) {
        let base = self.buffers.as_ptr() as usize;
        let slot = (ptr.as_ptr() as usize - base) / core::mem::size_of::<UnsafeCell<[u8; MTU]>>();
        if slot < CAPACITY {
            self.claimed[slot].store(false, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn all_slots_are_distinct_exhaustion_is_bounded_and_release_reuses_only_its_slot() {
        const CAPACITY: usize = 9;
        const MTU: usize = 502;
        let pool = std::boxed::Box::leak(std::boxed::Box::new(L2capPool::<MTU, CAPACITY>::new()));
        let claims: std::vec::Vec<_> = (0..CAPACITY).map(|_| pool.claim().unwrap()).collect();
        for (i, ptr) in claims.iter().enumerate() {
            assert!(!claims[..i].contains(ptr));
        }
        assert_eq!(pool.claim(), None);
        for ptr in &claims {
            pool.release(*ptr);
            assert_eq!(pool.claim(), Some(*ptr));
            assert_eq!(pool.claim(), None);
        }
        for ptr in &claims {
            pool.release(*ptr);
        }
        let again: std::vec::Vec<_> = (0..CAPACITY).map(|_| pool.claim().unwrap()).collect();
        assert_eq!(again, claims);
        assert_eq!(pool.claim(), None);
        for ptr in again {
            pool.release(ptr);
        }
    }

    #[test]
    fn concurrent_packets_never_share_a_slot() {
        const CAPACITY: usize = 4;
        let pool: &'static L2capPool<16, CAPACITY> =
            std::boxed::Box::leak(std::boxed::Box::new(L2capPool::new()));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(CAPACITY + 1));
        let threads: std::vec::Vec<_> = (0..CAPACITY)
            .map(|_| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let ptr = pool.claim().unwrap();
                    let address = ptr.as_ptr() as usize;
                    barrier.wait();
                    barrier.wait();
                    pool.release(ptr);
                    address
                })
            })
            .collect();
        barrier.wait();
        assert_eq!(pool.claim(), None);
        barrier.wait();
        let mut addresses: std::vec::Vec<_> =
            threads.into_iter().map(|t| t.join().unwrap()).collect();
        addresses.sort_unstable();
        addresses.dedup();
        assert_eq!(addresses.len(), CAPACITY);
        let claims: std::vec::Vec<_> = (0..CAPACITY).map(|_| pool.claim().unwrap()).collect();
        assert_eq!(pool.claim(), None);
        for ptr in claims {
            pool.release(ptr);
        }
    }
}
