#![cfg(all(feature = "host-test", feature = "uniffi-bindings"))]
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use prns_app::contract::{
    ContactListOutcome, ContactMutationOutcome, CreateManualContactInput, DevelopmentNodeRuntime,
    DevelopmentNodeStartInput, DevelopmentNodeStartOutcome, DevelopmentNodeStopOutcome,
    IdentityCreationOutcome, ListLxmfMessagesInput, LocalHostState, LxmfMessageListOutcome,
    NativeStoragePreparationOutcome,
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
    let storage_path = storage.to_str().expect("native storage path").to_owned();
    assert_eq!(
        prns_app::bindings::native_prepare_storage(storage_path.clone()),
        NativeStoragePreparationOutcome::Prepared
    );
    assert!(matches!(
        foreign_block_on(prns_app::bindings::create_manual_contact(
            CreateManualContactInput {
                destination: [5; 16],
                identity: None,
                alias: Some("Before startup".to_owned()),
            }
        )),
        ContactMutationOutcome::Saved { .. }
    ));
    assert!(matches!(
        foreign_block_on(prns_app::bindings::list_lxmf_messages(
            ListLxmfMessagesInput {
                peer: None,
                before: None,
                limit: 25,
            }
        )),
        LxmfMessageListOutcome::Listed { .. }
    ));
    assert!(matches!(
        prns_app::bindings::native_create_generated_identity(storage_path.clone()),
        IdentityCreationOutcome::Created { .. }
    ));
    let started = match prns_app::bindings::native_start(
        storage_path.clone(),
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
    assert!(from_foreign.revision >= started.revision);
    assert!(matches!(
        prns_app::bindings::native_stop(),
        DevelopmentNodeStopOutcome::Stopped
    ));
    let stopped = foreign_block_on(prns_app::bindings::read_snapshot());
    assert_eq!(stopped.runtime, DevelopmentNodeRuntime::Stopped);
    assert_eq!(stopped.generation_id, started.generation_id);
    let ContactListOutcome::Listed { contacts } =
        foreign_block_on(prns_app::bindings::list_contacts())
    else {
        panic!("offline contact list after node stop");
    };
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].destination, [5; 16]);
    assert_eq!(
        prns_app::bindings::native_reset(storage_path),
        DevelopmentNodeStopOutcome::AlreadyStopped
    );
    assert!(!storage.exists());
}
