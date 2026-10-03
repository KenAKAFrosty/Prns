use embassy_time_queue_utils::queue_generic::ConstGenericQueue;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::task::{Wake, Waker};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct QueueStats {
    pub pending: usize,
    pub peak: usize,
    pub capacity: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct CapacityExceeded {
    capacity: usize,
}

// Only the upstream queue receives these forwarding wakers; its wake retires the registration.
struct TrackedWake {
    original: Waker,
    pending: AtomicBool,
}

impl Wake for TrackedWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.pending.store(false, Ordering::Relaxed);
        self.original.wake_by_ref();
    }
}

pub(super) struct ObservedQueue<const CAPACITY: usize> {
    queue: ConstGenericQueue<CAPACITY>,
    registrations: Vec<Arc<TrackedWake>>,
    peak: usize,
}

impl<const CAPACITY: usize> ObservedQueue<CAPACITY> {
    pub(super) const fn new() -> Self {
        const {
            assert!(CAPACITY > 0);
        }
        Self {
            queue: ConstGenericQueue::new(),
            registrations: Vec::new(),
            peak: 0,
        }
    }

    pub(super) fn schedule_wake(&mut self, at: u64, waker: &Waker) -> Result<(), CapacityExceeded> {
        let tracked = match self
            .registrations
            .iter()
            .find(|entry| entry.original.will_wake(waker))
        {
            Some(entry) => entry.clone(),
            None => {
                if self.registrations.len() == CAPACITY {
                    return Err(CapacityExceeded { capacity: CAPACITY });
                }
                let entry = Arc::new(TrackedWake {
                    original: waker.clone(),
                    pending: AtomicBool::new(true),
                });
                self.registrations.push(entry.clone());
                self.peak = self.peak.max(self.registrations.len());
                entry
            }
        };
        self.queue.schedule_wake(at, &Waker::from(tracked));
        Ok(())
    }

    pub(super) fn next_expiration(&mut self, now: u64) -> u64 {
        let next = self.queue.next_expiration(now);
        self.registrations
            .retain(|entry| entry.pending.load(Ordering::Relaxed));
        next
    }

    pub(super) fn stats(&self) -> QueueStats {
        QueueStats {
            pending: self.registrations.len(),
            peak: self.peak,
            capacity: CAPACITY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct Counter(AtomicUsize);
    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn capacity_refusal_preserves_registered_deadlines_without_early_wakes() {
        let mut queue = ObservedQueue::<2>::new();
        let counters: Vec<_> = (0..3).map(|_| Arc::new(Counter::default())).collect();
        let wakers: Vec<_> = counters.iter().cloned().map(Waker::from).collect();
        assert_eq!(queue.schedule_wake(10, &wakers[0]), Ok(()));
        assert_eq!(queue.schedule_wake(20, &wakers[1]), Ok(()));
        assert_eq!(
            queue.schedule_wake(5, &wakers[2]),
            Err(CapacityExceeded { capacity: 2 })
        );
        assert_eq!(
            queue.stats(),
            QueueStats {
                pending: 2,
                peak: 2,
                capacity: 2
            }
        );
        assert_eq!(queue.next_expiration(9), 10);
        assert!(counters
            .iter()
            .all(|counter| counter.0.load(Ordering::Relaxed) == 0));
        assert_eq!(queue.next_expiration(10), 20);
        assert_eq!(queue.schedule_wake(15, &wakers[2]), Ok(()));
        assert_eq!(queue.next_expiration(20), u64::MAX);
        assert_eq!(
            counters
                .iter()
                .map(|counter| counter.0.load(Ordering::Relaxed))
                .collect::<Vec<_>>(),
            [1, 1, 1]
        );
        assert_eq!(
            queue.stats(),
            QueueStats {
                pending: 0,
                peak: 2,
                capacity: 2
            }
        );
    }

    #[test]
    fn repeated_waiter_uses_one_slot_and_upstream_earliest_deadline_policy() {
        let mut queue = ObservedQueue::<1>::new();
        let counter = Arc::new(Counter::default());
        let waker = Waker::from(counter.clone());
        for at in [20, 30, 10, 40] {
            assert_eq!(queue.schedule_wake(at, &waker), Ok(()));
        }
        assert_eq!(
            queue.stats(),
            QueueStats {
                pending: 1,
                peak: 1,
                capacity: 1
            }
        );
        assert_eq!(queue.next_expiration(9), 10);
        assert_eq!(counter.0.load(Ordering::Relaxed), 0);
        assert_eq!(queue.next_expiration(10), u64::MAX);
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        assert_eq!(queue.schedule_wake(30, &waker), Ok(()));
        assert_eq!(queue.next_expiration(30), u64::MAX);
        assert_eq!(counter.0.load(Ordering::Relaxed), 2);
        drop(queue);
        drop(waker);
        assert_eq!(Arc::strong_count(&counter), 1);
    }
}
