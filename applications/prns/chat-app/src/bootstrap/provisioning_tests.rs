#![allow(clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
use super::*;
use prns_service_runtime_uniffi::identity::keys::KeyOwner;
use prns_storage::files;
use std::{
    future::Future,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "prns-phone-provisioning-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        files::create_private_directory(&path).unwrap();
        Self(path)
    }
    fn review(&self, bytes: &[u8]) -> Arc<PhoneIdentityImport> {
        let path = self.0.join("selected.key");
        std::fs::write(&path, bytes).unwrap();
        app_review_phone_identity_file(path.to_str().unwrap().to_owned()).unwrap()
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
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
        assert!(Instant::now() < deadline, "phone provisioning stalled");
        std::thread::park_timeout(Duration::from_millis(5));
    }
}

#[test]
fn review_accepts_only_exact_regular_native_files() {
    let root = Root::new();
    let path = root.0.join("selected.key");
    for size in [0, 63, 65, 4096] {
        std::fs::write(&path, vec![42; size]).unwrap();
        assert!(matches!(
            app_review_phone_identity_file(path.to_str().unwrap().to_owned()),
            Err(PhoneIdentityImportError::InvalidKeyFile)
        ));
    }
    assert!(matches!(
        app_review_phone_identity_file(root.0.to_str().unwrap().to_owned()),
        Err(PhoneIdentityImportError::InvalidPath)
    ));
    assert!(matches!(
        app_review_phone_identity_file("relative.key".into()),
        Err(PhoneIdentityImportError::InvalidPath)
    ));
    std::fs::write(&path, [42; 64]).unwrap();
    let link = root.0.join("alias.key");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(matches!(
        app_review_phone_identity_file(link.to_str().unwrap().to_owned()),
        Err(PhoneIdentityImportError::InvalidPath)
    ));
    let reviewed = app_review_phone_identity_file(path.to_str().unwrap().to_owned()).unwrap();
    assert_eq!(
        reviewed.identity_hash(),
        PrivateIdentityMaterial::from_bytes([42; 64])
            .identity_hash()
            .as_bytes()
    );
}

#[test]
fn reviewed_controller_survives_source_change_and_reopen_with_fresh_target() {
    let root = Root::new();
    let reviewed = root.review(&[42; 64]);
    let expected = reviewed.hash();
    std::fs::write(root.0.join("selected.key"), [43; 64]).unwrap();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    let owner = registry
        .start_imported(&root.0, &reviewed, HostEmbedding::default())
        .unwrap();
    wait(owner.ready()).unwrap();
    wait(owner.network_ready()).unwrap();
    let (controller, target) = super::super::read_marker(&root.0.join("v2")).unwrap();
    assert_eq!(controller, expected);
    assert_ne!(target, expected);
    assert_eq!(
        owner.host().unwrap().identity_hash().unwrap().as_slice(),
        expected.as_bytes()
    );
    assert!(wait(owner.identities().unwrap().identities())
        .unwrap()
        .is_empty());
    // Even a live existing owner cannot make an import silently succeed as a
    // normal reopen. No imported key substitutes for the installation binding.
    let other = root.review(&[43; 64]);
    assert!(matches!(
        registry.start_imported(&root.0, &other, HostEmbedding::default()),
        Err(AppBootstrapError::ImportRequiresNewInstallation)
    ));
    wait(registry.shutdown().unwrap().wait()).unwrap();
    let key = KeyOwner::open_existing(root.0.join("v2/controller"), expected).unwrap();
    assert!(key.lease().material().as_bytes() == &[42; 64]);
    drop(key);
    assert_eq!(
        super::super::read_marker(&root.0.join("v2")).unwrap(),
        (controller, target)
    );
    let reopened = registry
        .open_existing(&root.0, HostEmbedding::default())
        .unwrap();
    wait(reopened.ready()).unwrap();
    wait(reopened.network_ready()).unwrap();
    assert_eq!(
        reopened.host().unwrap().identity_hash().unwrap().as_slice(),
        expected.as_bytes()
    );
    wait(registry.shutdown().unwrap().wait()).unwrap();
}

