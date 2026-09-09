#![cfg(all(feature = "host-test", feature = "uniffi-bindings"))]
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use prns_app::contract::{
    DevelopmentNodeRuntime, DevelopmentNodeStartInput, DevelopmentNodeStartOutcome,
    DevelopmentNodeStopOutcome, IdentityCreationOutcome, LocalHostState,
};

/// Poll like a foreign executor, with no Tokio runtime entered on this thread.
fn foreign_block_on<F: std::future::Future>(future: F) -> F::Output {
    struct Notify(std::thread::Thread);
    impl Wake for Notify {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Notify(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "foreign future exceeded its deadline"
        );
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park_timeout(
                deadline.saturating_duration_since(std::time::Instant::now()),
            ),
        }
    }
}

#[test]
fn foreign_snapshot_attaches_to_native_started_process_owner() {
    let directory = tempfile::tempdir().expect("isolated storage");
    let storage = directory.path().join("prns").join("development");
    assert!(matches!(
        prns_app::host_test::create_generated_identity(&storage),
        IdentityCreationOutcome::Created { .. }
    ));
    let started = match prns_app::host_test::start_configured(
        &storage,
        DevelopmentNodeStartInput {
            development_tcp_target: None,
        },
    ) {
        DevelopmentNodeStartOutcome::Started { snapshot } => snapshot,
        other => panic!("native startup failed: {other:?}"),
    };
    let from_foreign = foreign_block_on(prns_app::bindings::read_snapshot());
    assert_eq!(from_foreign.generation_id, started.generation_id);
    assert_eq!(from_foreign.primary_identity, started.primary_identity);
    assert_eq!(from_foreign.runtime, DevelopmentNodeRuntime::Running);
    assert!(matches!(
        from_foreign.local_host,
        LocalHostState::Running { .. }
    ));
    assert!(from_foreign.revision.0 >= started.revision.0);
    assert!(matches!(
        prns_app::host_test::stop(),
        DevelopmentNodeStopOutcome::Stopped
    ));
    let stopped = foreign_block_on(prns_app::bindings::read_snapshot());
    assert_eq!(stopped.runtime, DevelopmentNodeRuntime::Stopped);
    assert_eq!(stopped.generation_id, started.generation_id);
}
