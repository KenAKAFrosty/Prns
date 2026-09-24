use super::super::{host::FlushCommit, NodePersistence, PreparedFlush, TestDirectory};
use super::*;
use crate::engine::InstantMillis;
use crate::manifold::driver::PersistedStateSnapshot;
use crate::persistence::FileStore;
use crate::persistence::PersistedStore;
use crate::runtime::node_facade::PrnsNodeHandle;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{mpsc, oneshot};

fn persistence(directory: &TestDirectory) -> RemoteControlAuthorizationPersistence {
    let (commands, _) = mpsc::unbounded_channel();
    NodePersistence::custom_dir(directory.path())
        .unwrap()
        .worker(PrnsNodeHandle::over(commands))
        .remote_control_authorization_persistence()
}

fn flush(grants: &[u8], accesses: &[u8]) -> PreparedFlush {
    PreparedFlush {
        snapshot: PersistedStateSnapshot {
            taken_at: InstantMillis(42),
            routing_table: vec![1],
            tunnels: vec![2],
            destination_identities: vec![3],
        },
        remote_control_controller_grants: Some(grants.to_vec()),
        remote_control_target_accesses: Some(accesses.to_vec()),
    }
}

#[tokio::test]
async fn read_only_authorization_snapshot_does_not_invalidate_its_own_flush() {
    let directory = TestDirectory::new();
    let persistence = persistence(&directory);
    let revision = persistence
        .storage
        .lock()
        .unwrap()
        .authorization
        .flush_revision()
        .unwrap();
    let mut engine = crate::engine::EngineState::<crate::storage::GrowableHeap>::default();
    let mut remote = crate::runtime::configure_remote_control_service(
        &mut engine,
        crate::runtime::node_facade::test_remote_control_service(),
    )
    .unwrap();
    let (completion, receiver) = oneshot::channel();
    crate::runtime::remote_control_controller_grants::RemoteControlControllerGrantCommand::Snapshot { completion }
        .apply(&mut remote, Some(&persistence)).await.unwrap();
    assert!(receiver.await.unwrap().unwrap().is_some());
    assert!(persistence
        .storage
        .lock()
        .unwrap()
        .authorization
        .accepts_flush(&revision));
}

#[tokio::test(start_paused = true)]
async fn uncertain_publication_retries_without_rewriting_or_releasing_ownership() {
    for region in [
        SnapshotRegion::RemoteControlControllerGrants,
        SnapshotRegion::RemoteControlTargetAccesses,
    ] {
        let directory = TestDirectory::new();
        let persistence = persistence(&directory);
        let revision = persistence
            .storage
            .lock()
            .unwrap()
            .authorization
            .flush_revision()
            .unwrap();
        let transaction = persistence.begin().await.unwrap();
        let writes = Arc::new(AtomicUsize::new(0));
        let write_count = Arc::clone(&writes);
        let reads = Arc::new(AtomicUsize::new(0));
        let read_count = Arc::clone(&reads);
        let (events, mut observed) = mpsc::unbounded_channel();
        let confirmations = events.clone();
        let mut storing = Box::pin(transaction.store_with(
            region,
            b"candidate".to_vec(),
            move |store, region, bytes| {
                write_count.fetch_add(1, Ordering::SeqCst);
                store.store(region, bytes)?;
                events.send(0).unwrap();
                Err(FileStoreError::PublishedDurabilityUnconfirmed(
                    std::io::Error::other("lost acknowledgement"),
                ))
            },
            move |store, region, bytes| {
                let attempt = read_count.fetch_add(1, Ordering::SeqCst) + 1;
                confirmations.send(attempt).unwrap();
                match attempt {
                    1 => Err(FileStoreError::Io(std::io::Error::other("unavailable"))),
                    2 => Ok(FileStoreConfirmation::Missing),
                    3 => Ok(FileStoreConfirmation::Different),
                    4 => store.confirm_store(region, bytes),
                    _ => panic!("unexpected confirmation"),
                }
            },
        ));
        for expected in 0..4 {
            tokio::select! {
                event = observed.recv() => assert_eq!(event, Some(expected)),
                result = &mut storing => panic!("premature settlement: {result:?}"),
            }
            let mut storage = persistence.storage.lock().unwrap();
            assert!(matches!(
                storage.authorization.flush_revision(),
                Err(AuthorizationOwnerError::Busy)
            ));
            assert!(matches!(
                storage.commit_flush(&revision, flush(b"old", b"old")),
                FlushCommit::AuthorizationChanged
            ));
            assert_eq!(
                storage.store.stored_len(SnapshotRegion::Timebase).unwrap(),
                None
            );
        }
        storing.await.unwrap().unwrap();
        assert_eq!(
            (writes.load(Ordering::SeqCst), reads.load(Ordering::SeqCst)),
            (1, 4)
        );
        assert!(matches!(
            persistence.begin().await,
            Err(AuthorizationOwnerError::Busy)
        ));
        transaction.finish().await.unwrap();
        let mut storage = persistence.storage.lock().unwrap();
        assert!(matches!(
            storage.commit_flush(&revision, flush(b"old", b"old")),
            FlushCommit::AuthorizationChanged
        ));
        let fresh = storage.authorization.flush_revision().unwrap();
        assert!(matches!(
            storage.commit_flush(&fresh, flush(b"candidate", b"candidate")),
            FlushCommit::Completed(Ok(_))
        ));
        let mut bytes = [0; 9];
        assert_eq!(
            FileStore::new(directory.path())
                .load(region, &mut bytes)
                .unwrap(),
            Some(b"candidate".as_slice())
        );
    }
}

