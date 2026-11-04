//! Retained resource-cleanup observation, independent of any async executor.
use crate::{lock, CloseError};
use std::{
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
    thread::{self, ThreadId},
};

type Result<E> = std::result::Result<(), CloseError<E>>;
struct State<E> {
    result: Option<Result<E>>,
    thread: Option<ThreadId>,
    // Each live future owns at most one entry; dropping it removes the entry.
    waiters: Vec<(Arc<()>, Waker)>,
}

/// Inspect or await eventual resource cleanup. Merely observing does not close
/// admission. Clone to wait independently; dropping a waiter never cancels work.
///
/// A terminal result means initializer-owned resources and queued jobs have been
/// released, including after an initializer/handler panic. It does not join the
/// OS thread or imply that an interrupted domain mutation did not commit. A
/// missing result means cleanup is unresolved and the worker still owns it.
/// This future must not be synchronously awaited on the worker it observes.
pub struct Completion<E> {
    state: Arc<Mutex<State<E>>>,
    registration: Option<Arc<()>>,
}
impl<E> Clone for Completion<E> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            registration: None,
        }
    }
}
impl<E> Completion<E> {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                result: None,
                thread: None,
                waiters: Vec::new(),
            })),
            registration: None,
        }
    }
    pub(crate) fn set_thread(&self, thread: ThreadId) {
        lock(&self.state).thread = Some(thread);
    }
    /// The same retained outcome is returned to every observer and retry.
    pub fn status(&self) -> Option<Result<E>> {
        lock(&self.state).result.clone()
    }
    pub(crate) fn take_startup_result(&self) -> Option<Result<E>> {
        lock(&self.state).result.take()
    }
    pub(crate) fn finish(&self, result: Result<E>) {
        let waiters = {
            let mut state = lock(&self.state);
            if state.result.is_some() {
                return;
            }
            state.result = Some(result);
            std::mem::take(&mut state.waiters)
        };
        // Waker callbacks may reenter observation; never invoke/drop one under
        // the state mutex. Only the first terminal result can publish completion.
        for (_, waker) in waiters {
            // One broken executor callback must not strand other waiters.
            let _ = catch_unwind(AssertUnwindSafe(|| waker.wake()));
        }
    }
}
impl<E> Future for Completion<E> {
    type Output = Result<E>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        // Even Waker::clone is an external vtable call.
        let waker = cx.waker().clone();
        let mut state = lock(&this.state);
        if let Some(result) = &state.result {
            return Poll::Ready(result.clone());
        }
        if state.thread == Some(thread::current().id()) {
            return Poll::Ready(Err(CloseError::Reentrant));
        }
        let previous = if let Some(registration) = &this.registration {
            let Some((_, registered)) = state
                .waiters
                .iter_mut()
                .find(|(id, _)| Arc::ptr_eq(id, registration))
            else {
                // Publication drains entries only while installing a result.
                return Poll::Pending;
            };
            Some(std::mem::replace(registered, waker))
        } else {
            let registration = Arc::new(());
            state.waiters.push((Arc::clone(&registration), waker));
            this.registration = Some(registration);
            None
        };
        drop(state);
        drop(previous);
        Poll::Pending
    }
}
impl<E> Drop for Completion<E> {
    fn drop(&mut self) {
        let Some(registration) = self.registration.take() else {
            return;
        };
        let removed = {
            let mut state = lock(&self.state);
            state
                .waiters
                .iter()
                .position(|(id, _)| Arc::ptr_eq(id, &registration))
                .map(|index| state.waiters.swap_remove(index))
        };
        drop(removed);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use std::{
        convert::Infallible,
        sync::atomic::{AtomicUsize, Ordering},
        task::Wake,
    };

    struct ObserveOnWake(Completion<Infallible>, Arc<AtomicUsize>);
    impl Wake for ObserveOnWake {
        fn wake(self: Arc<Self>) {
            assert!(matches!(self.0.status(), Some(Ok(()))));
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl Drop for ObserveOnWake {
        fn drop(&mut self) {
            // Dropping or replacing a registered waker must also stay outside
            // the completion lock: executors may release arbitrary task state.
            let _ = self.0.status();
        }
    }
    #[test]
    fn dropped_and_replaced_waiters_unregister_and_callbacks_can_reenter() {
        let completion = Completion::<Infallible>::new();
        let calls = Arc::new(AtomicUsize::new(0));
        for _ in 0..100 {
            let waker = Waker::from(Arc::new(ObserveOnWake(completion.clone(), calls.clone())));
            let mut abandoned = completion.clone();
            assert!(Pin::new(&mut abandoned)
                .poll(&mut Context::from_waker(&waker))
                .is_pending());
            assert_eq!(lock(&completion.state).waiters.len(), 1);
            drop(abandoned);
            assert_eq!(lock(&completion.state).waiters.len(), 0);
        }
        let mut waiter = completion.clone();
        for _ in 0..100 {
            let waker = Waker::from(Arc::new(ObserveOnWake(completion.clone(), calls.clone())));
            assert!(Pin::new(&mut waiter)
                .poll(&mut Context::from_waker(&waker))
                .is_pending());
            assert_eq!(lock(&completion.state).waiters.len(), 1);
        }
        completion.finish(Ok(()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(lock(&completion.state).waiters.is_empty());
    }
    struct PanicWake;
    impl Wake for PanicWake {
        fn wake(self: Arc<Self>) {
            panic!("executor wake failed");
        }
    }
    #[test]
    fn one_panicking_executor_does_not_strand_other_observers() {
        let completion = Completion::<Infallible>::new();
        let mut broken = completion.clone();
        let mut healthy = completion.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let first = Waker::from(Arc::new(PanicWake));
        let second = Waker::from(Arc::new(ObserveOnWake(completion.clone(), calls.clone())));
        assert!(Pin::new(&mut broken)
            .poll(&mut Context::from_waker(&first))
            .is_pending());
        assert!(Pin::new(&mut healthy)
            .poll(&mut Context::from_waker(&second))
            .is_pending());
        completion.finish(Ok(()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(completion.status(), Some(Ok(()))));
    }
}
