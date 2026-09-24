use super::super::TestDirectory;
use super::*;

#[tokio::test]
async fn owned_authorization_defers_background_flush_but_refuses_shutdown_success() {
    let directory = TestDirectory::new();
    let (commands, mut receiver) = mpsc::unbounded_channel();
    let handle = PrnsNodeHandle::over(commands);
    let worker = NodePersistence::custom_dir(directory.path())
        .unwrap()
        .worker(handle.clone());
    let persistence = worker.remote_control_authorization_persistence();
    let transaction = persistence.begin().await.unwrap();
    let mut events = 0;
    let deferred = flush_state(
        &handle,
        &worker.storage,
        &worker.io,
        PersistenceTrigger::Interval,
        &mut |_| events += 1,
    )
    .await;
    assert!(matches!(deferred, StateFlush::Deferred));
    assert!(!deferred.should_exit(FlushFailurePolicy::Exit));
    assert_eq!(deferred.required(), PersistenceFlushStatus::Failed);
    assert_eq!(events, 0);
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    transaction.finish().await.unwrap();
}
