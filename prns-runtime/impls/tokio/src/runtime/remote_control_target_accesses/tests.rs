use super::*;
use crate::identity::{PrivateIdentityMaterial, IDENTITY_SECRET_KEY_LEN};
use crate::persistence::{
    read_remote_control_target_accesses_snapshot, remote_control_target_accesses_snapshot_capacity,
    FileStore, FileStoreError, PersistedStore,
};
use crate::remote_control::{
    RemoteControlControllerAuthority, RemoteControlRequestKind, RemoteControlRequestSet,
    RemoteControlTargetAccessTable, RemoteControlTargetIdentity,
};
use crate::runtime::node_facade::{
    test_remote_control_service, NodePersistence, PrnsNode, TestDirectory,
};
use crate::runtime::{
    ManuallyAttached, PersistenceIo, PersistenceIoCompletion, PersistenceIoOperation,
    PersistenceIoTask, PreConfiguredDestination, PrnsNodeRecipe,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn access(request: RemoteControlRequestKind) -> RemoteControlTargetAccess {
    access_for(0x63, request)
}

fn access_for(seed: u8, request: RemoteControlRequestKind) -> RemoteControlTargetAccess {
    let material = PrivateIdentityMaterial::from_bytes([seed; IDENTITY_SECRET_KEY_LEN]);
    RemoteControlTargetAccess::new(
        RemoteControlTargetIdentity::new(material.public().public_keys()),
        RemoteControlControllerAuthority::Operator,
        RemoteControlRequestSet::only(request),
    )
    .unwrap()
}

fn remote_control() -> AssembledRemoteControl {
    let mut engine = crate::engine::EngineState::<crate::storage::GrowableHeap>::default();
    crate::runtime::configure_remote_control_service(&mut engine, test_remote_control_service())
        .unwrap()
}

#[derive(Default)]
struct IoState {
    fail_next: AtomicBool,
    fail_all: AtomicBool,
    store_attempts: AtomicUsize,
    publish_unconfirmed_next: AtomicBool,
    pause_next: AtomicBool,
    pause_confirmation_next: AtomicBool,
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

struct TestIo(Arc<IoState>);

impl PersistenceIo for TestIo {
    fn execute(&self, task: PersistenceIoTask) -> PersistenceIoCompletion<'_> {
        Box::pin(async move {
            let pause_store = matches!(
                task.operation(),
                PersistenceIoOperation::AuthorizationStore(_)
            ) && self.0.pause_next.swap(false, Ordering::SeqCst);
            let pause_confirmation =
                matches!(
                    task.operation(),
                    PersistenceIoOperation::AuthorizationConfirm(_)
                ) && self.0.pause_confirmation_next.swap(false, Ordering::SeqCst);
            if pause_store || pause_confirmation {
                self.0.reached.notify_one();
                self.0.release.notified().await;
            }
            task.run();
            Ok(())
        })
    }

    fn store_authorization(
        &self,
        store: &mut FileStore,
        region: SnapshotRegion,
        snapshot: &[u8],
    ) -> Result<(), FileStoreError> {
        self.0.store_attempts.fetch_add(1, Ordering::SeqCst);
        if self.0.fail_all.load(Ordering::SeqCst) || self.0.fail_next.swap(false, Ordering::SeqCst)
        {
            return Err(FileStoreError::Io(std::io::Error::other(
                "injected write failure",
            )));
        }
        store.store(region, snapshot)?;
        if self
            .0
            .publish_unconfirmed_next
            .swap(false, Ordering::SeqCst)
        {
            return Err(FileStoreError::PublishedDurabilityUnconfirmed(
                std::io::Error::other("injected post-publication confirmation failure"),
            ));
        }
        Ok(())
    }
}

fn persistence(
    directory: &TestDirectory,
    io: Arc<IoState>,
) -> RemoteControlAuthorizationPersistence {
    let (commands, _) = mpsc::unbounded_channel();
    NodePersistence::custom_dir(directory.path())
        .unwrap()
        .with_io_driver(TestIo(io))
        .worker(PrnsNodeHandle::over(commands))
        .remote_control_authorization_persistence()
}

fn stored(directory: &TestDirectory) -> Vec<RemoteControlTargetAccess> {
    let store = FileStore::new(directory.path());
    let mut bytes = vec![0; remote_control_target_accesses_snapshot_capacity(8)];
    let Some(bytes) = store
        .load(SnapshotRegion::RemoteControlTargetAccesses, &mut bytes)
        .unwrap()
    else {
        return Vec::new();
    };
    read_remote_control_target_accesses_snapshot(bytes)
        .unwrap()
        .collect()
}

async fn save(
    remote: &mut AssembledRemoteControl,
    persistence: Option<&RemoteControlAuthorizationPersistence>,
    request: RemoteControlRequestKind,
) -> Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError> {
    let (completion, settled) = oneshot::channel();
    RemoteControlTargetAccessCommand::SaveTargetAccess {
        access: access(request),
        completion,
    }
    .apply(remote, persistence)
    .await
    .unwrap();
    settled.await.unwrap()
}

#[test]
fn projected_access_is_not_active_before_storage() {
    for replacement in [false, true] {
        let mut remote = remote_control();
        if replacement {
            remote
                .set_target_access(access(RemoteControlRequestKind::Describe))
                .unwrap();
        }
        let prepared =
            prepare_target_access_set(&mut remote, access(RemoteControlRequestKind::AnnounceSelf))
                .unwrap();
        assert_eq!(prepared.is_replacement(), replacement);
        let current = remote
            .target_accesses()
            .unwrap()
            .accesses_in_identity_hash_order();
        if replacement {
            assert_eq!(current, &[access(RemoteControlRequestKind::Describe)]);
        } else {
            assert!(current.is_empty());
        }
        let (_, _, projected) = prepared.into_parts();
        assert_eq!(
            read_remote_control_target_accesses_snapshot(&projected)
                .unwrap()
                .collect::<Vec<_>>(),
            vec![access(RemoteControlRequestKind::AnnounceSelf)]
        );
    }
}

#[tokio::test]
async fn durable_save_requires_persistence_even_for_unchanged_volatile_access() {
    let mut remote = remote_control();
    assert_eq!(
        save(&mut remote, None, RemoteControlRequestKind::Describe).await,
        Err(SaveRemoteControlTargetAccessError::PersistenceUnavailable)
    );
    assert!(remote.target_accesses().unwrap().is_empty());
    remote
        .set_target_access(access(RemoteControlRequestKind::Describe))
        .unwrap();
    assert_eq!(
        save(&mut remote, None, RemoteControlRequestKind::Describe).await,
        Err(SaveRemoteControlTargetAccessError::PersistenceUnavailable)
    );
    let directory = TestDirectory::new();
    let persistence = persistence(&directory, Arc::default());
    assert_eq!(
        save(
            &mut remote,
            Some(&persistence),
            RemoteControlRequestKind::Describe
        )
        .await,
        Ok(SetRemoteControlTargetAccessOutcome::Unchanged)
    );
    assert_eq!(
        stored(&directory),
        vec![access(RemoteControlRequestKind::Describe)]
    );
}

#[tokio::test]
async fn failed_save_retains_previous_disk_and_live_access() {
    let directory = TestDirectory::new();
    let io = Arc::new(IoState::default());
    let persistence = persistence(&directory, Arc::clone(&io));
    let mut remote = remote_control();
    save(
        &mut remote,
        Some(&persistence),
        RemoteControlRequestKind::Describe,
    )
    .await
    .unwrap();
    io.fail_next.store(true, Ordering::SeqCst);
    assert_eq!(
        save(
            &mut remote,
            Some(&persistence),
            RemoteControlRequestKind::AnnounceSelf
        )
        .await,
        Err(SaveRemoteControlTargetAccessError::PersistenceFailed)
    );
    assert_eq!(
        stored(&directory),
        vec![access(RemoteControlRequestKind::Describe)]
    );
    assert_eq!(
        remote
            .target_accesses()
            .unwrap()
            .accesses_in_identity_hash_order(),
        &[access(RemoteControlRequestKind::Describe)]
    );
    // A failed save releases ownership and can be deliberately retried.
    save(
        &mut remote,
        Some(&persistence),
        RemoteControlRequestKind::AnnounceSelf,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn failed_save_never_promotes_unrelated_volatile_access_or_requires_rollback_write() {
    let directory = TestDirectory::new();
    let io = Arc::new(IoState::default());
    let persistence = persistence(&directory, Arc::clone(&io));
    let mut remote = remote_control();
    save(
        &mut remote,
        Some(&persistence),
        RemoteControlRequestKind::Describe,
    )
    .await
    .unwrap();
    remote
        .set_target_access(access(RemoteControlRequestKind::AnnounceSelf))
        .unwrap();
    remote
        .set_target_access(access_for(0x64, RemoteControlRequestKind::Describe))
        .unwrap();
    let volatile = remote
        .target_accesses()
        .unwrap()
        .accesses_in_identity_hash_order()
        .iter()
        .map(|access| {
            (
                access.target().identity_hash(),
                *access.permitted_requests(),
            )
        })
        .collect::<Vec<_>>();
    let baseline_attempts = io.store_attempts.load(Ordering::SeqCst);
    io.fail_all.store(true, Ordering::SeqCst);
    for expected_attempts in 1..=2 {
        let (completion, settled) = oneshot::channel();
        RemoteControlTargetAccessCommand::SaveTargetAccess {
            access: access_for(0x65, RemoteControlRequestKind::Describe),
            completion,
        }
        .apply(&mut remote, Some(&persistence))
        .await
        .expect("a definitive write failure must release ownership without another write");
        assert_eq!(
            settled.await.unwrap(),
            Err(SaveRemoteControlTargetAccessError::PersistenceFailed)
        );
        assert_eq!(
            stored(&directory),
            vec![access(RemoteControlRequestKind::Describe)]
        );
        assert_eq!(
            remote
                .target_accesses()
                .unwrap()
                .accesses_in_identity_hash_order()
                .iter()
                .map(|access| (
                    access.target().identity_hash(),
                    *access.permitted_requests()
                ))
                .collect::<Vec<_>>(),
            volatile
        );
        assert_eq!(
            io.store_attempts.load(Ordering::SeqCst),
            baseline_attempts + expected_attempts
        );
    }
}

#[tokio::test]
async fn published_save_waits_for_confirmation_without_rollback_or_an_early_success() {
    let directory = TestDirectory::new();
    let io = Arc::new(IoState::default());
    let persistence = persistence(&directory, Arc::clone(&io));
    let mut remote = remote_control();
    io.publish_unconfirmed_next.store(true, Ordering::SeqCst);
    io.pause_confirmation_next.store(true, Ordering::SeqCst);
    let (completion, mut settled) = oneshot::channel();
    let applying = RemoteControlTargetAccessCommand::SaveTargetAccess {
        access: access(RemoteControlRequestKind::Describe),
        completion,
    }
    .apply(&mut remote, Some(&persistence));
    tokio::pin!(applying);
    tokio::select! {
        result = &mut applying => panic!("save completed before confirmation: {result:?}"),
        () = io.reached.notified() => {}
    }
    assert_eq!(
        stored(&directory),
        vec![access(RemoteControlRequestKind::Describe)]
    );
    assert_eq!(settled.try_recv(), Err(oneshot::error::TryRecvError::Empty));
    io.release.notify_one();
    applying.await.unwrap();
    assert_eq!(
        settled.await.unwrap(),
        Ok(SetRemoteControlTargetAccessOutcome::Added)
    );
    assert_eq!(io.store_attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn admitted_save_survives_cancelled_waiter_and_actual_node_restart() {
    let directory = TestDirectory::new();
    let io = Arc::new(IoState::default());
    let persistence = persistence(&directory, Arc::clone(&io));
    let mut remote = remote_control();
    let (completion, settled) = oneshot::channel();
    io.pause_next.store(true, Ordering::SeqCst);
    let applying = RemoteControlTargetAccessCommand::SaveTargetAccess {
        access: access(RemoteControlRequestKind::Describe),
        completion,
    }
    .apply(&mut remote, Some(&persistence));
    tokio::pin!(applying);
    tokio::select! {
        result = &mut applying => panic!("save completed before storage: {result:?}"),
        () = io.reached.notified() => {}
    }
    assert!(stored(&directory).is_empty());
    drop(settled);
    io.release.notify_one();
    applying.await.unwrap();
    assert_eq!(
        stored(&directory),
        vec![access(RemoteControlRequestKind::Describe)]
    );

    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: test_remote_control_service().into(),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: crate::runtime::NoRemoteControlHostControls,
        storage: crate::storage::GrowableHeap,
        request_endpoints: crate::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NodePersistence::custom_dir(directory.path()).unwrap(),
        on_event: |_, _: &crate::runtime::NoRemoteControlHostControls| {},
    });
    let handle = node.handle();
    let target = access(RemoteControlRequestKind::Describe)
        .target()
        .identity_hash();
    tokio::select! {
        resolved = handle.resolve_remote_control_target(target) => {
            assert_eq!(*resolved.unwrap().permitted_requests(), RemoteControlRequestSet::only(RemoteControlRequestKind::Describe));
        }
        result = node.run() => panic!("restored node stopped: {result:?}"),
    }
}

#[tokio::test]
async fn card_import_checks_live_recipient_and_refuses_silent_replacement() {
    let directory = TestDirectory::new();
    let persistence = persistence(&directory, Arc::default());
    let mut remote = remote_control();
    let recipient = *remote.identities().unwrap().controller();
    let other = RemoteControlControllerIdentity::new(
        PrivateIdentityMaterial::from_bytes([0x64; IDENTITY_SECRET_KEY_LEN])
            .public()
            .public_keys(),
    );
    for (controller, request, expected) in [
        (
            other,
            RemoteControlRequestKind::Describe,
            Err(SaveRemoteControlTargetAccessError::RecipientMismatch),
        ),
        (
            recipient,
            RemoteControlRequestKind::Describe,
            Ok(SetRemoteControlTargetAccessOutcome::Added),
        ),
        (
            recipient,
            RemoteControlRequestKind::Describe,
            Ok(SetRemoteControlTargetAccessOutcome::Unchanged),
        ),
        (
            recipient,
            RemoteControlRequestKind::AnnounceSelf,
            Err(SaveRemoteControlTargetAccessError::Conflict),
        ),
    ] {
        let card = RemoteControlTargetAccessCard::new(controller, access(request)).unwrap();
        let (completion, settled) = oneshot::channel();
        RemoteControlTargetAccessCommand::SaveTargetAccessCard { card, completion }
            .apply(&mut remote, Some(&persistence))
            .await
            .unwrap();
        assert_eq!(settled.await.unwrap(), expected);
    }
    assert_eq!(
        stored(&directory),
        vec![access(RemoteControlRequestKind::Describe)]
    );
    assert_eq!(
        remote
            .target_accesses()
            .unwrap()
            .accesses_in_identity_hash_order(),
        &[access(RemoteControlRequestKind::Describe)]
    );
}

#[tokio::test]
async fn public_identity_query_does_not_need_any_saved_target() {
    let mut remote = remote_control();
    let expected = *remote.identities().unwrap().controller();
    let (commands, _) = mpsc::unbounded_channel();
    let (handle, mut receiver) =
        PrnsNodeHandle::over_with_remote_control_target_access_lane(commands);
    let (identity, ()) = tokio::join!(
        super::super::RemoteControlTargetAccessPersistence::remote_control_controller_identity(
            &handle
        ),
        async {
            receiver
                .receive()
                .await
                .unwrap()
                .apply(&mut remote, None)
                .await
                .unwrap();
        }
    );
    assert_eq!(identity.unwrap(), expected);
    assert!(remote.target_accesses().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_after_enqueue_does_not_discard_admitted_save() {
    let directory = TestDirectory::new();
    let persistence = persistence(&directory, Arc::default());
    let mut remote = remote_control();
    let (commands, _) = mpsc::unbounded_channel();
    let (handle, mut receiver) =
        PrnsNodeHandle::over_with_remote_control_target_access_lane(commands);
    let saving = tokio::spawn(async move {
        handle
            .save_remote_control_target_access(access(RemoteControlRequestKind::Describe))
            .await
    });
    let command = receiver.receive().await.unwrap();
    saving.abort();
    assert!(saving.await.is_err());
    command
        .apply(&mut remote, Some(&persistence))
        .await
        .unwrap();
    assert_eq!(
        stored(&directory),
        vec![access(RemoteControlRequestKind::Describe)]
    );
}