#[tokio::test]
async fn cancelled_in_flight_write_keeps_the_owner_after_the_blocking_task_finishes() {
    let directory = TestDirectory::new();
    let persistence = persistence(&directory);
    let revision = persistence
        .storage
        .lock()
        .unwrap()
        .authorization
        .flush_revision()
        .unwrap();
    let transaction = persistence.begin().await.unwrap();
    let (entered, entered_rx) = oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let (finished, finished_rx) = oneshot::channel();
    let mut writing = Box::pin(transaction.store_with(
        SnapshotRegion::RemoteControlControllerGrants,
        b"candidate".to_vec(),
        move |store, region, bytes| {
            entered.send(()).unwrap();
            released.recv().unwrap();
            let result = store.store(region, bytes);
            finished.send(()).unwrap();
            result
        },
        |store, region, bytes| store.confirm_store(region, bytes),
    ));
    tokio::select! {
        result = entered_rx => result.unwrap(),
        result = &mut writing => panic!("write bypassed barrier: {result:?}"),
    }
    drop(writing);
    drop(transaction);
    release.send(()).unwrap();
    finished_rx.await.unwrap();
    assert!(matches!(
        persistence.begin().await,
        Err(AuthorizationOwnerError::Busy)
    ));
    let mut storage = persistence.storage.lock().unwrap();
    assert!(matches!(
        storage.commit_flush(&revision, flush(b"old", b"old")),
        FlushCommit::AuthorizationChanged
    ));
    assert_eq!(
        storage
            .store
            .confirm_store(SnapshotRegion::RemoteControlControllerGrants, b"candidate")
            .unwrap(),
        FileStoreConfirmation::Confirmed
    );
}

#[tokio::test]
async fn failed_write_is_distinct_from_lost_worker_and_requires_explicit_finish() {
    let directory = TestDirectory::new();
    let persistence = persistence(&directory);
    let transaction = persistence.begin().await.unwrap();
    let result = transaction
        .store_with(
            SnapshotRegion::RemoteControlTargetAccesses,
            b"candidate".to_vec(),
            |_, _, _| {
                Err(FileStoreError::Io(std::io::Error::other(
                    "before publication",
                )))
            },
            |_, _, _| panic!("definite failure must not confirm"),
        )
        .await
        .unwrap();
    assert!(matches!(result, Err(FileStoreError::Io(_))));
    assert!(matches!(
        persistence.begin().await,
        Err(AuthorizationOwnerError::Busy)
    ));
    transaction.finish().await.unwrap();
    let transaction = persistence.begin().await.unwrap();
    assert!(matches!(
        transaction
            .store_with(
                SnapshotRegion::RemoteControlTargetAccesses,
                vec![],
                |_, _, _| panic!("worker lost"),
                |_, _, _| panic!("worker lost"),
            )
            .await,
        Err(AuthorizationOwnerError::Task)
    ));
    drop(transaction);
    assert!(matches!(
        persistence.begin().await,
        Err(AuthorizationOwnerError::Busy)
    ));
}
