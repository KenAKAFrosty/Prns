#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Explicitly end coordinator ownership at simulated process-restart boundaries.
#![allow(clippy::drop_non_drop)]
#![cfg(unix)]

use prns_core::identity::PrivateIdentityMaterial;
use prns_identities::{Error as IdentityError, IdentitySecretKey, IdentityStore};
use prns_identities_core::*;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

type Disk = IdentityStore<4, 16, 2>;
type State = Catalog<4, 16, 2>;
const DIGEST: CommandDigest = CommandDigest([7; 32]);
static NEXT: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);
impl TestRoot {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("prns-identities-{}-{id}", std::process::id()));
        assert!(!path.exists());
        Self(path)
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

fn command(source: KeySource) -> Command<2> {
    Command::Create(CreateRequest {
        source,
        metadata: Metadata::new("Weather station", None).unwrap(),
        services: ServiceSet::new(&[ServiceId(17)]).unwrap(),
    })
}

fn private_hash(secret: &IdentitySecretKey) -> PublicIdentityHash {
    PublicIdentityHash(
        *PrivateIdentityMaterial::from(secret.clone())
            .identity_hash()
            .as_bytes(),
    )
}

fn key_path(root: &Path, reference: KeyReference) -> PathBuf {
    let name: String = reference
        .0
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    root.join("keys").join(format!("{name}.key"))
}

fn awaiting_key(
    disk: &mut Disk,
    core: &mut Coordinator<'_, 4, 16, 2>,
    source: KeySource,
) -> (OperationToken, EffectTicket, KeyReference) {
    let command = command(source);
    let token = core.reserve(disk, command, DIGEST).unwrap();
    core.begin(disk, token, &command).unwrap();
    let Action::PublishKey {
        ticket,
        key_reference,
        ..
    } = core.action(token).unwrap().unwrap()
    else {
        panic!("publication")
    };
    (token, ticket, key_reference)
}