#[test]
fn import_preserves_partial_installation_without_creating_replacement_keys() {
    let root = Root::new();
    let reviewed = root.review(&[42; 64]);
    let installation = root.0.join("v2");
    files::create_private_directory(&installation).unwrap();
    std::fs::write(installation.join("interrupted"), b"preserve").unwrap();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    assert!(matches!(
        registry.start_imported(&root.0, &reviewed, HostEmbedding::default()),
        Err(AppBootstrapError::ImportRequiresNewInstallation)
    ));
    assert_eq!(
        std::fs::read(installation.join("interrupted")).unwrap(),
        b"preserve"
    );
    assert!(!installation.join("controller").exists());
    assert!(!installation.join("management-target").exists());
    assert!(matches!(
        registry.owner(),
        Err(AppBootstrapError::NotStarted)
    ));
}

#[test]
fn process_review_slot_is_explicit_replaceable_and_stale_ids_cannot_clear_new_review() {
    let root = Root::new();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    let first = root.review(&[42; 64]);
    let selection = registry
        .retain_phone_identity_import(first.clone())
        .unwrap();
    assert_eq!(selection.id.len(), 64);
    assert!(selection
        .id
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    assert_eq!(
        registry.pending_phone_identity_import(),
        Some(selection.clone())
    );
    assert_eq!(Arc::strong_count(&first), 2);
    let next = registry
        .retain_phone_identity_import(root.review(&[43; 64]))
        .unwrap();
    assert_ne!(next.id, selection.id);
    assert_eq!(Arc::strong_count(&first), 1);
    assert!(matches!(
        registry.cancel_phone_identity_import(&selection.id),
        Err(PhoneIdentityImportError::StaleReview)
    ));
    assert_eq!(registry.pending_phone_identity_import(), Some(next.clone()));
    assert!(matches!(
        registry.prepare_retained_phone_identity_import(&root.0, &selection.id),
        Err(AppBootstrapError::StaleImportReview)
    ));
    assert!(!root.0.join("v2").exists());
    registry.cancel_phone_identity_import(&next.id).unwrap();
    assert_eq!(registry.pending_phone_identity_import(), None);
}

#[test]
fn retained_import_prepares_offline_and_consumes_exact_selection_without_network_start() {
    use prns_service_runtime_uniffi::runtime::groups::standalone::NetworkPhase;
    let root = Root::new();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    let reviewed = root.review(&[42; 64]);
    let expected = reviewed.hash();
    let selection = registry.retain_phone_identity_import(reviewed).unwrap();
    std::fs::remove_file(root.0.join("selected.key")).unwrap();
    let owner = registry
        .prepare_retained_phone_identity_import(&root.0, &selection.id)
        .unwrap();
    wait(owner.ready()).unwrap();
    assert_eq!(owner.network_status().phase, NetworkPhase::Offline);
    assert!(owner.host().is_err());
    assert!(wait(owner.identities().unwrap().identities())
        .unwrap()
        .is_empty());
    assert_eq!(
        super::super::read_marker(&root.0.join("v2")).unwrap().0,
        expected
    );
    assert_eq!(
        registry.pending_phone_identity_import(),
        Some(selection.clone())
    );
    registry
        .cancel_phone_identity_import(&selection.id)
        .unwrap();
    assert_eq!(registry.pending_phone_identity_import(), None);
    assert!(matches!(
        registry.prepare_retained_phone_identity_import(&root.0, &selection.id),
        Err(AppBootstrapError::StaleImportReview)
    ));
    wait(registry.shutdown().unwrap().wait()).unwrap();
    let key = KeyOwner::open_existing(root.0.join("v2/controller"), expected).unwrap();
    assert!(key.lease().material().as_bytes() == &[42; 64]);
}

#[test]
fn ordinary_create_never_implicitly_uses_a_pending_import() {
    let root = Root::new();
    let registry = Registry::with_initial_connections(|| Ok(vec![]));
    let reviewed = root.review(&[42; 64]);
    let expected = reviewed.hash();
    registry.retain_phone_identity_import(reviewed).unwrap();
    let owner = registry.prepare_new(&root.0).unwrap();
    wait(owner.ready()).unwrap();
    assert_ne!(
        super::super::read_marker(&root.0.join("v2")).unwrap().0,
        expected
    );
    assert_eq!(registry.pending_phone_identity_import(), None);
    wait(registry.shutdown().unwrap().wait()).unwrap();
}

#[test]
fn partial_import_keeps_review_until_explicit_reset_retires_it() {
    let root = Root::new();
    let registry = Registry::with_initial_connections(|| {
        Err(prns_host_runtime::connections::Error::WorkerFailed)
    });
    let reviewed = root.review(&[42; 64]);
    let expected = reviewed.hash();
    let selection = registry.retain_phone_identity_import(reviewed).unwrap();
    assert!(registry
        .prepare_retained_phone_identity_import(&root.0, &selection.id)
        .is_err());
    assert_eq!(
        super::super::read_marker(&root.0.join("v2")).unwrap().0,
        expected
    );
    assert_eq!(
        registry.pending_phone_identity_import(),
        Some(selection.clone())
    );
    assert!(matches!(
        registry.prepare_retained_phone_identity_import(&root.0, &selection.id),
        Err(AppBootstrapError::ImportRequiresNewInstallation)
    ));
    wait(registry.reset(registry.prepare_reset().unwrap())).unwrap();
    assert_eq!(registry.pending_phone_identity_import(), None);
    assert!(!root.0.join("v2").exists());
    assert!(matches!(
        registry.prepare_retained_phone_identity_import(&root.0, &selection.id),
        Err(AppBootstrapError::StaleImportReview)
    ));
}

#[test]
fn asynchronous_local_open_failure_keeps_the_admitted_review_until_reset() {
    fn path() -> PathBuf {
        std::env::temp_dir().join(format!("prns-import-async-failure-{}", std::process::id()))
    }
    fn obstruct_local_open() -> std::result::Result<
        Vec<prns_host_runtime::connections::Profile>,
        prns_host_runtime::connections::Error,
    > {
        // The ordinary initial-profile selection runs after key publication but
        // before the service worker opens its data directory. A regular file
        // deterministically makes that later local open fail.
        std::fs::write(path().join("v2/identities"), b"blocked local directory").unwrap();
        Ok(vec![])
    }
    let root = Root(path());
    files::create_private_directory(&root.0).unwrap();
    let registry = Registry::with_initial_connections(obstruct_local_open);
    let reviewed = root.review(&[42; 64]);
    let expected = reviewed.hash();
    let selection = registry
        .retain_phone_identity_import(reviewed.clone())
        .unwrap();
    let owner = registry
        .prepare_retained_phone_identity_import(&root.0, &selection.id)
        .unwrap();
    assert!(wait(owner.ready()).is_err());
    assert_eq!(
        registry.pending_phone_identity_import(),
        Some(selection.clone())
    );
    assert!(matches!(
        registry.cancel_phone_identity_import(&selection.id),
        Err(PhoneIdentityImportError::Busy)
    ));
    assert!(matches!(
        registry.retain_phone_identity_import(reviewed),
        Err(PhoneIdentityImportError::Busy)
    ));
    assert_eq!(
        super::super::read_marker(&root.0.join("v2")).unwrap().0,
        expected
    );
    assert!(matches!(
        registry.prepare_retained_phone_identity_import(&root.0, &selection.id),
        Err(AppBootstrapError::ImportRequiresNewInstallation)
    ));
    wait(registry.reset(registry.prepare_reset().unwrap())).unwrap();
    assert_eq!(registry.pending_phone_identity_import(), None);
    assert!(!root.0.join("v2").exists());
}
