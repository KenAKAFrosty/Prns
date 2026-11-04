use super::*;
use crate::owner::OwnedSession;
use personal_rns::remote_control::{RemoteControlProtocolError, RemoteControlResponse};
use personal_rns::units::RttMillis;
use prns_host::{HostConfig, HostRole, IdentityConfig, PersistenceConfig, PrnsLimits};
use std::time::Duration;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
}

fn config() -> HostConfig {
    HostConfig {
        identity: IdentityConfig::GenerateEphemeral,
        persistence: PersistenceConfig::Ephemeral,
        role: HostRole::Endpoint,
        destinations: vec![],
        required_capabilities: vec![],
        limits: PrnsLimits::balanced(),
    }
}

fn pending(store: &RemoteControlOperations, id: u64) -> Arc<Operation> {
    let operation = Arc::new(Operation {
        id,
        destination: RemoteControlOperationDestination::Link {
            link_id: personal_rns::routing::links::LinkId::new([1; 16]),
        },
        kind: RemoteControlRequestKind::Describe,
        state: Mutex::new(State {
            value: RemoteControlOperationState::Pending,
            waiters: Vec::new(),
        }),
    });
    lock(&store.entries).records.insert(id, operation.clone());
    operation
}

#[test]
fn cancel_and_send_have_one_atomic_boundary() {
    let store = Arc::new(RemoteControlOperations::new(2).expect("limits"));
    let operation = pending(&store, 1);
    assert_eq!(
        store.cancel(1),
        RemoteControlOperationCancellation::CancelledBeforeSubmission
    );
    assert!(operation
        .begin(|| -> Result<(), _> { panic!("cancelled must never send") })
        .is_none());

    let operation = pending(&store, 2);
    let (enter, entered) = std::sync::mpsc::sync_channel(1);
    let (release, released) = std::sync::mpsc::sync_channel(1);
    let sender = std::thread::spawn(move || {
        operation.begin(|| {
            enter.send(()).expect("entered");
            released
                .recv_timeout(Duration::from_secs(5))
                .expect("release");
            Ok(())
        })
    });
    entered
        .recv_timeout(Duration::from_secs(5))
        .expect("inside atomic send");
    let cancelling = store.clone();
    let cancel = std::thread::spawn(move || cancelling.cancel(2));
    release.send(()).expect("release");
    assert_eq!(sender.join().expect("sender"), Some(()));
    assert_eq!(
        cancel.join().expect("cancel"),
        RemoteControlOperationCancellation::AlreadySubmitted
    );
    assert_eq!(store.retire(2), RemoteControlOperationRetirement::Pending);
}

#[test]
fn dropped_queued_closures_and_active_tasks_retain_distinct_evidence() {
    let store = RemoteControlOperations::new(3).expect("limits");
    let first = pending(&store, 1);
    let completion = CompletionOnDrop(first.clone());
    let queued = move || async move {
        drop(completion);
    };
    drop(queued); // host closed its inbox before it polled the queued task
    assert!(matches!(
        first.snapshot().state,
        RemoteControlOperationState::NotSubmitted { .. }
    ));

    let submitted = pending(&store, 2);
    assert_eq!(submitted.begin(|| Ok(())), Some(()));
    drop(CompletionOnDrop(submitted.clone())); // host abort joined an active task
    assert!(matches!(
        submitted.snapshot().state,
        RemoteControlOperationState::OutcomeUnknown { .. }
    ));
    assert_eq!(
        store.cancel(2),
        RemoteControlOperationCancellation::AlreadySettled
    );

    let cancelled = pending(&store, 3);
    assert_eq!(
        store.cancel(3),
        RemoteControlOperationCancellation::CancelledBeforeSubmission
    );
    drop(CompletionOnDrop(cancelled.clone()));
    assert_eq!(
        cancelled.snapshot().state,
        RemoteControlOperationState::CancelledBeforeSubmission
    );
}

