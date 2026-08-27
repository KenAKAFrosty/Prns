use core::time::Duration;

use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::engine::InstantMillis;
use crate::identity::Zeroizing;
use crate::manifold::driver::{HostCommand, SelfRatchetSnapshot};
use crate::routing::{BlackholeExpiry, BlackholedIdentity};
use crate::runtime::{ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNodeRecipe};
use crate::wire::DestinationHash;

use super::super::test_remote_control_service;
use super::super::{PrnsNode, PrnsNodeHandle};
use super::{
    try_zeroed_buffer, wall_clock_timeline_origin, BlackholeSeedReport, NodePersistence,
    PersistenceEvent, PersistenceFlushStatus, PersistenceTrigger, MAX_BOOT_RECORD_LEN,
};

fn handle() -> (PrnsNodeHandle, UnboundedReceiver<HostCommand>) {
    let (commands, command_rx) = mpsc::unbounded_channel();
    (PrnsNodeHandle::over(commands), command_rx)
}

#[test]
fn an_oversized_persisted_length_is_rejected_before_allocation() {
    assert!(try_zeroed_buffer(MAX_BOOT_RECORD_LEN + 1).is_none());
    assert!(try_zeroed_buffer(usize::MAX).is_none());
}

#[tokio::test]
async fn one_ratchet_snapshot_command_resolves_one_destination() {
    let (handle, mut command_rx) = handle();
    let destination = DestinationHash::new([0x5A; 16]);
    let snapshotting = tokio::spawn(async move { handle.snapshot_self_ratchet(destination).await });
    let HostCommand::SnapshotSelfRatchet {
        destination: requested,
        reply,
    } = command_rx.recv().await.unwrap()
    else {
        panic!("expected one ratchet snapshot command");
    };
    assert_eq!(requested, destination);
    assert!(reply
        .send(Some(SelfRatchetSnapshot {
            destination,
            sealed: Zeroizing::new(vec![0xA5; 64]),
        }))
        .is_ok());
    let snapshot = snapshotting.await.unwrap().unwrap().unwrap();
    assert_eq!(snapshot.destination, destination);
    assert_eq!(snapshot.sealed.as_slice(), &[0xA5; 64]);
}

#[test]
fn the_standard_timeline_origin_is_unix_epoch_aligned() {
    let wall_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let origin = wall_clock_timeline_origin();

    assert!(wall_now.abs_diff(u128::from(origin.0)) < 1_000);
}

#[test]
fn boot_blackholes_seed_against_the_resumed_timeline() {
    let mut prns = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: test_remote_control_service(),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &()| {},
    })
    .with_timeline_origin(InstantMillis(1_000));
    let identity = crate::identity::IdentityHash::new([0x31; 16]);
    let source = crate::identity::IdentityHash::new([0x41; 16]);

    let report = prns.seed_blackholed_identities([
        BlackholedIdentity {
            identity,
            source,
            expiry: BlackholeExpiry::At(InstantMillis(2_000)),
            reason: Some("active"),
        },
        BlackholedIdentity {
            identity,
            source,
            expiry: BlackholeExpiry::Indefinite,
            reason: Some("duplicate"),
        },
        BlackholedIdentity {
            identity: crate::identity::IdentityHash::new([0x32; 16]),
            source,
            expiry: BlackholeExpiry::At(InstantMillis(999)),
            reason: Some("expired"),
        },
    ]);

    assert_eq!(
        report,
        BlackholeSeedReport {
            seeded_count: 1,
            refused_count: 1,
            dropped_count: 1,
        }
    );
    assert!(prns.node.engine.is_identity_blackholed(&identity));
    assert_eq!(prns.node.engine.blackholed_identity_count(), 1);
}

#[tokio::test]
async fn a_tolerated_write_failure_is_retried_while_the_node_keeps_running() {
    let directory =
        std::env::temp_dir().join(format!("prns-persistence-retry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let persistence = NodePersistence::custom_dir(&directory).unwrap();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: test_remote_control_service(),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &()| {},
    });
    let worker = persistence
        .worker(node.handle())
        .with_flush_interval(Duration::from_millis(10));
    let (node_shutdown, node_shutdown_requested) = tokio::sync::oneshot::channel();
    let node_run = node.run_until(async {
        let _ = node_shutdown_requested.await;
    });

    std::fs::remove_dir_all(&directory).unwrap();
    std::fs::write(&directory, b"persistence path blocked by a file").unwrap();
    let (events, mut observed) = mpsc::unbounded_channel();
    let (worker_shutdown, worker_shutdown_requested) = tokio::sync::oneshot::channel();
    let (worker_completed, worker_completion) = tokio::sync::oneshot::channel();
    let worker_run = async move {
        let status = worker
            .run(
                async {
                    let _ = worker_shutdown_requested.await;
                },
                move |event| match event {
                    PersistenceEvent::Flushed { trigger, .. } => {
                        let _ = events.send((trigger, PersistenceFlushStatus::Landed));
                    }
                    PersistenceEvent::FlushFailed { trigger, .. } => {
                        let _ = events.send((trigger, PersistenceFlushStatus::Failed));
                    }
                    PersistenceEvent::RatchetsFlushed { .. }
                    | PersistenceEvent::RatchetFlushFailed { .. } => {}
                },
            )
            .await;
        let _ = worker_completed.send(());
        status
    };
    let recoverable_directory = directory.clone();
    let recover_storage = async move {
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), observed.recv())
                .await
                .unwrap(),
            Some((PersistenceTrigger::Interval, PersistenceFlushStatus::Failed))
        );

        std::fs::remove_file(&recoverable_directory).unwrap();
        std::fs::create_dir_all(&recoverable_directory).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), observed.recv())
                .await
                .unwrap(),
            Some((PersistenceTrigger::Interval, PersistenceFlushStatus::Landed))
        );

        worker_shutdown.send(()).unwrap();
        worker_completion.await.unwrap();
        node_shutdown.send(()).unwrap();
    };

    let (node_result, worker_status, ()) = tokio::join!(node_run, worker_run, recover_storage);
    assert_eq!(worker_status, PersistenceFlushStatus::Landed);
    assert_eq!(node_result, Ok(()));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_reticulum_dir_nests_snapshots_under_storage_prns() {
    let reticulum_dir =
        std::env::temp_dir().join(format!("prns-reticulum-dir-{}", std::process::id()));
    let opened = super::NodePersistence::in_reticulum_dir(&reticulum_dir);
    let nested = reticulum_dir.join("storage").join("prns").is_dir();
    let _ = std::fs::remove_dir_all(&reticulum_dir);
    opened.unwrap();
    assert!(nested);
}
