#![allow(clippy::expect_used)]
use super::*;

fn command(store: &SnapshotStore) -> WifiCommand {
    let operation = RemoteWifiOperation {
        operation_id: 1,
        generation_id: store.read().generation_id,
        target_identity_fingerprint: vec![7; 16],
        action: RemoteWifiAction::Inspect,
        candidate_revision: None,
        status: RemoteWifiStatus::Pending,
    };
    store.update(|snapshot| {
        snapshot.runtime = DevelopmentNodeRuntime::Running;
        snapshot.last_remote_wifi = Some(operation.clone());
        snapshot.active_operation = Some(DevelopmentNodeOperation {
            kind: DevelopmentNodeOperationKind::RemoteWifi,
            started_at_millis: 0,
        });
    });
    WifiCommand {
        operation,
        work: Work::Inspect,
        deadline: tokio::time::Instant::now() + WIFI_TIMEOUT,
    }
}

#[tokio::test(start_paused = true)]
async fn timeout_before_dispatch_is_failed_after_dispatch_is_uncertain() {
    for dispatched_at_timeout in [false, true] {
        let store = SnapshotStore::new();
        let command = command(&store);
        let admitted = AtomicBool::new(true);
        let (_sender, mut shutdown) = watch::channel(false);
        assert!(
            !run_with(
                command,
                &store,
                &admitted,
                &mut shutdown,
                |_, _, dispatched| async move {
                    dispatched.store(dispatched_at_timeout, Ordering::Release);
                    std::future::pending().await
                }
            )
            .await
        );
        let status = store
            .read()
            .last_remote_wifi
            .expect("operation retained")
            .status;
        if dispatched_at_timeout {
            assert_eq!(
                status,
                RemoteWifiStatus::OutcomeUnknown {
                    reason: RemoteControlAnnounceUnknownReason::Timeout
                }
            );
        } else {
            assert!(matches!(
                status,
                RemoteWifiStatus::Failed {
                    stage: RemoteManagementFailureStage::Timeout,
                    ..
                }
            ));
        }
        assert!(!admitted.load(Ordering::Acquire));
        assert!(store.read().active_operation.is_none());
    }
}

#[tokio::test]
async fn actor_shutdown_keeps_dispatched_write_uncertain() {
    let store = SnapshotStore::new();
    let command = command(&store);
    let admitted = AtomicBool::new(true);
    let (sender, mut shutdown) = watch::channel(false);
    assert!(
        run_with(
            command,
            &store,
            &admitted,
            &mut shutdown,
            |_, _, dispatched| async move {
                dispatched.store(true, Ordering::Release);
                sender.send(true).expect("actor alive");
                std::future::pending().await
            }
        )
        .await
    );
    assert_eq!(
        store.read().last_remote_wifi.expect("operation").status,
        RemoteWifiStatus::OutcomeUnknown {
            reason: RemoteControlAnnounceUnknownReason::NodeStopped
        }
    );
    assert!(!admitted.load(Ordering::Acquire));
}

#[tokio::test]
async fn candidate_revision_is_retained_and_status_has_no_credentials() {
    let store = SnapshotStore::new();
    let command = command(&store);
    let operation = command.operation.clone();
    let admitted = AtomicBool::new(true);
    let (_sender, mut shutdown) = watch::channel(false);
    update(&store, &operation, |current| {
        current.candidate_revision = Some(2)
    });
    run_with(command, &store, &admitted, &mut shutdown, |_, _, _| async {
        RemoteWifiStatus::Observed {
            transaction: RemoteWifiTransaction::AwaitingConfirmation {
                revision: 2,
                remaining_seconds: 119,
            },
        }
    })
    .await;
    let snapshot = store.read();
    let retained = snapshot.last_remote_wifi.expect("operation");
    assert_eq!(retained.candidate_revision, Some(2));
    assert!(matches!(retained.status, RemoteWifiStatus::Observed { .. }));
    assert!(!format!("{retained:?}").contains("password"));
}

#[tokio::test]
async fn old_generation_completion_cannot_publish_into_new_runtime() {
    let store = SnapshotStore::new();
    let command = command(&store);
    let old = command.operation.clone();
    store.update(|snapshot| snapshot.generation_id += 1);
    update(&store, &old, |current| {
        current.candidate_revision = Some(99)
    });
    let admitted = AtomicBool::new(true);
    let (_sender, mut shutdown) = watch::channel(false);
    run_with(command, &store, &admitted, &mut shutdown, |_, _, _| async {
        RemoteWifiStatus::Observed {
            transaction: RemoteWifiTransaction::FactoryProvisioning,
        }
    })
    .await;
    let retained = store
        .read()
        .last_remote_wifi
        .expect("old evidence retained");
    assert_eq!(retained.candidate_revision, None);
    assert_eq!(retained.status, RemoteWifiStatus::Pending);
}

#[tokio::test]
async fn lifecycle_retains_uncertainty_until_reset_without_replaying_work() {
    let store = SnapshotStore::new();
    let _command = command(&store);
    store.stopped();
    assert_eq!(
        store.read().last_remote_wifi.expect("retained").status,
        RemoteWifiStatus::OutcomeUnknown {
            reason: RemoteControlAnnounceUnknownReason::NodeStopped
        }
    );
    store.begin_generation(PrimaryIdentityState::Missing);
    assert!(store.read().last_remote_wifi.is_some());
    store.reset();
    assert!(store.read().last_remote_wifi.is_none());
}
