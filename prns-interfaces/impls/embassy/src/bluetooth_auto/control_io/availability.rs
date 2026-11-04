use core::cell::RefCell;
use core::future::poll_fn;
use core::task::Poll;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::waitqueue::MultiWakerRegistration;

/// Bounded broadcast waiters for a pool whose availability is stored separately.
///
/// Publish availability before calling `notify_all`. Checking the predicate and
/// registering a waiter under the same lock as notification avoids a lost wake.
/// The predicate must not call back into this object. Completion is a hint to
/// retry admission, not a reserved permit or a FIFO fairness guarantee.
pub struct AvailabilityWaiters<M: RawMutex, const WAITERS: usize> {
    waiters: Mutex<M, RefCell<MultiWakerRegistration<WAITERS>>>,
}

impl<M: RawMutex, const WAITERS: usize> Default for AvailabilityWaiters<M, WAITERS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: RawMutex, const WAITERS: usize> AvailabilityWaiters<M, WAITERS> {
    pub const fn new() -> Self {
        assert!(WAITERS > 0);
        Self {
            waiters: Mutex::new(RefCell::new(MultiWakerRegistration::new())),
        }
    }

    pub fn notify_all(&self) {
        self.waiters.lock(|waiters| waiters.borrow_mut().wake());
    }

    pub async fn wait(&self, available: impl Fn() -> bool) {
        poll_fn(|cx| {
            self.waiters.lock(|waiters| {
                if available() {
                    Poll::Ready(())
                } else {
                    // Canceled waits may leave a stale waker. A full registration
                    // list wakes everyone to re-register, rather than losing one.
                    waiters.borrow_mut().register(cx.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::Future;
    use core::pin::pin;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use core::task::{Context, Waker};
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use std::sync::{Arc, Barrier};
    use std::task::Wake;

    #[derive(Default)]
    struct WakeCount(AtomicUsize);

    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn ready_state_is_observed_even_when_notification_precedes_wait() {
        let waiters = AvailabilityWaiters::<CriticalSectionRawMutex, 2>::new();
        waiters.notify_all();
        let mut waiting = pin!(waiters.wait(|| true));
        assert!(waiting
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_ready());
    }

    #[test]
    fn canceled_and_duplicate_registrations_do_not_lose_live_waiters() {
        let waiters = AvailabilityWaiters::<CriticalSectionRawMutex, 2>::new();
        let available = AtomicBool::new(false);
        let counts: [_; 3] = core::array::from_fn(|_| Arc::new(WakeCount::default()));
        let wakers = counts
            .each_ref()
            .map(|counter| Waker::from(Arc::clone(counter)));
        {
            let mut canceled = pin!(waiters.wait(|| available.load(Ordering::Acquire)));
            for _ in 0..8 {
                assert!(canceled
                    .as_mut()
                    .poll(&mut Context::from_waker(&wakers[0]))
                    .is_pending());
            }
        }
        let mut first = pin!(waiters.wait(|| available.load(Ordering::Acquire)));
        assert!(first
            .as_mut()
            .poll(&mut Context::from_waker(&wakers[1]))
            .is_pending());
        assert_eq!(counts[0].0.load(Ordering::Relaxed), 0);
        let mut second = pin!(waiters.wait(|| available.load(Ordering::Acquire)));
        assert!(second
            .as_mut()
            .poll(&mut Context::from_waker(&wakers[2]))
            .is_pending());
        assert_eq!(counts[0].0.load(Ordering::Relaxed), 1);
        assert_eq!(counts[1].0.load(Ordering::Relaxed), 1);
        available.store(true, Ordering::Release);
        waiters.notify_all();
        assert_eq!(counts[2].0.load(Ordering::Relaxed), 1);
        assert!(first
            .as_mut()
            .poll(&mut Context::from_waker(&wakers[1]))
            .is_ready());
        assert!(second
            .as_mut()
            .poll(&mut Context::from_waker(&wakers[2]))
            .is_ready());
    }

    #[test]
    fn releasing_capacity_during_registration_cannot_lose_its_wakeup() {
        let waiters = Arc::new(AvailabilityWaiters::<CriticalSectionRawMutex, 2>::new());
        let available = Arc::new(AtomicBool::new(false));
        let predicate_observed_full = Arc::new(Barrier::new(2));
        let capacity_released = Arc::new(Barrier::new(2));
        let release = {
            let waiters = Arc::clone(&waiters);
            let available = Arc::clone(&available);
            let predicate_observed_full = Arc::clone(&predicate_observed_full);
            let capacity_released = Arc::clone(&capacity_released);
            std::thread::spawn(move || {
                predicate_observed_full.wait();
                available.store(true, Ordering::Release);
                capacity_released.wait();
                waiters.notify_all();
            })
        };
        let first_poll = AtomicBool::new(true);
        let count = Arc::new(WakeCount::default());
        let waker = Waker::from(Arc::clone(&count));
        let mut waiting = pin!(waiters.wait(|| {
            let ready = available.load(Ordering::Acquire);
            if first_poll.swap(false, Ordering::Relaxed) {
                assert!(!ready);
                predicate_observed_full.wait();
                capacity_released.wait();
            }
            ready
        }));
        assert!(waiting
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());
        assert!(release.join().is_ok());
        assert_eq!(count.0.load(Ordering::Relaxed), 1);
        assert!(waiting
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_ready());
    }
}
