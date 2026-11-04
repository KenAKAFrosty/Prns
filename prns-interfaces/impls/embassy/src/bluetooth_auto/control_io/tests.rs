use super::*;
use core::pin::{pin, Pin};
use core::task::{Context, Waker};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use prns_core::interfaces::bluetooth_auto::CloseReason;
use std::boxed::Box;
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::Wake;

use crate::bluetooth_auto::connection_slots::{ConnectionSlotOwners, ConnectionSlotPool};

type Raw = CriticalSectionRawMutex;

fn control() -> Control {
    Control::Close {
        reason: CloseReason::DuplicateLink,
    }
}

fn ticket(completion: &ControlCompletion<Raw>, now: Instant) -> ControlTicket {
    let result = completion.start(&control(), now);
    assert!(result.is_ok());
    let Ok(ticket) = result else {
        unreachable!("fresh operation must start")
    };
    ticket
}

fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

fn owners() -> ConnectionSlotOwners<Raw> {
    let pool = Box::leak(Box::new(ConnectionSlotPool::<Raw, 1>::new()));
    let Some(lease) = pool.try_acquire().ok().flatten() else {
        unreachable!("fresh slot must be available")
    };
    lease.activate()
}

#[test]
fn admission_and_canceled_waits_do_not_complete_or_duplicate_an_operation() {
    let completion = ControlCompletion::<Raw>::new();
    let now = Instant::from_secs(10);
    let first = ticket(&completion, now);
    assert!(!completion.submitted(first));
    assert!(!completion.complete_at(first, true, now));
    {
        let mut waiting = pin!(completion.wait(first));
        assert!(poll_once(waiting.as_mut()).is_pending());
    }
    assert_eq!(
        completion.start(&control(), now + Duration::from_millis(1)),
        Ok(first)
    );
    assert_eq!(completion.admit_at(first, now), Ok(()));
    assert!(completion.submitted(first));
    {
        let mut waiting = pin!(completion.wait(first));
        assert!(poll_once(waiting.as_mut()).is_pending());
    }
    assert_eq!(
        completion.start(&control(), now + Duration::from_millis(2)),
        Ok(first)
    );
    assert_eq!(completion.admit_at(first, now), Err(ControlIoError));
    assert_eq!(first.deadline(), now + CONTROL_OPERATION_TIMEOUT);
    assert!(completion.complete_at(first, true, now));
    assert_eq!(poll_once(pin!(completion.wait(first))), Poll::Ready(Ok(())));
    assert_eq!(completion.acknowledge(first), Ok(()));
    let second = ticket(&completion, now);
    assert_ne!(second, first);
    assert!(!completion.complete_at(first, true, now));
    assert!(!completion.submitted(second));
}

#[test]
fn a_different_message_cannot_resume_or_consume_a_pending_receipt() {
    for admitted in [false, true] {
        let completion = ControlCompletion::<Raw>::new();
        let now = Instant::from_secs(10);
        let first = ticket(&completion, now);
        if admitted {
            assert_eq!(completion.admit_at(first, now), Ok(()));
        }
        let other = Control::Close {
            reason: CloseReason::Incompatible,
        };
        assert_eq!(completion.start(&other, now), Err(ControlIoError));
        assert_eq!(
            poll_once(pin!(completion.wait(first))),
            Poll::Ready(Err(ControlIoError))
        );
        assert!(!completion.complete_at(first, true, now));
    }
}

#[test]
fn timely_completion_remains_consumable_after_cancellation_and_delayed_resume() {
    let completion = ControlCompletion::<Raw>::new();
    let now = Instant::from_secs(10);
    let first = ticket(&completion, now);
    assert_eq!(completion.admit_at(first, now), Ok(()));
    {
        let mut waiting = pin!(completion.wait(first));
        assert!(poll_once(waiting.as_mut()).is_pending());
    }
    assert!(completion.complete_at(first, true, first.deadline() - Duration::from_millis(1)));
    assert_eq!(
        completion.start(&control(), first.deadline() + Duration::from_secs(5)),
        Ok(first)
    );
    assert_eq!(poll_once(pin!(completion.wait(first))), Poll::Ready(Ok(())));
    assert!(!completion.complete_at(first, false, first.deadline()));
    assert_eq!(completion.acknowledge(first), Ok(()));
}

