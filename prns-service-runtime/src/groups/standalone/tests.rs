#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    task::{Wake, Waker},
};
struct PanicWake;
impl Wake for PanicWake {
    fn wake(self: Arc<Self>) {
        panic!("broken executor");
    }
}
struct Observe(Arc<Shared>, Arc<AtomicUsize>);
impl Wake for Observe {
    fn wake(self: Arc<Self>) {
        assert_eq!(self.0.lock().startup, Some(Ok(())));
        self.1.fetch_add(1, Ordering::SeqCst);
    }
}
impl Drop for Observe {
    fn drop(&mut self) {
        let _status = self.0.lock().status.clone();
    }
}
#[test]
fn cancelled_startup_waits_reclaim_slots_and_publication_is_retained() {
    let shared = Shared::new();
    for _ in 0..100 {
        let mut waiter = Box::pin(shared.ready());
        assert!(waiter
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        drop(waiter);
        assert_eq!(shared.lock().waiters.len(), 1);
    }
    let mut waiters: Vec<_> = (0..32).map(|_| Box::pin(shared.ready())).collect();
    for waiter in &mut waiters {
        assert!(waiter
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
    }
    assert!(matches!(
        Box::pin(shared.ready())
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(Error::ObserverCapacity))
    ));
    shared.change(|state| state.startup = Some(Ok(())));
    assert!(shared.lock().waiters.is_empty());
    for waiter in &mut waiters {
        assert!(matches!(
            waiter
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(()))
        ));
    }
    assert!(matches!(
        Box::pin(shared.ready())
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(()))
    ));
}
#[test]
fn one_bad_executor_does_not_strand_startup_or_run_callbacks_under_locks() {
    let shared = Arc::new(Shared::new());
    let mut broken = Box::pin(shared.ready());
    let mut healthy = Box::pin(shared.ready());
    let calls = Arc::new(AtomicUsize::new(0));
    let bad = Waker::from(Arc::new(PanicWake));
    let good = Waker::from(Arc::new(Observe(shared.clone(), calls.clone())));
    assert!(broken
        .as_mut()
        .poll(&mut Context::from_waker(&bad))
        .is_pending());
    assert!(healthy
        .as_mut()
        .poll(&mut Context::from_waker(&good))
        .is_pending());
    shared.change(|state| state.startup = Some(Ok(())));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        healthy
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(()))
    ));
}

#[test]
fn failed_local_close_preserves_typed_cause_without_claiming_cleanup() {
    use prns_identities_core::{ApiError, StorageFailure};
    use prns_storage::CloseError;
    let cause = ApiError::Storage {
        reason: StorageFailure::Corrupt,
    };
    let failed: CloseError<LocalError> = CloseError::Failed(Arc::new(
        crate::local::Error::Identity(owned::Error::Api(cause)),
    ));
    for (error, expected) in [
        (failed, Error::Identity { error: cause }),
        (CloseError::Panicked, Error::LifecyclePanicked),
        (CloseError::Reentrant, Error::Reentrant),
    ] {
        let shared = Shared::new();
        shared.change(|state| {
            state.closed = true;
            state.startup = Some(Ok(()));
            state.status.phase = Phase::Closing;
        });
        shared.fail(Error::from(&error), true);
        assert_eq!(
            shared.lock().status,
            Status {
                phase: Phase::Unresolved,
                failure: Some(expected),
                cleanup_complete: false,
                latest_identity_diagnostic: None,
            }
        );
    }
}

#[test]
fn store_failure_projection_keeps_binding_capacity_and_io_reasons() {
    use crate::stores::Error as StoreError;
    use prns_identities::scoped_store::Error as Binding;
    use prns_storage::{database::Error as Physical, files};
    let cases: [(StoreError<Infallible>, StoreFailure); 7] = [
        (StoreError::Capacity, StoreFailure::Capacity),
        (StoreError::Allocation, StoreFailure::Allocation),
        (
            StoreError::Binding(Binding::WrongBinding),
            StoreFailure::WrongBinding,
        ),
        (
            StoreError::Binding(Binding::MissingBinding),
            StoreFailure::MissingBinding,
        ),
        (
            StoreError::File(files::Error::UnsafePermissions),
            StoreFailure::UnsafePermissions,
        ),
        (
            StoreError::Physical(Physical::File(files::Error::Io(
                std::io::Error::from_raw_os_error(13),
            ))),
            StoreFailure::Io { code: Some(13) },
        ),
        (
            StoreError::Physical(Physical::InvalidOptions),
            StoreFailure::InvalidOptions,
        ),
    ];
    for (cause, reason) in cases {
        let detail = cause.to_string();
        assert_eq!(
            Error::from(crate::local::Error::Stores(cause)),
            Error::Storage { reason, detail }
        );
    }
}

#[test]
fn failed_startup_stop_settles_only_after_cleanup_publication() {
    let shared = Shared::new();
    let failure = Error::Identity {
        error: prns_identities_core::ApiError::Driver {
            reason: prns_identities_core::ServiceFailure::StorageUnavailable,
        },
    };
    shared.fail(failure.clone(), false);
    let stop = shared.request_network(RequestKind::Stop).unwrap();
    let mut waiting = Box::pin(stop.wait());
    assert!(waiting
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    assert!(!shared.lock().status.cleanup_complete);
    shared.finish_cleanup(&Err(failure.clone()));
    assert_eq!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(()))
    );
    assert!(shared.lock().status.cleanup_complete);
    assert_eq!(shared.lock().status.failure, Some(failure));
    assert_eq!(shared.lock().network.phase, NetworkPhase::Offline);
    let late = shared.request_network(RequestKind::Stop).unwrap();
    assert_eq!(
        Box::pin(late.wait())
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(()))
    );
    assert!(shared.lock().request.is_none());
}

#[test]
fn unresolved_cleanup_does_not_accept_a_network_stop_that_cannot_finish() {
    let shared = Shared::new();
    shared.fail(Error::LifecyclePanicked, true);
    assert!(matches!(
        shared.request_network(RequestKind::Stop),
        Err(Error::LifecyclePanicked)
    ));
    assert!(!shared.lock().status.cleanup_complete);
    assert!(shared.lock().request.is_none());
}