#[test]
fn peer_replies_and_ambiguous_transport_failures_are_not_conflated() {
    let response =
        RemoteControlResponse::ProtocolError(RemoteControlProtocolError::UnknownRequestKind {
            found: 0xEF,
        });
    let RemoteControlResponse::ProtocolError(error) = response else {
        unreachable!()
    };
    assert!(matches!(
        classify_submitted(Err(HostRemoteControlError::Exchange(
            RemoteControlError::Remote(error)
        ))),
        RemoteControlOperationState::Completed { .. }
    ));
    for error in [
        RemoteControlError::Request(SendError::NodeStopped),
        RemoteControlError::Request(SendError::Failed(SendRequestFailure::Timeout)),
        RemoteControlError::Response(
            personal_rns::remote_control::RemoteControlResponseParseError::Truncated,
        ),
    ] {
        assert!(matches!(
            classify_submitted(Err(HostRemoteControlError::Exchange(error))),
            RemoteControlOperationState::OutcomeUnknown { .. }
        ));
    }
    assert!(matches!(
        classify_submitted(Ok((
            RemoteControlResponse::AnnounceSelf(
                personal_rns::remote_control::RemoteControlAnnounceSelfOutcome::Announced
            ),
            RttMillis::new(3)
        ))),
        RemoteControlOperationState::Completed { .. }
    ));
}

#[test]
fn actual_host_result_survives_waiter_loss_fence_stop_and_explicit_retirement() {
    runtime().block_on(async {
        let owner = OwnedSession::open(config()).await.expect("host");
        let scope = owner.client_scope();
        let client = scope.client();
        let store = RemoteControlOperations::new(1).expect("limits");
        let id = store
            .submit_link(
                &client,
                prns_host::LinkId::new([0xA7; 16]),
                RemoteControlRequest::Describe,
            )
            .expect("admitted");
        // The operation was synchronously accepted; it has no dependency on any waiter.
        drop(Box::pin(store.wait(id)));
        scope.fence();
        let result = tokio::time::timeout(Duration::from_secs(5), store.wait(id))
            .await
            .expect("settled")
            .expect("retained");
        assert!(
            matches!(
                result.state,
                RemoteControlOperationState::NotSubmitted {
                    failure: HostRemoteControlError::Exchange(RemoteControlError::Request(
                        SendError::Failed(SendRequestFailure::Rejected(_))
                    ))
                }
            ),
            "the real engine proves invalid-link rejection: {result:?}"
        );
        owner.stop().await.expect("joined shutdown");
        assert_eq!(store.wait(id).await, Some(result.clone()));
        assert_eq!(store.operation(id), Some(result));
        // Full records cannot be replaced implicitly, even after host replacement.
        assert_eq!(
            store.submit_link(
                &client,
                prns_host::LinkId::new([1; 16]),
                RemoteControlRequest::Describe
            ),
            Err(HostSubmitError::Busy)
        );
        assert_eq!(store.retire(id), RemoteControlOperationRetirement::Retired);
        assert!(store.wait(id).await.is_none());
        assert_eq!(store.retire(id), RemoteControlOperationRetirement::Missing);
        assert_eq!(
            store.cancel(id),
            RemoteControlOperationCancellation::Missing
        );
        assert_eq!(
            store.submit_target(
                &client,
                prns_host::IdentityHash::new([1; 16]),
                RemoteControlRequest::Describe
            ),
            Err(HostSubmitError::Stopped)
        );
        assert!(
            store.operations().is_empty(),
            "failed admission does not leave a record"
        );

        let replacement = OwnedSession::open(config()).await.expect("replacement");
        let next = store
            .submit_link(
                &replacement.client(),
                prns_host::LinkId::new([1; 16]),
                RemoteControlRequest::Describe,
            )
            .expect("new admission");
        assert!(next > id, "retired IDs never alias new operations");
        replacement.stop().await.expect("stop accepted task");
        assert!(
            tokio::time::timeout(Duration::from_secs(5), store.wait(next))
                .await
                .expect("drop guard settles")
                .expect("retained")
                .state
                .is_terminal()
        );
    });
}

#[test]
fn host_queue_capacity_and_fenced_admission_leave_no_phantom_records() {
    runtime().block_on(async {
        let mut configuration = config();
        configuration.limits = PrnsLimits::try_new(1, 1, 1024, 1).expect("limits");
        let owner = OwnedSession::open(configuration).await.expect("host");
        let client = owner.client();
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let blocker = client
            .admit(|host| {
                host.begin_preview_operation(move |_| async move {
                    let _ = released.await;
                })
            })
            .expect("blocker admission");
        let store = RemoteControlOperations::new(2).expect("limits");
        assert_eq!(
            store.submit_link(
                &client,
                prns_host::LinkId::new([1; 16]),
                RemoteControlRequest::Describe
            ),
            Err(HostSubmitError::Busy)
        );
        assert!(store.operations().is_empty());
        release.send(()).expect("release");
        blocker.await.expect("blocker completed");
        let scope = owner.client_scope();
        scope.fence();
        assert_eq!(
            store.submit_link(
                &scope.client(),
                prns_host::LinkId::new([1; 16]),
                RemoteControlRequest::Describe
            ),
            Err(HostSubmitError::Stopped)
        );
        assert!(store.operations().is_empty());
        owner.stop().await.expect("stop");
    });
}