#[test]
fn completion_at_or_after_the_original_deadline_cannot_succeed() {
    for late_by in [Duration::from_millis(0), Duration::from_secs(1)] {
        let completion = ControlCompletion::<Raw>::new();
        let first = ticket(&completion, Instant::from_secs(10));
        assert_eq!(completion.admit_at(first, Instant::from_secs(10)), Ok(()));
        completion.complete_at(first, true, first.deadline() + late_by);
        assert_eq!(
            poll_once(pin!(completion.wait(first))),
            Poll::Ready(Err(ControlIoError))
        );
        assert_eq!(completion.acknowledge(first), Err(ControlIoError));
    }
}

#[test]
fn admission_and_resumption_cannot_extend_an_expired_budget() {
    let completion = ControlCompletion::<Raw>::new();
    let first = ticket(&completion, Instant::from_secs(10));
    assert_eq!(
        completion.admit_at(first, first.deadline()),
        Err(ControlIoError)
    );
    assert_eq!(
        completion.start(&control(), first.deadline()),
        Err(ControlIoError)
    );
    assert_eq!(
        poll_once(pin!(completion.wait(first))),
        Poll::Ready(Err(ControlIoError))
    );
}

#[test]
fn native_failure_is_terminal_and_cannot_be_overwritten_by_success() {
    let completion = ControlCompletion::<Raw>::new();
    let now = Instant::from_secs(10);
    let first = ticket(&completion, now);
    assert_eq!(completion.admit_at(first, now), Ok(()));
    assert!(completion.complete_at(first, false, now));
    assert!(!completion.complete_at(first, true, now));
    assert_eq!(
        poll_once(pin!(completion.wait(first))),
        Poll::Ready(Err(ControlIoError))
    );
    assert_eq!(completion.acknowledge(first), Err(ControlIoError));
}

#[test]
fn retired_and_reused_slots_reject_old_results_and_waiters() {
    let completion = ControlCompletion::<Raw>::new();
    let now = Instant::from_secs(10);
    let old = ticket(&completion, now);
    assert_eq!(completion.admit_at(old, now), Ok(()));
    completion.retire();
    assert_eq!(
        poll_once(pin!(completion.wait(old))),
        Poll::Ready(Err(ControlIoError))
    );
    assert!(!completion.complete_at(old, true, now));
    completion.reset();
    let current = ticket(&completion, now);
    assert_ne!(old, current);
    assert_eq!(completion.admit_at(current, now), Ok(()));
    assert!(!completion.complete_at(old, true, now));
    assert_eq!(completion.acknowledge(old), Err(ControlIoError));
    assert_eq!(
        poll_once(pin!(completion.wait(old))),
        Poll::Ready(Err(ControlIoError))
    );
    assert!(poll_once(pin!(completion.wait(current))).is_pending());
    assert!(completion.complete_at(current, true, now));
    assert_eq!(
        poll_once(pin!(completion.wait(current))),
        Poll::Ready(Ok(()))
    );
}

#[test]
fn sequence_exhaustion_never_reuses_a_ticket_even_after_reset() {
    let completion = ControlCompletion::<Raw>::new();
    completion
        .state
        .lock(|state| state.borrow_mut().sequence = u32::MAX);
    assert_eq!(
        completion.start(&control(), Instant::from_secs(10)),
        Err(ControlIoError)
    );
    completion.reset();
    assert_eq!(
        completion.start(&control(), Instant::from_secs(10)),
        Err(ControlIoError)
    );
}

