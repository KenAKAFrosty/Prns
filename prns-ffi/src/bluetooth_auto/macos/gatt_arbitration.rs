//! A single native write lane shared by one connection's control and data owners.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, MutexGuard};
use tokio::time::{timeout_at, Instant};

use super::MacosBleError;

#[derive(Default)]
pub(super) struct GattWriteGate {
    lane: Mutex<()>,
    retired: AtomicBool,
}

impl GattWriteGate {
    pub(super) fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }

    /// Waiting for the lane does not start a new deadline or submit native work. Once admitted,
    /// cancellation/error retires the exact connection before unlocking its untagged reply lane.
    pub(super) async fn run<T>(
        &self,
        deadline: Instant,
        operation: impl Future<Output = Result<T, MacosBleError>>,
        retire: impl FnOnce(),
    ) -> Result<T, MacosBleError> {
        let lock = timeout_at(deadline, self.lane.lock())
            .await
            .map_err(|_| MacosBleError::GattWriteTimeout)?;
        if self.is_retired() {
            return Err(MacosBleError::Closed);
        }
        if Instant::now() >= deadline {
            return Err(MacosBleError::GattWriteTimeout);
        }
        let mut transaction = WriteTransaction {
            gate: self,
            _lock: lock,
            retire: Some(retire),
        };
        let result = timeout_at(deadline, operation)
            .await
            .map_err(|_| MacosBleError::GattWriteTimeout)
            .and_then(core::convert::identity)
            .and_then(|value| {
                if Instant::now() < deadline {
                    Ok(value)
                } else {
                    Err(MacosBleError::GattWriteTimeout)
                }
            });
        if result.is_ok() {
            transaction.retire = None;
        }
        drop(transaction);
        result
    }
}

struct WriteTransaction<'a, R: FnOnce()> {
    gate: &'a GattWriteGate,
    _lock: MutexGuard<'a, ()>,
    retire: Option<R>,
}

impl<R: FnOnce()> Drop for WriteTransaction<'_, R> {
    fn drop(&mut self) {
        if let Some(retire) = self.retire.take() {
            self.gate.retired.store(true, Ordering::Release);
            retire();
        }
        // Rust drops the mutex guard only after this exact-owner retirement has run.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::future::{pending, poll_fn};
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    fn poll<T>(future: std::pin::Pin<&mut impl Future<Output = T>>) -> Poll<T> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    #[tokio::test(start_paused = true)]
    async fn canceled_waiter_does_not_cancel_or_poll_the_active_native_write() {
        let gate = GattWriteGate::default();
        let ready = Cell::new(false);
        let deadline = Instant::now() + Duration::from_secs(5);
        let active = poll_fn(|_| {
            if ready.get() {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        });
        let mut active = pin!(gate.run(deadline, active, || panic!("active write was canceled")));
        assert!(poll(active.as_mut()).is_pending());
        {
            let mut waiting = pin!(gate.run(
                deadline,
                poll_fn(|_| -> Poll<Result<(), MacosBleError>> {
                    panic!("waiting work cannot reach the platform")
                }),
                || panic!("queue cancellation cannot retire the active owner")
            ));
            assert!(poll(waiting.as_mut()).is_pending());
        }
        ready.set(true);
        assert!(matches!(poll(active.as_mut()), Poll::Ready(Ok(()))));
        assert!(!gate.is_retired());
        assert!(gate
            .run(deadline, async { Ok(()) }, || panic!())
            .await
            .is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn canceled_admitted_write_retires_before_a_waiter_can_submit() {
        let gate = GattWriteGate::default();
        let retired = Cell::new(false);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut waiting = pin!(gate.run(
            deadline,
            poll_fn(|_| -> Poll<Result<(), MacosBleError>> {
                panic!("poisoned lane cannot issue the next write")
            }),
            || panic!("never admitted")
        ));
        {
            let mut active = pin!(gate.run(
                deadline,
                pending::<Result<(), MacosBleError>>(),
                || retired.set(true)
            ));
            assert!(poll(active.as_mut()).is_pending());
            assert!(poll(waiting.as_mut()).is_pending());
        }
        assert!(retired.get());
        assert!(gate.is_retired());
        assert!(matches!(
            poll(waiting.as_mut()),
            Poll::Ready(Err(MacosBleError::Closed))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn queued_time_consumes_the_original_deadline() {
        let gate = GattWriteGate::default();
        let lock = gate.lane.lock().await;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut write = pin!(gate.run(deadline, pending::<Result<(), MacosBleError>>(), || {}));
        assert!(poll(write.as_mut()).is_pending());
        tokio::time::advance(Duration::from_secs(4)).await;
        drop(lock);
        assert!(poll(write.as_mut()).is_pending());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(matches!(
            poll(write.as_mut()),
            Poll::Ready(Err(MacosBleError::GattWriteTimeout))
        ));
        assert!(gate.is_retired());
    }

    #[tokio::test(start_paused = true)]
    async fn expiry_while_waiting_never_submits_or_retires_another_write() {
        let gate = GattWriteGate::default();
        let _lock = gate.lane.lock().await;
        let result = gate.run(
            Instant::now() + Duration::from_secs(1),
            poll_fn(|_| -> Poll<Result<(), MacosBleError>> {
                panic!("expired queue waiter must not submit")
            }),
            || panic!("unadmitted timeout must not retire"),
        );
        assert!(matches!(result.await, Err(MacosBleError::GattWriteTimeout)));
        assert!(!gate.is_retired());
    }
}
