#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::panic)]
#![allow(
    clippy::result_large_err,
    reason = "Concurrent bootstrap fixtures preserve the complete typed cleanup failure from each worker"
)]
use prns_chat_app::bootstrap::{AppBootstrapError, Registry};
use prns_core::identity::IdentityHash;
use prns_host_runtime::HostEmbedding;
use prns_service_runtime_uniffi::{
    identities::KeyError,
    identity::{keys::KeyOwner, Error as KeyFailure},
    runtime::groups::standalone::Phase,
};
use prns_storage::files;
use std::{
    future::Future,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    },
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};
struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}
fn wait<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return value;
        }
        assert!(Instant::now() < deadline, "app bootstrap stalled");
        std::thread::park_timeout(Duration::from_millis(5));
    }
}
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "prns-app-bootstrap-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        files::create_private_directory(&path).unwrap();
        Self(path)
    }
    fn controller(&self) -> PathBuf {
        self.0.join("v2/controller")
    }
    fn public_hash(&self) -> IdentityHash {
        let marker = std::fs::read(self.0.join("v2/installation")).unwrap();
        assert_eq!(marker.len(), 44);
        IdentityHash::new(marker[12..28].try_into().unwrap())
    }
    fn target(&self) -> PathBuf {
        self.0.join("v2/management-target")
    }
    fn target_hash(&self) -> IdentityHash {
        let marker = std::fs::read(self.0.join("v2/installation")).unwrap();
        IdentityHash::new(marker[28..44].try_into().unwrap())
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn concurrent_start_and_js_reload_share_one_owner_then_reopen_the_same_controller() {
    let root = Root::new();
    let registry = Arc::new(Registry::with_initial_connections(|| Ok(vec![])));
    let operations = registry.remote_control_operations().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let registry = registry.clone();
            let barrier = barrier.clone();
            let path = root.0.clone();
            std::thread::spawn(move || {
                barrier.wait();
                registry.start_new(path, HostEmbedding::default())
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    let owner = registry.owner().unwrap();
    for result in results {
        match result {
            Ok(candidate) => assert!(Arc::ptr_eq(&candidate, &owner)),
            Err(AppBootstrapError::StartInProgress) => {}
            Err(error) => panic!("unexpected concurrent start: {error}"),
        }
    }
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    assert!(wait(owner.identities().unwrap().identities())
        .unwrap()
        .is_empty());
    let expected = root.public_hash();
    let target = root.target_hash();
    assert_ne!(expected, target);
    use prns_service_runtime_uniffi::host::remote_control::{
        remote_control_controller_identity_hash, RemoteControlControllerIdentitySettlement,
    };
    let RemoteControlControllerIdentitySettlement::Completed { value } =
        wait(owner.host().unwrap().remote_control_controller_identity()).unwrap()
    else {
        panic!("product management service unavailable")
    };
    assert_eq!(
        remote_control_controller_identity_hash(value)
            .unwrap()
            .value,
        expected.as_bytes()
    );
    assert_eq!(
        owner.host().unwrap().identity_hash().unwrap().as_slice(),
        expected.as_bytes()
    );
    assert!(matches!(
        KeyOwner::open_existing(root.controller(), expected),
        Err(KeyFailure::Busy)
    ));
    assert!(matches!(
        KeyOwner::open_existing(root.target(), target),
        Err(KeyFailure::Busy)
    ));
    // Releasing a borrowed JS-facing handle does not close the process owner.
    drop(owner);
    let owner = registry.owner().unwrap();
    assert_eq!(owner.status().phase, Phase::Ready);
    let duplicate = registry
        .open_existing(&root.0, HostEmbedding::default())
        .unwrap();
    assert!(Arc::ptr_eq(&duplicate, &owner));
    let other = Root::new();
    assert!(matches!(
        registry.start_new(&other.0, HostEmbedding::default()),
        Err(AppBootstrapError::DifferentInstallation)
    ));
    assert!(!other.0.join("v2").exists());
    let borrowed = owner.identities().unwrap();
    wait(registry.shutdown().unwrap().wait()).unwrap();
    assert!(wait(borrowed.identities()).is_err());
    assert!(owner.status().cleanup_complete);
    let key = KeyOwner::open_existing(root.controller(), expected).unwrap();
    assert_eq!(key.identity_hash(), expected);
    drop(key);
    drop(KeyOwner::open_existing(root.target(), target).unwrap());
    let reopened = registry
        .open_existing(&root.0, HostEmbedding::default())
        .unwrap();
    assert!(!Arc::ptr_eq(&reopened, &owner));
    assert!(Arc::ptr_eq(
        &operations,
        &registry.remote_control_operations().unwrap()
    ));
    wait(reopened.ready()).unwrap();
    wait(reopened.network_ready()).unwrap();
    assert_eq!(
        reopened.host().unwrap().identity_hash().unwrap().as_slice(),
        expected.as_bytes()
    );
    assert!(wait(reopened.identities().unwrap().identities())
        .unwrap()
        .is_empty());
    assert_eq!(root.target_hash(), target);
    wait(registry.shutdown().unwrap().wait()).unwrap();
}

#[test]
fn strict_open_preserves_wrong_markers_missing_keys_and_partial_installations() {
    let root = Root::new();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    let owner = registry
        .start_new(&root.0, HostEmbedding::default())
        .unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    wait(registry.shutdown().unwrap().wait()).unwrap();
    let expected = root.public_hash();
    let marker_path = root.0.join("v2/installation");
    let original = std::fs::read(&marker_path).unwrap();
    let mut marker = original.clone();
    marker[8..12].copy_from_slice(&3_u32.to_le_bytes());
    std::fs::write(&marker_path, &marker).unwrap();
    assert!(matches!(
        registry.open_existing(&root.0, HostEmbedding::default()),
        Err(AppBootstrapError::UnsupportedInstallation { version: 3 })
    ));
    assert_eq!(std::fs::read(&marker_path).unwrap(), marker);
    marker = original.clone();
    marker.push(0);
    std::fs::write(&marker_path, &marker).unwrap();
    assert!(matches!(
        registry.open_existing(&root.0, HostEmbedding::default()),
        Err(AppBootstrapError::InvalidInstallation)
    ));
    assert_eq!(std::fs::read(&marker_path).unwrap(), marker);
    marker = original.clone();
    marker[12] ^= 1;
    std::fs::write(&marker_path, &marker).unwrap();
    assert!(matches!(
        registry.open_existing(&root.0, HostEmbedding::default()),
        Err(AppBootstrapError::ControllerKey {
            error: KeyError::IdentityMismatch
        })
    ));
    assert_eq!(
        KeyOwner::open_existing(root.controller(), expected)
            .unwrap()
            .identity_hash(),
        expected
    );
    marker = original.clone();
    marker[28] ^= 1;
    std::fs::write(&marker_path, &marker).unwrap();
    assert!(matches!(
        registry.open_existing(&root.0, HostEmbedding::default()),
        Err(AppBootstrapError::ManagementTargetKey {
            error: KeyError::IdentityMismatch
        })
    ));
    assert_eq!(std::fs::read(&marker_path).unwrap(), marker);
    std::fs::write(&marker_path, &original).unwrap();
    let target_path = root.target().join("identity.key");
    let target_key = std::fs::read(&target_path).unwrap();
    std::fs::remove_file(&target_path).unwrap();
    assert!(matches!(
        registry.open_existing(&root.0, HostEmbedding::default()),
        Err(AppBootstrapError::ManagementTargetKey {
            error: KeyError::Missing
        })
    ));
    assert!(
        !target_path.exists(),
        "reopen regenerated management target key"
    );
    std::fs::write(&target_path, &target_key).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&target_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let key_path = root.controller().join("identity.key");
    std::fs::remove_file(&key_path).unwrap();
    assert!(matches!(
        registry.open_existing(&root.0, HostEmbedding::default()),
        Err(AppBootstrapError::ControllerKey {
            error: KeyError::Missing
        })
    ));
    assert!(!key_path.exists(), "reopen regenerated controller key");
    assert_eq!(std::fs::read(&marker_path).unwrap(), original);
    assert!(registry
        .start_new(&root.0, HostEmbedding::default())
        .is_err());
    assert!(
        !key_path.exists(),
        "create on existing installation reset it"
    );

    let partial = Root::new();
    files::create_private_directory(&partial.0.join("v2")).unwrap();
    let key = KeyOwner::create_new(partial.controller()).unwrap();
    let expected = key.identity_hash();
    drop(key);
    let incomplete = Registry::with_initial_connections(|| Ok(vec![]));
    assert!(matches!(
        incomplete.open_existing(&partial.0, HostEmbedding::default()),
        Err(AppBootstrapError::MissingInstallation)
    ));
    assert!(incomplete
        .start_new(&partial.0, HostEmbedding::default())
        .is_err());
    assert_eq!(
        KeyOwner::open_existing(partial.controller(), expected)
            .unwrap()
            .identity_hash(),
        expected
    );
    assert!(!partial.0.join("v2/installation").exists());
}

#[test]
fn startup_caller_loss_and_stop_keep_controller_locked_until_actual_cleanup() {
    let root = Root::new();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let embedding = HostEmbedding {
        prepare_interfaces: Some(Box::new(move |_| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(vec![])
        })),
        ..HostEmbedding::default()
    };
    let owner = registry.start_new(&root.0, embedding).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let expected = root.public_hash();
    let mut cancelled = Box::pin(owner.network_ready());
    assert!(cancelled
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    drop(cancelled);
    let close = registry.shutdown().unwrap();
    drop(close);
    assert!(matches!(
        registry.open_existing(&root.0, HostEmbedding::default()),
        Err(AppBootstrapError::CleanupPending { .. })
    ));
    assert!(matches!(
        KeyOwner::open_existing(root.controller(), expected),
        Err(KeyFailure::Busy)
    ));
    release_tx.send(()).unwrap();
    wait(registry.shutdown().unwrap().wait()).unwrap();
    assert!(owner.status().cleanup_complete);
    drop(KeyOwner::open_existing(root.controller(), expected).unwrap());
    // Local storage opens before the blocked host, and remains a valid strict
    // installation after startup is cancelled and all key leases drain.
    assert!(root.0.join("v2/identities").exists());
    let reopened = registry
        .open_existing(&root.0, HostEmbedding::default())
        .unwrap();
    wait(reopened.ready()).unwrap();
    wait(reopened.network_ready()).unwrap();
    wait(registry.shutdown().unwrap().wait()).unwrap();
    assert!(reopened.status().cleanup_complete);
    assert!(root.0.join("v2/identities").exists());
    assert_eq!(
        KeyOwner::open_existing(root.controller(), expected)
            .unwrap()
            .identity_hash(),
        expected
    );
}

#[test]
fn product_reopen_restores_saved_enablement_without_a_js_recovery_call() {
    use prns_service_runtime_uniffi::{
        identity::{api, creation::CreateRequestId},
        identity_core as core,
    };
    let root = Root::new();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    let owner = registry
        .start_new(&root.0, HostEmbedding::default())
        .unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let catalog = owner.identities().unwrap();
    wait(catalog.create_or_recover(
        CreateRequestId {
            scope: wait(catalog.creation_scope()).unwrap(),
            nonce: [71; 32],
        },
        api::CreateRequest {
            source: core::KeySource::Generate,
            metadata: core::Metadata::new("saved messaging identity", None).unwrap(),
            services: vec![],
        },
        None,
    ))
    .unwrap();
    let reference = wait(catalog.identities()).unwrap()[0].reference();
    let identity = catalog.identity(reference).unwrap();
    for target in [
        core::TransitionTarget::AddService(prns_service_runtime_uniffi::runtime::groups::SERVICE),
        core::TransitionTarget::Enable,
    ] {
        let deadline = Instant::now() + Duration::from_secs(10);
        let prepared = loop {
            match wait(identity.prepare(target)) {
                Ok(prepared) => break prepared,
                Err(error) => {
                    assert!(Instant::now() < deadline, "prepare stalled: {error:?}");
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        };
        let token = wait(catalog.reserve(prepared.clone(), core::CommandDigest([72; 32]))).unwrap();
        wait(identity.begin(token, prepared.clone())).unwrap();
        loop {
            if matches!(
                wait(catalog.operation(token, api::Command::Transition(prepared.clone()))).unwrap(),
                core::OperationState::Completed(_)
            ) {
                break;
            }
            assert!(Instant::now() < deadline, "accepted transition stalled");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let before = wait(identity.snapshot()).unwrap();
    assert_eq!(before.runtime, core::RuntimeStatus::Running);
    let controller = root.public_hash();
    wait(registry.shutdown().unwrap().wait()).unwrap();
    assert!(wait(identity.snapshot()).is_err());
    let reopened = registry
        .open_existing(&root.0, HostEmbedding::default())
        .unwrap();
    wait(reopened.ready()).unwrap();
    wait(reopened.network_ready()).unwrap();
    let restored = reopened.identities().unwrap().identity(reference).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = wait(restored.snapshot()).unwrap();
        assert_eq!(snapshot.identity, before.identity);
        assert!(snapshot.last_error.is_none());
        if snapshot.runtime == core::RuntimeStatus::Running {
            break;
        }
        assert!(Instant::now() < deadline, "saved identity did not restart");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(root.public_hash(), controller);
    let borrowed = registry.owner().unwrap();
    assert!(Arc::ptr_eq(&borrowed, &reopened));
    drop(reopened);
    assert_eq!(
        wait(restored.snapshot()).unwrap().runtime,
        core::RuntimeStatus::Running
    );
    assert_eq!(borrowed.status().phase, Phase::Ready);
    wait(registry.shutdown().unwrap().wait()).unwrap();
    assert!(borrowed.status().cleanup_complete);
}

fn offline_presets() -> std::result::Result<
    Vec<prns_host_runtime::connections::Profile>,
    prns_host_runtime::connections::Error,
> {
    let mut profiles = prns_chat_app::policy::initial_connection_profiles()?;
    for profile in &mut profiles {
        profile.enabled = false;
    }
    Ok(profiles)
}

#[test]
fn connection_settings_remain_editable_after_stop_and_removed_presets_stay_removed() {
    let root = Root::new();
    let registry = Registry::with_initial_connections(offline_presets);
    let owner = registry
        .start_new(&root.0, HostEmbedding::default())
        .unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let settings = registry.connection_profiles().unwrap();
    let initial = wait(settings.snapshot()).unwrap();
    assert_eq!(initial.profiles.len(), 2);
    assert!(initial.profiles.iter().all(|profile| !profile.enabled));
    let generation = owner.connection_binding().unwrap().status().generation;
    wait(registry.shutdown().unwrap().wait()).unwrap();
    // No host is running, but settings and the exact revision guard still work.
    let empty = wait(settings.replace(initial.guard, vec![])).unwrap();
    assert!(empty.profiles.is_empty());
    let reopened = registry
        .open_existing(&root.0, HostEmbedding::default())
        .unwrap();
    wait(reopened.ready()).unwrap();
    wait(reopened.network_ready()).unwrap();
    assert_ne!(
        reopened.connection_binding().unwrap().status().generation,
        generation
    );
    assert!(wait(settings.snapshot()).unwrap().profiles.is_empty());
    wait(registry.shutdown().unwrap().wait()).unwrap();
    drop(reopened);
    drop(owner);
    drop(registry);
    // A borrowed settings handle does not keep the file owner alive. Its drop
    // fences immediately; the retained native worker completes file cleanup.
    assert!(wait(settings.snapshot()).is_err());
    let next = Registry::with_initial_connections(|| panic!("strict reopen inserted presets"));
    let deadline = Instant::now() + Duration::from_secs(5);
    let owner = loop {
        match next.open_existing(&root.0, HostEmbedding::default()) {
            Ok(owner) => break owner,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "profile owner did not release: {error}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let saved = wait(next.connection_profiles().unwrap().snapshot()).unwrap();
    assert_eq!(saved.guard.revision, empty.guard.revision);
    assert_eq!(saved.guard.incarnation, empty.guard.incarnation);
    assert!(saved.profiles.is_empty());
    wait(next.shutdown().unwrap().wait()).unwrap();
}

#[test]
fn network_stop_preserves_the_installation_and_local_clients() {
    let root = Root::new();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    let owner = registry
        .start_new(&root.0, HostEmbedding::default())
        .unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let catalog = owner.identities().unwrap();
    let generation = owner.network_status().generation;
    wait(registry.stop().unwrap().wait()).unwrap();
    assert_eq!(owner.status().phase, Phase::Ready);
    assert!(wait(catalog.identities()).unwrap().is_empty());
    assert!(!owner.status().cleanup_complete);
    let resumed = registry
        .open_existing(&root.0, HostEmbedding::default())
        .unwrap();
    assert!(Arc::ptr_eq(&owner, &resumed));
    wait(resumed.network_ready()).unwrap();
    assert!(resumed.network_status().generation > generation);
    assert!(wait(catalog.identities()).unwrap().is_empty());
    wait(registry.shutdown().unwrap().wait()).unwrap();
    assert!(wait(catalog.identities()).is_err());
}

#[test]
fn explicit_reset_drains_all_owners_and_preserves_unrelated_private_files() {
    let root = Root::new();
    std::fs::write(root.0.join("platform-preferences"), b"preserve").unwrap();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    let owner = registry
        .start_new(&root.0, HostEmbedding::default())
        .unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let catalog = owner.identities().unwrap();
    let profiles = registry.connection_profiles().unwrap();
    let operations = registry.remote_control_operations().unwrap();
    let review = registry.prepare_reset().unwrap();
    assert!(root.0.join("v2/installation").exists());
    wait(registry.reset(review.clone())).unwrap();
    assert!(owner.status().cleanup_complete);
    assert!(matches!(
        registry.owner(),
        Err(AppBootstrapError::NotStarted)
    ));
    assert!(wait(catalog.identities()).is_err());
    assert!(wait(profiles.snapshot()).is_err());
    assert!(!root.0.join("v2").exists());
    assert_eq!(
        std::fs::read(root.0.join("platform-preferences")).unwrap(),
        b"preserve"
    );
    wait(registry.reset(review.clone())).unwrap();
    let new_operations = registry.remote_control_operations().unwrap();
    assert!(!Arc::ptr_eq(&operations, &new_operations));
    let fresh = registry
        .start_new(&root.0, HostEmbedding::default())
        .unwrap();
    wait(fresh.ready()).unwrap();
    assert!(matches!(
        wait(registry.reset(review)),
        Err(AppBootstrapError::StaleResetReview)
    ));
    wait(registry.shutdown().unwrap().wait()).unwrap();
}

#[test]
fn explicit_reset_can_remove_reviewed_partial_creation_before_owner_registration() {
    let root = Root::new();
    files::create_private_directory(&root.0.join("v2")).unwrap();
    std::fs::write(root.0.join("v2/partial"), b"initial creation").unwrap();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    assert!(registry.prepare_reset().is_err());
    assert!(registry
        .open_existing(&root.0, HostEmbedding::default())
        .is_err());
    assert!(matches!(
        registry.owner(),
        Err(AppBootstrapError::NotStarted)
    ));
    let review = registry.prepare_reset().unwrap();
    wait(registry.reset(review)).unwrap();
    assert!(!root.0.join("v2").exists());
    let fresh = registry
        .start_new(&root.0, HostEmbedding::default())
        .unwrap();
    wait(fresh.ready()).unwrap();
    wait(registry.shutdown().unwrap().wait()).unwrap();
}

#[test]
fn reset_rejects_replaced_directory_and_marker_without_deleting_files() {
    let root = Root::new();
    files::create_private_directory(&root.0.join("v2")).unwrap();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    assert!(registry
        .open_existing(&root.0, HostEmbedding::default())
        .is_err());
    let review = registry.prepare_reset().unwrap();
    std::fs::rename(root.0.join("v2"), root.0.join("previous")).unwrap();
    files::create_private_directory(&root.0.join("v2")).unwrap();
    std::fs::write(root.0.join("v2/keep"), b"different").unwrap();
    assert!(matches!(
        wait(registry.reset(review)),
        Err(AppBootstrapError::StaleResetReview)
    ));
    assert!(root.0.join("v2/keep").exists());
    let review = registry.prepare_reset().unwrap();
    std::fs::write(root.0.join("v2/installation"), b"changed").unwrap();
    assert!(matches!(
        wait(registry.reset(review)),
        Err(AppBootstrapError::StaleResetReview)
    ));
    assert!(root.0.join("v2/keep").exists());
}

#[test]
fn reset_filesystem_error_preserves_marker_and_same_review_can_retry() {
    use std::os::unix::fs::PermissionsExt;
    let root = Root::new();
    files::create_private_directory(&root.0.join("v2")).unwrap();
    let child = root.0.join("v2/blocked");
    files::create_private_directory(&child).unwrap();
    std::fs::write(child.join("data"), b"kept until retry").unwrap();
    std::fs::write(root.0.join("v2/installation"), b"partial marker").unwrap();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    assert!(registry
        .open_existing(&root.0, HostEmbedding::default())
        .is_err());
    let review = registry.prepare_reset().unwrap();
    std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o500)).unwrap();
    let failed = wait(registry.reset(review.clone()));
    std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(failed.is_err());
    assert!(root.0.join("v2/installation").exists());
    wait(registry.reset(review)).unwrap();
    assert!(!root.0.join("v2").exists());
}

#[test]
fn cancelled_reset_wait_keeps_files_and_blocks_replacement_until_cleanup() {
    let root = Root::new();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let embedding = HostEmbedding {
        prepare_interfaces: Some(Box::new(move |_| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(vec![])
        })),
        ..HostEmbedding::default()
    };
    let owner = registry.start_new(&root.0, embedding).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let review = registry.prepare_reset().unwrap();
    let mut reset = Box::pin(registry.reset(review.clone()));
    assert!(reset
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    assert!(root.0.join("v2/installation").exists());
    assert!(matches!(
        registry.open_existing(&root.0, HostEmbedding::default()),
        Err(AppBootstrapError::StartInProgress)
    ));
    drop(reset);
    assert!(root.0.join("v2/installation").exists());
    release_tx.send(()).unwrap();
    wait(registry.reset(review)).unwrap();
    assert!(owner.status().cleanup_complete);
    assert!(!root.0.join("v2").exists());
}

#[test]
fn partial_reset_review_is_stale_after_another_failed_construction_admission() {
    let root = Root::new();
    files::create_private_directory(&root.0.join("v2")).unwrap();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    assert!(registry
        .open_existing(&root.0, HostEmbedding::default())
        .is_err());
    let review = registry.prepare_reset().unwrap();
    assert!(registry
        .start_new(&root.0, HostEmbedding::default())
        .is_err());
    assert!(matches!(
        wait(registry.reset(review)),
        Err(AppBootstrapError::StaleResetReview)
    ));
    wait(registry.reset(registry.prepare_reset().unwrap())).unwrap();
}

#[test]
fn local_preparation_is_readable_before_optional_platform_network_admission() {
    use prns_service_runtime_uniffi::runtime::groups::standalone::NetworkPhase;
    let root = Root::new();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    let local = registry.prepare_new(&root.0).unwrap();
    wait(local.ready()).unwrap();
    assert_eq!(local.network_status().phase, NetworkPhase::Offline);
    assert_eq!(local.network_status().generation, 0);
    assert!(wait(local.identities().unwrap().identities())
        .unwrap()
        .is_empty());
    // If OS service admission refuses, the platform never makes the following
    // optional networking call. The same local owner remains published/readable.
    assert!(Arc::ptr_eq(&local, &registry.owner().unwrap()));
    let joined = registry.prepare_existing(&root.0).unwrap();
    assert!(Arc::ptr_eq(&local, &joined));
    let calls = Arc::new(AtomicUsize::new(0));
    let prepared = calls.clone();
    let online = registry
        .open_existing(
            &root.0,
            HostEmbedding {
                prepare_interfaces: Some(Box::new(move |_| {
                    prepared.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![])
                })),
                ..HostEmbedding::default()
            },
        )
        .unwrap();
    assert!(Arc::ptr_eq(&local, &online));
    wait(online.network_ready()).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(online.network_status().generation, 1);
    wait(registry.shutdown().unwrap().wait()).unwrap();
}

#[test]
fn network_start_cannot_report_offline_success_while_local_preparation_owns_admission() {
    use std::sync::{Mutex, OnceLock};
    type Gate = (
        std::sync::mpsc::SyncSender<()>,
        std::sync::mpsc::Receiver<()>,
    );
    static GATE: OnceLock<Mutex<Option<Gate>>> = OnceLock::new();
    fn profiles(
    ) -> Result<Vec<prns_host_runtime::connections::Profile>, prns_host_runtime::connections::Error>
    {
        let gate = GATE.get().unwrap().lock().unwrap().take().unwrap();
        gate.0.send(()).unwrap();
        gate.1.recv().unwrap();
        Ok(vec![])
    }
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    *GATE.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some((entered_tx, release_rx));
    let root = Root::new();
    let registry = Arc::new(Registry::with_initial_connections(profiles));
    let prepare = {
        let registry = registry.clone();
        let path = root.0.clone();
        std::thread::spawn(move || registry.prepare_new(path))
    };
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        registry.start_new(&root.0, HostEmbedding::default()),
        Err(AppBootstrapError::StartInProgress)
    ));
    release_tx.send(()).unwrap();
    let local = prepare.join().unwrap().unwrap();
    wait(local.ready()).unwrap();
    assert_eq!(local.network_status().generation, 0);
    let online = registry
        .open_existing(&root.0, HostEmbedding::default())
        .unwrap();
    assert!(Arc::ptr_eq(&local, &online));
    wait(online.network_ready()).unwrap();
    assert_eq!(online.network_status().generation, 1);
    wait(registry.shutdown().unwrap().wait()).unwrap();
}

#[test]
fn trusted_native_root_allows_fresh_reset_review_without_starting_damaged_data() {
    let root = Root::new();
    files::create_private_directory(&root.0.join("v2")).unwrap();
    std::fs::write(root.0.join("v2/installation"), b"damaged marker").unwrap();
    std::fs::write(root.0.join("keep"), b"native preferences").unwrap();
    let registry =
        Registry::with_initial_connections(|| panic!("reset capture must not construct profiles"));
    registry.capture_reset_root(&root.0).unwrap();
    assert!(matches!(
        registry.owner(),
        Err(AppBootstrapError::NotStarted)
    ));
    let other = Root::new();
    files::create_private_directory(&other.0.join("v2")).unwrap();
    assert!(matches!(
        registry.capture_reset_root(&other.0),
        Err(AppBootstrapError::DifferentInstallation)
    ));
    let review = registry.prepare_reset().unwrap();
    wait(registry.reset(review)).unwrap();
    assert!(!root.0.join("v2").exists());
    assert!(root.0.join("keep").exists());
}