#[test]
fn rejected_queue_admission_keeps_the_original_operation_and_deadline() {
    let completion = ControlCompletion::<Raw>::new();
    let now = Instant::from_secs(10);
    let first = ticket(&completion, now);
    let attempts = Cell::new(0);
    assert_eq!(
        completion.admit_with(first, now, || {
            attempts.set(attempts.get() + 1);
            false
        }),
        Ok(false)
    );
    assert!(!completion.submitted(first));
    assert_eq!(completion.pending_ticket(), None);
    assert_eq!(
        completion.start(&control(), now + Duration::from_secs(1)),
        Ok(first)
    );
    assert_eq!(
        completion.admit_with(first, now + Duration::from_secs(1), || {
            attempts.set(attempts.get() + 1);
            true
        }),
        Ok(true)
    );
    assert_eq!(attempts.get(), 2);
    assert_eq!(completion.pending_ticket(), Some(first));
    assert_eq!(first.deadline(), now + CONTROL_OPERATION_TIMEOUT);
}

#[test]
fn invalid_expired_and_submitted_tickets_never_execute_the_enqueue_closure() {
    let completion = ControlCompletion::<Raw>::new();
    let now = Instant::from_secs(10);
    let old = ticket(&completion, now);
    completion.reset();
    let current = ticket(&completion, now);
    let attempted = Cell::new(false);
    for (candidate, at) in [(old, now), (current, current.deadline())] {
        assert_eq!(
            completion.admit_with(candidate, at, || {
                attempted.set(true);
                true
            }),
            Err(ControlIoError)
        );
    }
    assert!(!attempted.get());
    assert_eq!(completion.admit_at(current, now), Ok(()));
    assert_eq!(
        completion.admit_with(current, now, || {
            attempted.set(true);
            true
        }),
        Err(ControlIoError)
    );
    assert!(!attempted.get());
}

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn completion_retirement_and_reset_wake_the_registered_receipt_waiter() {
    for action in 0..3 {
        let completion = ControlCompletion::<Raw>::new();
        let now = Instant::from_secs(10);
        let current = ticket(&completion, now);
        assert_eq!(completion.admit_at(current, now), Ok(()));
        let count = Arc::new(WakeCount::default());
        let waker = Waker::from(Arc::clone(&count));
        let mut waiting = pin!(completion.wait(current));
        assert!(waiting
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());
        match action {
            0 => assert!(completion.complete_at(current, true, now)),
            1 => completion.retire(),
            _ => completion.reset(),
        }
        assert_eq!(count.0.load(Ordering::Relaxed), 1);
        assert_eq!(
            poll_once(waiting.as_mut()),
            Poll::Ready(if action == 0 {
                Ok(())
            } else {
                Err(ControlIoError)
            })
        );
    }
}

#[test]
fn notification_before_wait_is_retained_but_surplus_counts_are_not_future_credit() {
    let notifications = NotificationCompletion::<Raw>::new();
    notifications.completed(7);
    assert_eq!(notifications.begin(), Ok(()));
    assert_eq!(notifications.begin(), Err(ControlIoError));
    notifications.completed(0);
    assert!(poll_once(pin!(notifications.wait())).is_pending());
    notifications.completed(4);
    assert_eq!(poll_once(pin!(notifications.wait())), Poll::Ready(()));
    assert_eq!(notifications.begin(), Ok(()));
    {
        let mut canceled = pin!(notifications.wait());
        assert!(poll_once(canceled.as_mut()).is_pending());
    }
    notifications.completed(1);
    assert_eq!(poll_once(pin!(notifications.wait())), Poll::Ready(()));
}

#[test]
fn canceled_admitted_write_retires_before_a_waiting_write_can_enter() {
    let ConnectionSlotOwners {
        worker,
        link: _link,
    } = owners();
    let arbiter = GattWriteArbiter::<Raw>::new(&worker);
    let deadline = Instant::now() + Duration::from_secs(30);
    let second_started = Cell::new(false);
    let mut second = pin!(arbiter.run(deadline, async {
        second_started.set(true);
        Ok(())
    }));
    {
        let mut first = pin!(arbiter.run(
            deadline,
            core::future::pending::<Result<(), ControlIoError>>()
        ));
        assert!(poll_once(first.as_mut()).is_pending());
        assert!(poll_once(second.as_mut()).is_pending());
        assert!(!second_started.get());
    }
    assert!(worker.is_closed());
    assert_eq!(poll_once(second.as_mut()), Poll::Ready(Err(ControlIoError)));
    assert!(!second_started.get());
}

