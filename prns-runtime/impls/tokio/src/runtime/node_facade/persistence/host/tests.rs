use super::super::TestDirectory;
use super::*;

#[cfg(unix)]
#[test]
fn custom_directory_preparation_stays_inside_the_owned_namespace(
) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use crate::persistence::{PersistedStore, SnapshotRegion};

    struct RestorePermissions(PathBuf);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
        }
    }

    let temporary = TestDirectory::new();
    let ancestor = temporary.path().join("execute-only");
    let existing = ancestor.join("existing-store");
    let application = ancestor.join("owned-application");
    fs::create_dir_all(&existing)?;
    fs::create_dir_all(&application)?;
    // Restore access before TestDirectory's recursive cleanup, including on failure.
    let _permissions = RestorePermissions(ancestor.clone());
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o111))?;
    match fs::File::open(&ancestor) {
        Ok(_) => {
            eprintln!(
                "sandbox persistence fixture skipped: directory read restrictions are bypassed"
            );
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
        Err(error) => return Err(error.into()),
    }

    for directory in [existing, application.join("new/nested-store")] {
        let mut persistence = NodePersistence::custom_dir(&directory)?;
        assert!(directory.is_dir());
        assert!(!directory.join(WRITE_PROBE).exists());
        persistence
            .store
            .store(SnapshotRegion::Timebase, b"sandbox snapshot")?;
        assert_eq!(fs::read(directory.join("timebase"))?, b"sandbox snapshot");
    }
    Ok(())
}

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