#[test]
fn limits_and_identifier_exhaustion_fail_closed() {
    assert!(RemoteControlOperations::new(0).is_err());
    assert!(RemoteControlOperations::new(MAX_OPERATIONS + 1).is_err());
    runtime().block_on(async {
        let store = RemoteControlOperations::new(1).expect("limits");
        lock(&store.entries).next = u64::MAX;
        let owner = OwnedSession::open(config()).await.expect("host");
        assert_eq!(
            store.submit_link(
                &owner.client(),
                prns_host::LinkId::new([1; 16]),
                RemoteControlRequest::Describe
            ),
            Err(HostSubmitError::Busy)
        );
        assert!(store.operations().is_empty());
        owner.stop().await.expect("stop");
    });
}

#[test]
fn cancelled_and_panicking_waiters_cannot_interrupt_completion_or_other_waiters() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;
    struct Observer {
        operation: Arc<Operation>,
        wakes: AtomicUsize,
        panic: bool,
    }
    impl Wake for Observer {
        fn wake(self: Arc<Self>) {
            assert!(self.operation.snapshot().state.is_terminal());
            self.wakes.fetch_add(1, Ordering::SeqCst);
            assert!(!self.panic, "injected executor wake panic");
        }
    }
    impl Drop for Observer {
        fn drop(&mut self) {
            let _ = self.operation.snapshot();
        }
    }
    let store = RemoteControlOperations::new(1).expect("limits");
    let operation = pending(&store, 1);
    for _ in 0..100 {
        let waker = Waker::from(Arc::new(Observer {
            operation: operation.clone(),
            wakes: AtomicUsize::new(0),
            panic: false,
        }));
        let mut abandoned = Box::pin(store.wait(1));
        assert!(abandoned
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());
        assert_eq!(lock(&operation.state).waiters.len(), 1);
        drop(waker); // the registration now owns the last external callback
        drop(abandoned);
        assert!(lock(&operation.state).waiters.is_empty());
    }
    let mut replacing = Box::pin(store.wait(1));
    for _ in 0..100 {
        let waker = Waker::from(Arc::new(Observer {
            operation: operation.clone(),
            wakes: AtomicUsize::new(0),
            panic: false,
        }));
        assert!(replacing
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());
        assert_eq!(lock(&operation.state).waiters.len(), 1);
        // The next poll drops this callback while replacing the registration.
        drop(waker);
    }
    drop(replacing);
    assert!(lock(&operation.state).waiters.is_empty());
    let broken = Waker::from(Arc::new(Observer {
        operation: operation.clone(),
        wakes: AtomicUsize::new(0),
        panic: true,
    }));
    let healthy = Arc::new(Observer {
        operation: operation.clone(),
        wakes: AtomicUsize::new(0),
        panic: false,
    });
    let healthy_waker = Waker::from(healthy.clone());
    let mut first = Box::pin(store.wait(1));
    let mut second = Box::pin(store.wait(1));
    assert!(first
        .as_mut()
        .poll(&mut Context::from_waker(&broken))
        .is_pending());
    for _ in 0..100 {
        assert!(second
            .as_mut()
            .poll(&mut Context::from_waker(&healthy_waker))
            .is_pending());
        assert_eq!(lock(&operation.state).waiters.len(), 2);
    }
    assert_eq!(operation.begin(|| Ok(())), Some(()));
    assert_eq!(
        healthy.wakes.load(Ordering::SeqCst),
        0,
        "submission is queried, terminal waits stay pending"
    );
    drop(CompletionOnDrop(operation.clone()));
    assert_eq!(healthy.wakes.load(Ordering::SeqCst), 1);
    assert!(lock(&operation.state).waiters.is_empty());
    for wait in [&mut first, &mut second] {
        let Poll::Ready(Some(snapshot)) =
            wait.as_mut().poll(&mut Context::from_waker(&healthy_waker))
        else {
            panic!("terminal");
        };
        assert!(matches!(
            snapshot.state,
            RemoteControlOperationState::OutcomeUnknown { .. }
        ));
    }
}