#[test]
fn canceling_a_caller_pinned_operation_retires_before_its_storage_is_dropped() {
    let ConnectionSlotOwners {
        worker,
        link: _link,
    } = owners();
    let arbiter = GattWriteArbiter::<Raw>::new(&worker);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut operation = pin!(core::future::pending::<Result<(), ControlIoError>>());
    {
        let mut waiting = pin!(arbiter.run_pinned(deadline, operation.as_mut()));
        assert!(poll_once(waiting.as_mut()).is_pending());
    }
    assert!(worker.is_closed());
    let admitted = Cell::new(false);
    let mut next = pin!(async {
        admitted.set(true);
        Ok(())
    });
    assert_eq!(
        poll_once(pin!(arbiter.run_pinned(deadline, next.as_mut()))),
        Poll::Ready(Err(ControlIoError))
    );
    assert!(!admitted.get());
}

#[test]
fn cancellation_before_write_admission_does_not_poison_the_active_write() {
    let ConnectionSlotOwners {
        worker,
        link: _link,
    } = owners();
    let arbiter = GattWriteArbiter::<Raw>::new(&worker);
    let deadline = Instant::now() + Duration::from_secs(30);
    let finish = Cell::new(false);
    let active = poll_fn(|_| {
        if finish.get() {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    });
    let mut first = pin!(arbiter.run(deadline, active));
    assert!(poll_once(first.as_mut()).is_pending());
    {
        let mut canceled = pin!(arbiter.run(deadline, async { Ok(()) }));
        assert!(poll_once(canceled.as_mut()).is_pending());
    }
    assert!(!worker.is_closed());
    finish.set(true);
    assert_eq!(poll_once(first.as_mut()), Poll::Ready(Ok(())));
    assert!(!worker.is_closed());
    assert_eq!(
        poll_once(pin!(arbiter.run(deadline, async { Ok(()) }))),
        Poll::Ready(Ok(()))
    );
}

#[test]
fn deadline_while_waiting_for_the_write_lane_retires_the_physical_connection() {
    let ConnectionSlotOwners {
        worker,
        link: _link,
    } = owners();
    let arbiter = GattWriteArbiter::<Raw>::new(&worker);
    let native_polled = Cell::new(false);
    let mut expired = pin!(arbiter.run(Instant::MIN, async {
        native_polled.set(true);
        Ok(())
    }));
    {
        let mut active = pin!(arbiter.run(
            Instant::now() + Duration::from_secs(30),
            core::future::pending::<Result<(), ControlIoError>>()
        ));
        assert!(poll_once(active.as_mut()).is_pending());
        // Embassy's timer yields once even when its deadline is already past.
        assert!(poll_once(expired.as_mut()).is_pending());
        assert_eq!(
            poll_once(expired.as_mut()),
            Poll::Ready(Err(ControlIoError))
        );
        assert!(worker.is_closed());
        assert!(!native_polled.get());
    }
    assert_eq!(
        poll_once(pin!(
            arbiter.run(Instant::now() + Duration::from_secs(30), async { Ok(()) })
        )),
        Poll::Ready(Err(ControlIoError))
    );
}

#[test]
fn expired_deadline_and_native_error_never_release_a_reusable_write_lane() {
    for already_expired in [true, false] {
        let ConnectionSlotOwners {
            worker,
            link: _link,
        } = owners();
        let arbiter = GattWriteArbiter::<Raw>::new(&worker);
        let native_polled = Cell::new(false);
        let deadline = if already_expired {
            Instant::MIN
        } else {
            Instant::now() + Duration::from_secs(30)
        };
        let operation = async {
            native_polled.set(true);
            Err::<(), _>(ControlIoError)
        };
        assert_eq!(
            poll_once(pin!(arbiter.run(deadline, operation))),
            Poll::Ready(Err(ControlIoError))
        );
        assert_eq!(native_polled.get(), !already_expired);
        assert!(worker.is_closed());
        assert_eq!(
            poll_once(pin!(
                arbiter.run(Instant::now() + Duration::from_secs(30), async { Ok(()) })
            )),
            Poll::Ready(Err(ControlIoError))
        );
    }
}
