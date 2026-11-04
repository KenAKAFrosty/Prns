#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use prns_core::identity::IdentityHash;
use prns_identities::keys::KeyOwner;
use prns_identities::Error;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("prns-key-owner-{}-{id}", std::process::id()));
        assert!(!root.exists());
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn entries(root: &Path) -> Vec<String> {
    let mut entries: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    entries
}

#[test]
fn creates_one_private_key_without_catalog_and_reopens_same_identity() {
    let root = TestRoot::new();
    let owner = KeyOwner::create_new(root.path()).unwrap();
    let public = owner.public_identity();
    let expected = owner.identity_hash();
    let before = fs::read(root.path().join("identity.key")).unwrap();
    assert_eq!(entries(root.path()), ["identity.key", "owner.lock"]);
    assert!(owner.root().is_absolute());
    assert_eq!(public.identity_hash(), expected);
    assert_eq!(
        fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in ["identity.key", "owner.lock"] {
        assert_eq!(
            fs::metadata(root.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let signature = owner.lease().material().sign(b"native telemetry");
    public.verify(b"native telemetry", &signature).unwrap();
    drop(owner);
    let reopened = KeyOwner::open_existing(root.path(), expected).unwrap();
    assert_eq!(reopened.public_identity(), public);
    assert!(fs::read(root.path().join("identity.key")).unwrap() == before);
}

#[test]
fn wrong_identity_fails_without_replacing_key_or_retaining_lock() {
    let root = TestRoot::new();
    let owner = KeyOwner::create_new(root.path()).unwrap();
    let expected = owner.identity_hash();
    let mut wrong = *expected.as_bytes();
    wrong[0] ^= 1;
    drop(owner);
    let before = fs::read(root.path().join("identity.key")).unwrap();
    assert!(matches!(
        KeyOwner::open_existing(root.path(), IdentityHash::new(wrong)),
        Err(Error::KeyMismatch)
    ));
    assert!(fs::read(root.path().join("identity.key")).unwrap() == before);
    drop(KeyOwner::open_existing(root.path(), expected).unwrap());
}

#[test]
fn live_owner_and_cloned_leases_exclude_another_owner_until_last_drop() {
    let root = TestRoot::new();
    let owner = KeyOwner::create_new(root.path()).unwrap();
    let expected = owner.identity_hash();
    let first = owner.lease();
    let second = first.clone();
    assert!(matches!(
        KeyOwner::open_existing(root.path(), expected),
        Err(Error::Busy)
    ));
    drop(owner);
    assert!(matches!(
        KeyOwner::open_existing(root.path(), expected),
        Err(Error::Busy)
    ));
    drop(first);
    assert!(matches!(
        KeyOwner::open_existing(root.path(), expected),
        Err(Error::Busy)
    ));
    let signature = second.material().sign(b"leased domain work");
    second
        .public_identity()
        .verify(b"leased domain work", &signature)
        .unwrap();
    assert_eq!(second.identity_hash(), expected);
    drop(second);
    drop(KeyOwner::open_existing(root.path(), expected).unwrap());
}

#[test]
fn caller_failure_does_not_release_a_domains_retained_key() {
    let root = TestRoot::new();
    let owner = KeyOwner::create_new(root.path()).unwrap();
    let expected = owner.identity_hash();
    let public = owner.public_identity();
    let lease = owner.lease();
    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let worker = std::thread::spawn(move || {
        worker_barrier.wait();
        lease.material().sign(b"accepted before caller failure")
    });
    // The invoking caller can fail or disappear independently of accepted work.
    drop(owner);
    assert!(matches!(
        KeyOwner::open_existing(root.path(), expected),
        Err(Error::Busy)
    ));
    barrier.wait();
    let signature = worker.join().unwrap();
    public
        .verify(b"accepted before caller failure", &signature)
        .unwrap();
    drop(KeyOwner::open_existing(root.path(), expected).unwrap());
}

#[test]
fn create_never_reinitializes_an_existing_directory() {
    let root = TestRoot::new();
    let owner = KeyOwner::create_new(root.path()).unwrap();
    let expected = owner.identity_hash();
    drop(owner);
    let before = fs::read(root.path().join("identity.key")).unwrap();
    assert!(KeyOwner::create_new(root.path()).is_err());
    assert!(fs::read(root.path().join("identity.key")).unwrap() == before);
    drop(KeyOwner::open_existing(root.path(), expected).unwrap());

    let empty = TestRoot::new();
    fs::create_dir(empty.path()).unwrap();
    assert!(KeyOwner::create_new(empty.path()).is_err());
    assert!(entries(empty.path()).is_empty());
}

#[test]
fn missing_and_staged_only_keys_never_generate_or_promote() {
    let root = TestRoot::new();
    let expected = IdentityHash::new([7; 16]);
    assert!(KeyOwner::open_existing(root.path(), expected).is_err());
    assert!(!root.path().exists());

    let owner = KeyOwner::create_new(root.path()).unwrap();
    let expected = owner.identity_hash();
    drop(owner);
    let path = root.path().join("identity.key");
    let pending = root.path().join("identity.pending");
    fs::rename(&path, &pending).unwrap();
    let before = fs::read(&pending).unwrap();
    assert!(matches!(
        KeyOwner::open_existing(root.path(), expected),
        Err(Error::MissingKey)
    ));
    assert!(!path.exists());
    assert!(fs::read(&pending).unwrap() == before);
    assert!(KeyOwner::create_new(root.path()).is_err());
    assert!(fs::read(&pending).unwrap() == before);
    fs::remove_file(&pending).unwrap();
    assert!(matches!(
        KeyOwner::open_existing(root.path(), expected),
        Err(Error::MissingKey)
    ));
    assert_eq!(entries(root.path()), ["owner.lock"]);
}

#[test]
fn corrupt_published_key_never_falls_back_to_valid_stage() {
    let root = TestRoot::new();
    let owner = KeyOwner::create_new(root.path()).unwrap();
    let expected = owner.identity_hash();
    drop(owner);
    let path = root.path().join("identity.key");
    let original = fs::read(&path).unwrap();
    let pending = root.path().join("identity.pending");
    fs::copy(&path, &pending).unwrap();
    for invalid in [original[..original.len() - 1].to_vec(), {
        let mut invalid = original.clone();
        invalid[56] ^= 1;
        invalid
    }] {
        fs::write(&path, &invalid).unwrap();
        assert!(matches!(
            KeyOwner::open_existing(root.path(), expected),
            Err(Error::Corrupt)
        ));
        assert!(fs::read(&path).unwrap() == invalid);
        assert!(fs::read(&pending).unwrap() == original);
    }
    fs::write(&path, &original).unwrap();
    drop(KeyOwner::open_existing(root.path(), expected).unwrap());
}

#[test]
fn unsafe_key_permissions_and_symlink_are_rejected_without_changes() {
    let root = TestRoot::new();
    let owner = KeyOwner::create_new(root.path()).unwrap();
    let expected = owner.identity_hash();
    drop(owner);
    let path = root.path().join("identity.key");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        KeyOwner::open_existing(root.path(), expected),
        Err(Error::UnsafePermissions)
    ));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let moved = root.path().join("moved.key");
    fs::rename(&path, &moved).unwrap();
    symlink(&moved, &path).unwrap();
    assert!(matches!(
        KeyOwner::open_existing(root.path(), expected),
        Err(Error::UnsafePath)
    ));
    assert!(fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(feature = "fault-injection")]
fn child(root: &Path, mode: &str, point: Option<&str>) -> std::process::Output {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "crash_child_entry", "--nocapture"])
        .env("PRNS_KEYS_TEST_ROOT", root)
        .env("PRNS_KEYS_TEST_MODE", mode);
    if let Some(point) = point {
        command.env("PRNS_IDENTITIES_CRASH_AT", point);
    }
    command.output().unwrap()
}

#[cfg(feature = "fault-injection")]
#[test]
fn crash_child_entry() {
    let Some(root) = std::env::var_os("PRNS_KEYS_TEST_ROOT") else {
        return;
    };
    match std::env::var("PRNS_KEYS_TEST_MODE").unwrap().as_str() {
        "create" => drop(KeyOwner::create_new(root).unwrap()),
        "lock" => assert!(matches!(
            KeyOwner::open_existing(root, IdentityHash::new([0; 16])),
            Err(Error::Busy)
        )),
        _ => panic!("unknown child mode"),
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn separate_process_cannot_steal_key_owner_lock() {
    let root = TestRoot::new();
    let _owner = KeyOwner::create_new(root.path()).unwrap();
    assert!(child(root.path(), "lock", None).status.success());
}

#[cfg(feature = "fault-injection")]
#[test]
fn interrupted_creation_never_reinitializes_or_replaces_staged_material() {
    for step in [
        "before_stage",
        "before_file_sync",
        "after_file_sync",
        "after_rename",
        "after_directory_sync",
    ] {
        let root = TestRoot::new();
        let output = child(root.path(), "create", Some(&format!("key.{step}")));
        assert_eq!(
            output.status.code(),
            Some(91),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let path = root.path().join("identity.key");
        let pending = root.path().join("identity.pending");
        let before = fs::read(&path).or_else(|_| fs::read(&pending)).ok();
        assert!(KeyOwner::create_new(root.path()).is_err());
        if step == "before_stage" {
            assert!(matches!(
                KeyOwner::recover_creation(root.path()),
                Err(Error::MissingKey)
            ));
            assert!(!path.exists());
        } else {
            let recovered = KeyOwner::recover_creation(root.path()).unwrap();
            let public = recovered.public_identity();
            let expected = public.identity_hash();
            let signature = recovered.lease().material().sign(b"recovered creation");
            public.verify(b"recovered creation", &signature).unwrap();
            assert!(!pending.exists());
            drop(recovered);
            let reopened = KeyOwner::open_existing(root.path(), expected).unwrap();
            assert_eq!(reopened.public_identity(), public);
        }
        assert!(fs::read(&path).or_else(|_| fs::read(&pending)).ok() == before);
    }
}

#[test]
fn lost_creation_reply_recovers_public_evidence_and_the_exact_key() {
    let root = TestRoot::new();
    // The caller loses the return value before retaining any public identity.
    drop(KeyOwner::create_new(root.path()).unwrap());
    let before = fs::read(root.path().join("identity.key")).unwrap();
    let recovered = KeyOwner::recover_creation(root.path()).unwrap();
    let public = recovered.public_identity();
    let expected = recovered.identity_hash();
    assert!(matches!(
        KeyOwner::recover_creation(root.path()),
        Err(Error::Busy)
    ));
    drop(recovered);
    let again = KeyOwner::recover_creation(root.path()).unwrap();
    assert_eq!(again.public_identity(), public);
    assert!(fs::read(root.path().join("identity.key")).unwrap() == before);
    drop(again);
    drop(KeyOwner::open_existing(root.path(), expected).unwrap());
}

#[test]
fn creation_recovery_never_initializes_an_absent_directory() {
    let root = TestRoot::new();
    assert!(KeyOwner::recover_creation(root.path()).is_err());
    assert!(!root.path().exists());
}

#[test]
fn creation_recovery_preserves_partial_or_contradictory_keys() {
    let root = TestRoot::new();
    drop(KeyOwner::create_new(root.path()).unwrap());
    let path = root.path().join("identity.key");
    let pending = root.path().join("identity.pending");
    let original = fs::read(&path).unwrap();
    fs::rename(&path, &pending).unwrap();
    let partial = &original[..original.len() - 1];
    fs::write(&pending, partial).unwrap();
    assert!(matches!(
        KeyOwner::recover_creation(root.path()),
        Err(Error::Corrupt)
    ));
    assert!(!path.exists());
    assert!(fs::read(&pending).unwrap() == partial);

    fs::write(&pending, &original).unwrap();
    drop(KeyOwner::recover_creation(root.path()).unwrap());
    let other = TestRoot::new();
    drop(KeyOwner::create_new(other.path()).unwrap());
    fs::copy(other.path().join("identity.key"), &pending).unwrap();
    let conflicting = fs::read(&pending).unwrap();
    assert!(matches!(
        KeyOwner::recover_creation(root.path()),
        Err(Error::KeyMismatch)
    ));
    assert!(fs::read(&path).unwrap() == original);
    assert!(fs::read(&pending).unwrap() == conflicting);

    fs::write(&path, partial).unwrap();
    fs::write(&pending, &original).unwrap();
    assert!(matches!(
        KeyOwner::recover_creation(root.path()),
        Err(Error::Corrupt)
    ));
    assert!(fs::read(&path).unwrap() == partial);
    assert!(fs::read(&pending).unwrap() == original);
}
#[test]
fn imported_key_keeps_exact_secret_and_never_replaces_an_existing_root() {
    use prns_core::identity::PrivateIdentityMaterial;
    use prns_identities::IdentitySecretKey;
    let root = TestRoot::new();
    let secret = IdentitySecretKey::new([42; 64]);
    let expected = PrivateIdentityMaterial::from(secret.clone()).identity_hash();
    let mut wrong = *expected.as_bytes();
    wrong[0] ^= 1;
    assert!(matches!(
        KeyOwner::create_imported(root.path(), &secret, IdentityHash::new(wrong)),
        Err(Error::KeyMismatch)
    ));
    assert!(!root.path().exists());
    let owner = KeyOwner::create_imported(root.path(), &secret, expected).unwrap();
    assert_eq!(owner.identity_hash(), expected);
    assert!(owner.lease().material().as_bytes().as_slice() == secret.as_ref());
    drop(owner);
    let before = fs::read(root.path().join("identity.key")).unwrap();
    assert!(KeyOwner::create_imported(root.path(), &secret, expected).is_err());
    let recovered = KeyOwner::recover_import(root.path(), &secret, expected).unwrap();
    assert!(recovered.lease().material().as_bytes().as_slice() == secret.as_ref());
    drop(recovered);
    drop(KeyOwner::open_existing(root.path(), expected).unwrap());
    assert!(fs::read(root.path().join("identity.key")).unwrap() == before);
}

#[test]
fn import_recovery_requires_exact_staged_secret_even_when_public_hash_matches() {
    use prns_core::identity::PrivateIdentityMaterial;
    use prns_identities::IdentitySecretKey;
    let root = TestRoot::new();
    let secret = IdentitySecretKey::new([42; 64]);
    let expected = PrivateIdentityMaterial::from(secret.clone()).identity_hash();
    drop(KeyOwner::create_imported(root.path(), &secret, expected).unwrap());
    let key = root.path().join("identity.key");
    let pending = root.path().join("identity.pending");
    fs::rename(&key, &pending).unwrap();
    let before = fs::read(&pending).unwrap();
    // X25519 masks these scalar bits, so public identity equality alone does
    // not establish that the reviewed private file is the preserved source.
    let mut other = secret.clone();
    other[0] ^= 1;
    assert_eq!(
        PrivateIdentityMaterial::from(other.clone()).identity_hash(),
        expected
    );
    assert!(matches!(
        KeyOwner::recover_import(root.path(), &other, expected),
        Err(Error::KeyMismatch)
    ));
    assert!(!key.exists());
    assert!(fs::read(&pending).unwrap() == before);
    let owner = KeyOwner::recover_import(root.path(), &secret, expected).unwrap();
    assert!(owner.lease().material().as_bytes().as_slice() == secret.as_ref());
    assert!(!pending.exists());
    assert!(fs::read(&key).unwrap() == before);
}

#[test]
fn import_recovery_never_populates_missing_or_corrupt_key_state() {
    use prns_core::identity::PrivateIdentityMaterial;
    use prns_identities::IdentitySecretKey;
    let root = TestRoot::new();
    let secret = IdentitySecretKey::new([42; 64]);
    let expected = PrivateIdentityMaterial::from(secret.clone()).identity_hash();
    assert!(KeyOwner::recover_import(root.path(), &secret, expected).is_err());
    assert!(!root.path().exists());
    drop(KeyOwner::create_imported(root.path(), &secret, expected).unwrap());
    let key = root.path().join("identity.key");
    fs::write(&key, [0; 7]).unwrap();
    assert!(matches!(
        KeyOwner::recover_import(root.path(), &secret, expected),
        Err(Error::Corrupt)
    ));
    assert_eq!(fs::read(&key).unwrap(), [0; 7]);
    fs::remove_file(&key).unwrap();
    assert!(matches!(
        KeyOwner::recover_import(root.path(), &secret, expected),
        Err(Error::MissingKey)
    ));
    assert_eq!(entries(root.path()), ["owner.lock"]);
}