#[test]
fn exclusive_owner_create_open_and_private_modes() {
    let root = TestRoot::new();
    let store = Disk::create_new(root.path()).unwrap();
    assert!(matches!(Disk::open(root.path()), Err(IdentityError::Busy)));
    assert!(Disk::create_new(root.path()).is_err());
    for entry in ["", "keys", "retained"] {
        assert_eq!(
            fs::metadata(root.path().join(entry))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    for entry in ["owner.lock", "catalog.bin"] {
        assert_eq!(
            fs::metadata(root.path().join(entry))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(store);
    let mut store = Disk::open(root.path()).unwrap();
    let mut snapshot = State::new([0; 16]);
    store.load(&mut snapshot).unwrap();
    assert_eq!(snapshot.revision, 0);
    assert_ne!(snapshot.incarnation, [0; 16]);
    assert_eq!(snapshot.identities().count(), 0);
}

#[test]
fn existing_missing_corrupt_and_wrong_capacity_catalogs_never_initialize() {
    let root = TestRoot::new();
    drop(Disk::create_new(root.path()).unwrap());
    let path = root.path().join("catalog.bin");
    let original = fs::read(&path).unwrap();
    for invalid in [
        Vec::new(),
        original[..original.len() - 1].to_vec(),
        {
            let mut bytes = original.clone();
            bytes[20] ^= 1;
            bytes
        },
        {
            let mut bytes = original.clone();
            bytes.push(0);
            bytes
        },
    ] {
        // Even a valid staged candidate may not replace corrupt committed data.
        fs::write(root.path().join("catalog.next"), &original).unwrap();
        fs::set_permissions(
            root.path().join("catalog.next"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        fs::write(&path, &invalid).unwrap();
        assert!(Disk::open(root.path()).is_err());
        assert_eq!(fs::read(&path).unwrap(), invalid);
    }
    fs::write(&path, &original).unwrap();
    assert!(IdentityStore::<3, 16, 2>::open(root.path()).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    fs::remove_file(&path).unwrap();
    assert!(matches!(
        Disk::open(root.path()),
        Err(IdentityError::MissingCatalog)
    ));
    assert!(!path.exists());
    assert!(Disk::create_new(root.path()).is_err());
}

#[test]
fn symlinks_hardlinks_and_permissive_files_are_rejected() {
    let root = TestRoot::new();
    drop(Disk::create_new(root.path()).unwrap());
    let path = root.path().join("catalog.bin");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        Disk::open(root.path()),
        Err(IdentityError::UnsafePermissions)
    ));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let alternate = root.path().join("alternate");
    fs::hard_link(&path, &alternate).unwrap();
    assert!(matches!(
        Disk::open(root.path()),
        Err(IdentityError::UnsafePermissions)
    ));
    fs::remove_file(&alternate).unwrap();
    fs::rename(&path, &alternate).unwrap();
    symlink(&alternate, &path).unwrap();
    assert!(matches!(
        Disk::open(root.path()),
        Err(IdentityError::UnsafePath)
    ));
}

#[test]
fn unpublished_generated_key_replays_exactly_and_restore_never_replaces_missing_key() {
    let root = TestRoot::new();
    let mut disk = Disk::create_new(root.path()).unwrap();
    let mut working = State::new([0; 16]);
    let mut staging = working.clone();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let (token, ticket, reference) = awaiting_key(&mut disk, &mut core, KeySource::Generate);
    let verified = disk.publish_key(&core, ticket, None).unwrap();
    let bytes = fs::read(key_path(root.path(), reference)).unwrap();
    assert_eq!(core.snapshot().identities().count(), 0);
    assert_eq!(disk.publish_key(&core, ticket, None).unwrap(), verified);
    assert!(fs::read(key_path(root.path(), reference)).unwrap() == bytes);
    drop(core);
    drop(disk);

    let mut disk = Disk::open(root.path()).unwrap();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    assert!(matches!(
        disk.publish_key(&core, ticket, None),
        Err(IdentityError::Authority(_))
    ));
    let ticket = core.action(token).unwrap().unwrap().ticket();
    assert_eq!(disk.publish_key(&core, ticket, None).unwrap(), verified);
    let OperationResult::Created { identity } = core.ack_key(&mut disk, ticket, verified).unwrap()
    else {
        panic!("created")
    };
    let prepared = core
        .prepare(
            identity,
            TransitionTarget::Enable,
            DependencyReport::new(&[ServiceDependency::clear(ServiceId(17), 0)], 0).unwrap(),
        )
        .unwrap();
    let command = Command::Transition(prepared);
    let enable = core.reserve(&mut disk, command, DIGEST).unwrap();
    core.begin(&mut disk, enable, &command).unwrap();
    let active = core.action(enable).unwrap().unwrap().ticket();
    let secret = disk.activate_key(&core, active).unwrap();
    assert_eq!(private_hash(&secret), verified.public_hash);
    fs::remove_file(key_path(root.path(), reference)).unwrap();
    assert!(matches!(
        disk.activate_key(&core, active),
        Err(IdentityError::MissingKey)
    ));
    assert!(!key_path(root.path(), reference).exists());
}

#[test]
fn import_validation_and_immutable_publication() {
    let root = TestRoot::new();
    let mut disk = Disk::create_new(root.path()).unwrap();
    let mut working = State::new([0; 16]);
    let mut staging = working.clone();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let secret = IdentitySecretKey::new([42; 64]);
    let (_, ticket, reference) = awaiting_key(
        &mut disk,
        &mut core,
        KeySource::Import {
            expected_hash: private_hash(&secret),
        },
    );
    assert!(matches!(
        disk.publish_key(&core, ticket, None),
        Err(IdentityError::ImportRequired)
    ));
    let wrong = IdentitySecretKey::new([43; 64]);
    assert!(matches!(
        disk.publish_key(&core, ticket, Some(&wrong)),
        Err(IdentityError::KeyMismatch)
    ));
    assert!(!key_path(root.path(), reference).exists());
    let verified = disk.publish_key(&core, ticket, Some(&secret)).unwrap();
    assert_eq!(verified.public_hash, private_hash(&secret));
    assert_eq!(disk.publish_key(&core, ticket, None).unwrap(), verified);
    assert!(matches!(
        disk.publish_key(&core, ticket, Some(&wrong)),
        Err(IdentityError::KeyMismatch)
    ));
}

#[test]
fn complete_stage_recovers_but_partial_stage_is_never_regenerated() {
    let root = TestRoot::new();
    let mut disk = Disk::create_new(root.path()).unwrap();
    let mut working = State::new([0; 16]);
    let mut staging = working.clone();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let (_, ticket, reference) = awaiting_key(&mut disk, &mut core, KeySource::Generate);
    let verified = disk.publish_key(&core, ticket, None).unwrap();
    let key = key_path(root.path(), reference);
    let pending = key.with_extension("pending");
    let bytes = fs::read(&key).unwrap();
    fs::rename(&key, &pending).unwrap();
    assert_eq!(disk.publish_key(&core, ticket, None).unwrap(), verified);
    assert!(fs::read(&key).unwrap() == bytes);
    fs::rename(&key, &pending).unwrap();
    fs::write(&pending, &bytes[..40]).unwrap();
    assert!(matches!(
        disk.publish_key(&core, ticket, None),
        Err(IdentityError::Corrupt)
    ));
    assert!(fs::read(&pending).unwrap() == bytes[..40]);
    assert!(!key.exists());
}

#[test]
fn stale_coordinator_and_cas_cannot_mutate_catalog_or_vault() {
    let root = TestRoot::new();
    let mut disk = Disk::create_new(root.path()).unwrap();
    let mut working = State::new([0; 16]);
    let mut staging = working.clone();
    let mut old = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let (_, ticket, reference) = awaiting_key(&mut disk, &mut old, KeySource::Generate);
    let mut fresh_working = State::new([0; 16]);
    let mut fresh_staging = fresh_working.clone();
    let fresh = Coordinator::open(&mut disk, &mut fresh_working, &mut fresh_staging).unwrap();
    assert!(matches!(
        disk.publish_key(&old, ticket, None),
        Err(IdentityError::RevisionMismatch { .. })
    ));
    assert!(!key_path(root.path(), reference).exists());
    let before = fs::read(root.path().join("catalog.bin")).unwrap();
    let mut old_snapshot = old.snapshot().clone();
    old_snapshot.revision += 1;
    assert!(disk
        .compare_and_store(old.snapshot().revision, &old_snapshot)
        .is_err());
    assert_eq!(fs::read(root.path().join("catalog.bin")).unwrap(), before);
    assert_eq!(fresh.snapshot().identities().count(), 0);
    assert!(old
        .reserve(&mut disk, command(KeySource::Generate), DIGEST)
        .is_err());
    assert!(old.is_poisoned());
    old.reload(&mut disk).unwrap();
    assert!(!old.is_poisoned());
    assert!(matches!(
        disk.publish_key(&old, ticket, None),
        Err(IdentityError::Authority(_))
    ));
}

#[test]
fn key_checksum_reference_and_derived_hash_are_checked_without_replacement() {
    let root = TestRoot::new();
    let mut disk = Disk::create_new(root.path()).unwrap();
    let mut working = State::new([0; 16]);
    let mut staging = working.clone();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let (_, ticket, reference) = awaiting_key(&mut disk, &mut core, KeySource::Generate);
    disk.publish_key(&core, ticket, None).unwrap();
    let key = key_path(root.path(), reference);
    let original = fs::read(&key).unwrap();
    for offset in [8, 40, 57] {
        let mut invalid = original.clone();
        invalid[offset] ^= 1;
        // Even a recomputed unkeyed checksum cannot authorize another key/ref.
        let digest = prns_core::crypto::sha256(&invalid[..120]);
        invalid[120..].copy_from_slice(&digest);
        fs::write(&key, &invalid).unwrap();
        assert!(disk.publish_key(&core, ticket, None).is_err());
        assert!(fs::read(&key).unwrap() == invalid);
    }
    fs::write(&key, &original[..151]).unwrap();
    assert!(matches!(
        disk.publish_key(&core, ticket, None),
        Err(IdentityError::Corrupt)
    ));
}

#[test]
fn duplicate_import_is_accounted_durably_before_result_pruning() {
    let root = TestRoot::new();
    let mut disk = Disk::create_new(root.path()).unwrap();
    let mut working = State::new([0; 16]);
    let mut staging = working.clone();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let (_, first, first_ref) = awaiting_key(&mut disk, &mut core, KeySource::Generate);
    let first_key = disk.publish_key(&core, first, None).unwrap();
    // Model a competing import of a generated but not yet catalogued key. Two
    // already-known same-hash import commands are rejected earlier by the core.
    let stored = fs::read(key_path(root.path(), first_ref)).unwrap();
    let mut secret = IdentitySecretKey::new([0; 64]);
    secret.copy_from_slice(&stored[56..120]);
    let source = KeySource::Import {
        expected_hash: first_key.public_hash,
    };
    let (second_token, second, second_ref) = awaiting_key(&mut disk, &mut core, source);
    core.ack_key(&mut disk, first, first_key).unwrap();
    let second_key = disk.publish_key(&core, second, Some(&secret)).unwrap();
    assert!(matches!(
        core.ack_key(&mut disk, second, second_key).unwrap(),
        OperationResult::Existing { .. }
    ));
    assert_ne!(first_ref, second_ref);
    assert!(matches!(
        core.prune_completed(&mut disk, second_token.sequence),
        Err(prns_identities_core::Error::Fault(Fault::UnaccountedKey))
    ));
    disk.account_retained_key(&core, second_token, second_ref)
        .unwrap();
    drop(core);
    drop(disk);
    let mut disk = Disk::open(root.path()).unwrap();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    disk.account_retained_key(&core, second_token, second_ref)
        .unwrap();
    core.acknowledge_retained_key(&mut disk, second_token, second_ref)
        .unwrap();
    disk.account_retained_key(&core, second_token, second_ref)
        .unwrap();
    core.prune_completed(&mut disk, second_token.sequence)
        .unwrap();
    assert!(key_path(root.path(), first_ref).exists());
    assert!(key_path(root.path(), second_ref).exists());
    assert_eq!(
        fs::read_dir(root.path().join("retained")).unwrap().count(),
        1
    );
}

#[test]
fn native_catalog_bound_is_checked_before_large_stack_construction() {
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(|| {
            let rejected = TestRoot::new();
            assert!(matches!(
                IdentityStore::<4, 8000, 2>::create_new(rejected.path()),
                Err(IdentityError::Bounds)
            ));
            assert!(!rejected.path().exists());
            assert!(matches!(
                IdentityStore::<4, 8000, 2>::open(rejected.path()),
                Err(IdentityError::Bounds)
            ));
            // A zero-record catalog can be tiny even though its service-bound
            // record types would create enormous codec stack temporaries.
            assert!(matches!(
                IdentityStore::<0, 0, 8000>::create_new(rejected.path()),
                Err(IdentityError::Bounds)
            ));
            assert!(matches!(
                IdentityStore::<0, 0, 8000>::open(rejected.path()),
                Err(IdentityError::Bounds)
            ));
            assert!(!rejected.path().exists());
            let root = TestRoot::new();
            let mut disk = Disk::create_new(root.path()).unwrap();
            let mut working = State::new([0; 16]);
            let mut staging = working.clone();
            let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
            let (_, ticket, _) = awaiting_key(&mut disk, &mut core, KeySource::Generate);
            let key = disk.publish_key(&core, ticket, None).unwrap();
            core.ack_key(&mut disk, ticket, key).unwrap();
            drop(core);
            drop(disk);
            drop(Disk::open(root.path()).unwrap());
        })
        .unwrap()
        .join()
        .unwrap();
}

#[cfg(feature = "fault-injection")]
fn child(root: &Path, mode: &str, point: Option<&str>) -> std::process::Output {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "crash_child_entry", "--nocapture"])
        .env("PRNS_IDENTITIES_TEST_ROOT", root)
        .env("PRNS_IDENTITIES_TEST_MODE", mode);
    if let Some(point) = point {
        command.env("PRNS_IDENTITIES_CRASH_AT", point);
    }
    command.output().unwrap()
}

#[cfg(feature = "fault-injection")]
#[test]
fn crash_child_entry() {
    let Some(root) = std::env::var_os("PRNS_IDENTITIES_TEST_ROOT") else {
        return;
    };
    let mode = std::env::var("PRNS_IDENTITIES_TEST_MODE").unwrap();
    if mode == "create" {
        drop(Disk::create_new(root).unwrap());
        return;
    }
    if mode == "relative" {
        let root = PathBuf::from(root);
        std::env::set_current_dir(root.parent().unwrap()).unwrap();
        let relative = Path::new(root.file_name().unwrap());
        let mut disk = Disk::create_new(relative).unwrap();
        assert!(disk.root().is_absolute());
        std::env::set_current_dir(root.join("keys")).unwrap();
        let mut snapshot = State::new([0; 16]);
        disk.load(&mut snapshot).unwrap();
        drop(disk);
        std::env::set_current_dir(root.parent().unwrap()).unwrap();
        let mut disk = Disk::open(relative).unwrap();
        std::env::set_current_dir(root.join("retained")).unwrap();
        disk.load(&mut snapshot).unwrap();
        return;
    }
    if mode == "lock" {
        assert!(matches!(Disk::open(root), Err(IdentityError::Busy)));
        return;
    }
    let mut disk = Disk::open(root).unwrap();
    let mut working = State::new([0; 16]);
    let mut staging = working.clone();
    let core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    if mode == "key" {
        let token = core.snapshot().operations().next().unwrap().token;
        let ticket = core.action(token).unwrap().unwrap().ticket();
        disk.publish_key(&core, ticket, None).unwrap();
    }
    if mode == "retained" {
        let operation = core
            .snapshot()
            .operations()
            .find(|op| op.unpublished_key.is_some())
            .unwrap();
        disk.account_retained_key(&core, operation.token, operation.unpublished_key.unwrap())
            .unwrap();
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn separate_process_cannot_steal_owner_lock() {
    let root = TestRoot::new();
    let _store = Disk::create_new(root.path()).unwrap();
    assert!(child(root.path(), "lock", None).status.success());
}

#[cfg(feature = "fault-injection")]
#[test]
fn relative_root_is_resolved_once_and_survives_process_cwd_changes() {
    let root = TestRoot::new();
    let output = child(root.path(), "relative", None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.path().join("catalog.bin").exists());
}

#[cfg(feature = "fault-injection")]
#[test]
fn process_crashes_at_every_catalog_publication_boundary_preserve_old_or_new_snapshot() {
    for step in [
        "before_stage",
        "before_file_sync",
        "after_file_sync",
        "after_rename",
        "after_directory_sync",
    ] {
        let root = TestRoot::new();
        drop(Disk::create_new(root.path()).unwrap());
        let outcome = child(root.path(), "catalog", Some(&format!("catalog.{step}")));
        assert_eq!(
            outcome.status.code(),
            Some(91),
            "{step}: {}",
            String::from_utf8_lossy(&outcome.stderr)
        );
        let mut disk = Disk::open(root.path()).unwrap();
        let mut snapshot = State::new([0; 16]);
        disk.load(&mut snapshot).unwrap();
        assert_eq!(
            snapshot.revision,
            if step.contains("rename") || step.contains("directory") {
                1
            } else {
                0
            }
        );
        let mut staging = snapshot.clone();
        let core = Coordinator::open(&mut disk, &mut snapshot, &mut staging).unwrap();
        assert!(core.snapshot().session >= 1);
        assert_eq!(core.snapshot().identities().count(), 0);
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn initial_creation_crashes_never_reinitialize_an_incomplete_directory() {
    for step in [
        "before_stage",
        "before_file_sync",
        "after_file_sync",
        "after_rename",
        "after_directory_sync",
    ] {
        let root = TestRoot::new();
        let outcome = child(root.path(), "create", Some(&format!("catalog.{step}")));
        assert_eq!(
            outcome.status.code(),
            Some(91),
            "{step}: {}",
            String::from_utf8_lossy(&outcome.stderr)
        );
        assert!(root.path().exists());
        assert!(Disk::create_new(root.path()).is_err());
        let published = step.contains("rename") || step.contains("directory");
        if published {
            let mut disk = Disk::open(root.path()).unwrap();
            let mut snapshot = State::new([0; 16]);
            disk.load(&mut snapshot).unwrap();
            assert_eq!(snapshot.revision, 0);
            assert_eq!(snapshot.identities().count(), 0);
        } else {
            assert!(matches!(
                Disk::open(root.path()),
                Err(IdentityError::MissingCatalog)
            ));
            assert!(!root.path().join("catalog.bin").exists());
        }
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn process_crashes_preserve_exact_key_reference_and_material_until_ack() {
    for step in [
        "before_stage",
        "before_file_sync",
        "after_file_sync",
        "after_rename",
        "after_directory_sync",
    ] {
        let root = TestRoot::new();
        let mut disk = Disk::create_new(root.path()).unwrap();
        let mut working = State::new([0; 16]);
        let mut staging = working.clone();
        let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
        let (token, _, reference) = awaiting_key(&mut disk, &mut core, KeySource::Generate);
        drop(core);
        drop(disk);
        let outcome = child(root.path(), "key", Some(&format!("key.{step}")));
        assert_eq!(
            outcome.status.code(),
            Some(91),
            "{step}: {}",
            String::from_utf8_lossy(&outcome.stderr)
        );
        let key = key_path(root.path(), reference);
        let persisted = fs::read(&key)
            .or_else(|_| fs::read(key.with_extension("pending")))
            .ok();
        assert_eq!(persisted.is_none(), step == "before_stage");
        let mut disk = Disk::open(root.path()).unwrap();
        let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
        assert_eq!(core.snapshot().identities().count(), 0);
        let ticket = core.action(token).unwrap().unwrap().ticket();
        let verified = disk.publish_key(&core, ticket, None).unwrap();
        if let Some(persisted) = persisted {
            assert!(fs::read(&key).unwrap() == persisted, "{step}: key replaced");
        }
        let result = core.ack_key(&mut disk, ticket, verified).unwrap();
        assert!(matches!(result, OperationResult::Created { .. }));
        assert_eq!(core.snapshot().identities().count(), 1);
        assert_eq!(core.ack_key(&mut disk, ticket, verified).unwrap(), result);
        assert_eq!(
            core.resume(token, &command(KeySource::Generate)).unwrap(),
            OperationState::Completed(result)
        );
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn process_crashes_during_retention_indexing_keep_pruning_fenced_until_durable_ack() {
    for step in [
        "before_stage",
        "before_file_sync",
        "after_file_sync",
        "after_rename",
        "after_directory_sync",
    ] {
        let root = TestRoot::new();
        let mut disk = Disk::create_new(root.path()).unwrap();
        let mut working = State::new([0; 16]);
        let mut staging = working.clone();
        let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
        let (_, first, first_ref) = awaiting_key(&mut disk, &mut core, KeySource::Generate);
        let verified = disk.publish_key(&core, first, None).unwrap();
        let stored = fs::read(key_path(root.path(), first_ref)).unwrap();
        let mut secret = IdentitySecretKey::new([0; 64]);
        secret.copy_from_slice(&stored[56..120]);
        let (token, second, reference) = awaiting_key(
            &mut disk,
            &mut core,
            KeySource::Import {
                expected_hash: verified.public_hash,
            },
        );
        core.ack_key(&mut disk, first, verified).unwrap();
        let imported = disk.publish_key(&core, second, Some(&secret)).unwrap();
        core.ack_key(&mut disk, second, imported).unwrap();
        drop(core);
        drop(disk);
        let outcome = child(root.path(), "retained", Some(&format!("retained.{step}")));
        assert_eq!(
            outcome.status.code(),
            Some(91),
            "{step}: {}",
            String::from_utf8_lossy(&outcome.stderr)
        );
        let mut disk = Disk::open(root.path()).unwrap();
        let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
        assert!(matches!(
            core.prune_completed(&mut disk, token.sequence),
            Err(prns_identities_core::Error::Fault(Fault::UnaccountedKey))
        ));
        disk.account_retained_key(&core, token, reference).unwrap();
        core.acknowledge_retained_key(&mut disk, token, reference)
            .unwrap();
        core.prune_completed(&mut disk, token.sequence).unwrap();
        assert!(key_path(root.path(), first_ref).exists());
        assert!(key_path(root.path(), reference).exists());
        assert_eq!(
            fs::read_dir(root.path().join("retained")).unwrap().count(),
            1
        );
    }
}
