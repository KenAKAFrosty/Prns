#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::drop_non_drop)]
#![cfg(unix)]

use prns_core::identity::PrivateIdentityMaterial;
use prns_identities::creation::{CreateError, CreateOutcome, CreateRequestId, CreateRequestScope};
use prns_identities::{Error as IdentityError, IdentitySecretKey, IdentityStore};
use prns_identities_core::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

type Disk = IdentityStore<4, 8, 2>;
type State = Catalog<4, 8, 2>;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "prns-create-evidence-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(!root.exists());
        drop(Disk::create_new(&root).unwrap());
        Self(root)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn intent() -> CreateRequest<2> {
    CreateRequest {
        source: KeySource::Generate,
        metadata: Metadata::new("Weather station", None).unwrap(),
        services: ServiceSet::empty(),
    }
}
fn request(scope: CreateRequestScope, nonce: u8) -> CreateRequestId {
    CreateRequestId {
        scope,
        nonce: [nonce; 32],
    }
}
fn with_core<T>(root: &Path, f: impl FnOnce(&mut Disk, &mut Coordinator<'_, 4, 8, 2>) -> T) -> T {
    let mut disk = Disk::open(root).unwrap();
    let mut working = State::new([0; 16]);
    let mut staging = working.clone();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    f(&mut disk, &mut core)
}
fn scope(root: &Path) -> CreateRequestScope {
    Disk::open(root).unwrap().creation_scope().unwrap()
}
fn created(outcome: CreateOutcome) -> LocalIdentityId {
    match outcome.result {
        OperationResult::Created { identity } => identity,
        _ => panic!("expected created identity"),
    }
}
fn key_bytes(root: &Path) -> Option<Vec<u8>> {
    let mut paths: Vec<_> = fs::read_dir(root.join("keys"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    assert!(paths.len() <= 1);
    paths.first().map(|path| fs::read(path).unwrap())
}

#[test]
fn same_request_recovers_after_pruning_restart_and_metadata_changes() {
    let root = Root::new();
    let id = request(scope(root.path()), 1);
    let first = with_core(root.path(), |disk, core| {
        let first = disk.create_or_recover(core, id, intent(), None).unwrap();
        let identity = created(first);
        let record = *core.identity(identity).unwrap();
        assert_eq!(record.desired, DesiredState::Paused);
        assert_eq!(record.services, ServiceSet::empty());
        assert_eq!(core.snapshot().operations().count(), 0);
        assert_eq!(core.snapshot().completed_floor, first.operation.sequence);
        core.update_metadata(
            disk,
            record.metadata_guard(),
            Metadata::new("Renamed locally", None).unwrap(),
        )
        .unwrap();
        assert_eq!(
            disk.create_or_recover(core, id, intent(), None).unwrap(),
            first
        );
        assert_eq!(
            core.identity(identity)
                .unwrap()
                .metadata
                .local_label
                .as_str(),
            "Renamed locally"
        );
        first
    });
    let key = key_bytes(root.path()).unwrap();
    with_core(root.path(), |disk, core| {
        assert_eq!(
            disk.create_or_recover(core, id, intent(), None).unwrap(),
            first
        );
        assert_eq!(core.snapshot().identities().count(), 1);
        assert_eq!(core.snapshot().issued_sequence, first.operation.sequence);
    });
    assert!(key_bytes(root.path()).unwrap() == key);
}

#[test]
fn full_intent_conflicts_and_foreign_future_scopes_never_allocate() {
    let root = Root::new();
    let id = request(scope(root.path()), 2);
    with_core(root.path(), |disk, core| {
        let first = disk.create_or_recover(core, id, intent(), None).unwrap();
        let mut changed = intent();
        changed.services = ServiceSet::new(&[ServiceId(17)]).unwrap();
        assert!(matches!(
            disk.create_or_recover(core, id, changed, None),
            Err(CreateError::Conflict)
        ));
        changed = intent();
        changed.metadata = Metadata::new("Other label", None).unwrap();
        assert!(matches!(
            disk.create_or_recover(core, id, changed, None),
            Err(CreateError::Conflict)
        ));
        changed = intent();
        changed.source = KeySource::Import {
            expected_hash: PublicIdentityHash([9; 16]),
        };
        assert!(matches!(
            disk.create_or_recover(core, id, changed, None),
            Err(CreateError::Conflict)
        ));
        let mut future = id;
        future.scope.epoch += 1;
        assert!(matches!(
            disk.create_or_recover(core, future, intent(), None),
            Err(CreateError::FutureScope)
        ));
        let mut foreign = id;
        foreign.scope.incarnation[0] ^= 1;
        assert!(matches!(
            disk.create_or_recover(core, foreign, intent(), None),
            Err(CreateError::ForeignCatalog)
        ));
        assert_eq!(core.snapshot().issued_sequence, first.operation.sequence);
    });
    let other = Root::new();
    with_core(other.path(), |disk, core| {
        assert!(matches!(
            disk.create_or_recover(core, id, intent(), None),
            Err(CreateError::ForeignCatalog)
        ));
        assert_eq!(core.snapshot().identities().count(), 0);
    });
}

#[test]
fn explicit_confirmation_expires_forgotten_ids_but_preserves_pending_old_scope() {
    let root = Root::new();
    let initial = scope(root.path());
    let done = request(initial, 1);
    let pending = request(initial, 2);
    let secret = IdentitySecretKey::new([31; 64]);
    let mut imported = intent();
    imported.source = KeySource::Import {
        expected_hash: PublicIdentityHash(
            *PrivateIdentityMaterial::from(secret.clone())
                .identity_hash()
                .as_bytes(),
        ),
    };
    let next = with_core(root.path(), |disk, core| {
        disk.create_or_recover(core, done, intent(), None).unwrap();
        assert!(matches!(
            disk.create_or_recover(core, pending, imported, None),
            Err(CreateError::Storage(IdentityError::ImportRequired))
        ));
        assert!(matches!(
            disk.retire_creations(core, &[pending]),
            Err(CreateError::Pending)
        ));
        assert!(matches!(
            disk.retire_creations(core, &[done, pending]),
            Err(CreateError::Pending)
        ));
        assert_eq!(disk.creation_scope().unwrap(), initial);
        let next = disk.retire_creations(core, &[done]).unwrap();
        assert_eq!(next.epoch, initial.epoch + 1);
        assert_eq!(disk.retire_creations(core, &[done]).unwrap(), next);
        assert!(matches!(
            disk.create_or_recover(core, done, intent(), None),
            Err(CreateError::Expired)
        ));
        assert!(matches!(
            disk.create_or_recover(core, request(initial, 3), intent(), None),
            Err(CreateError::Expired)
        ));
        next
    });
    with_core(root.path(), |disk, core| {
        let recovered = disk
            .create_or_recover(core, pending, imported, Some(&secret))
            .unwrap();
        assert!(matches!(recovered.result, OperationResult::Created { .. }));
        assert_eq!(core.snapshot().identities().count(), 2);
        assert_eq!(disk.creation_scope().unwrap(), next);
        assert!(matches!(
            disk.create_or_recover(core, done, intent(), None),
            Err(CreateError::Expired)
        ));
    });
}

#[test]
fn evidence_capacity_never_evicts_results_or_implicitly_rotates_scope() {
    type Small = IdentityStore<2, 2, 0>;
    let root = Root::new();
    let child = root.path().join("small");
    let mut disk = Small::create_new(&child).unwrap();
    let initial = disk.creation_scope().unwrap();
    let mut working = Catalog::<2, 2, 0>::new([0; 16]);
    let mut staging = working.clone();
    let mut core = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let generated = CreateRequest {
        source: KeySource::Generate,
        metadata: Metadata::new("Sensor", None).unwrap(),
        services: ServiceSet::empty(),
    };
    let first = disk
        .create_or_recover(&mut core, request(initial, 1), generated, None)
        .unwrap();
    let mut imported = generated;
    imported.source = KeySource::Import {
        expected_hash: core.identity(created(first)).unwrap().public_hash,
    };
    disk.create_or_recover(&mut core, request(initial, 2), imported, None)
        .unwrap();
    let issued = core.snapshot().issued_sequence;
    assert!(matches!(
        disk.create_or_recover(&mut core, request(initial, 3), imported, None),
        Err(CreateError::Capacity)
    ));
    assert_eq!(disk.creation_scope().unwrap(), initial);
    assert_eq!(core.snapshot().issued_sequence, issued);
    assert_eq!(
        disk.create_or_recover(&mut core, request(initial, 1), generated, None)
            .unwrap(),
        first
    );
    let next = disk
        .retire_creations(&mut core, &[request(initial, 1)])
        .unwrap();
    disk.create_or_recover(&mut core, request(next, 3), imported, None)
        .unwrap();
    assert!(matches!(
        disk.create_or_recover(&mut core, request(initial, 1), generated, None),
        Err(CreateError::Expired)
    ));
    assert_eq!(core.snapshot().identities().count(), 1);
}

#[cfg(feature = "fault-injection")]
fn child(root: &Path, mode: &str, point: &str) -> std::process::Output {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "create_crash_child", "--nocapture"])
        .env("PRNS_CREATE_TEST_ROOT", root)
        .env("PRNS_CREATE_TEST_MODE", mode)
        .env("PRNS_IDENTITIES_CRASH_AT", point)
        .output()
        .unwrap()
}
#[cfg(feature = "fault-injection")]
#[test]
fn create_crash_child() {
    let Some(root) = std::env::var_os("PRNS_CREATE_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let initial = scope(&root);
    with_core(&root, |disk, core| {
        let id = request(initial, 1);
        if std::env::var("PRNS_CREATE_TEST_MODE").unwrap() == "retire" {
            disk.retire_creations(core, &[id]).unwrap();
        } else {
            disk.create_or_recover(core, id, intent(), None).unwrap();
        }
    });
}

#[cfg(feature = "fault-injection")]
#[test]
fn every_creation_publication_boundary_recovers_one_request_and_one_exact_key() {
    let publication_steps = [
        "before_stage",
        "before_file_sync",
        "after_file_sync",
        "after_rename",
        "after_directory_sync",
    ];
    let mut points: Vec<String> = ["create-request", "key", "create-result", "create-prune"]
        .into_iter()
        .flat_map(|family| publication_steps.map(|step| format!("{family}.{step}")))
        .collect();
    points.extend(
        [
            "after_reserve",
            "after_begin",
            "after_key",
            "after_ack",
            "after_prune",
        ]
        .map(|step| format!("create.{step}")),
    );
    for point in points {
        let root = Root::new();
        let id = request(scope(root.path()), 1);
        let output = child(root.path(), "create", &point);
        assert_eq!(
            output.status.code(),
            Some(91),
            "{point}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let before = key_bytes(root.path());
        let outcome = with_core(root.path(), |disk, core| {
            let outcome = disk.create_or_recover(core, id, intent(), None).unwrap();
            assert_eq!(core.snapshot().identities().count(), 1, "{point}");
            assert_eq!(
                core.snapshot().issued_sequence,
                outcome.operation.sequence,
                "{point}"
            );
            assert_eq!(
                core.snapshot().completed_floor,
                outcome.operation.sequence,
                "{point}"
            );
            outcome
        });
        if let Some(before) = before {
            assert!(
                key_bytes(root.path()).unwrap() == before,
                "{point}: replaced key"
            );
        }
        with_core(root.path(), |disk, core| {
            assert_eq!(
                disk.create_or_recover(core, id, intent(), None).unwrap(),
                outcome,
                "{point}"
            );
            assert_eq!(core.snapshot().identities().count(), 1);
        });
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn interrupted_confirmation_has_complete_old_or_new_scope_and_never_recreates() {
    for step in [
        "before_stage",
        "before_file_sync",
        "after_file_sync",
        "after_rename",
        "after_directory_sync",
    ] {
        let root = Root::new();
        let id = request(scope(root.path()), 1);
        let outcome = with_core(root.path(), |disk, core| {
            disk.create_or_recover(core, id, intent(), None).unwrap()
        });
        let output = child(root.path(), "retire", &format!("create-retire.{step}"));
        assert_eq!(
            output.status.code(),
            Some(91),
            "{step}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        with_core(root.path(), |disk, core| {
            match disk.create_or_recover(core, id, intent(), None) {
                Ok(recovered) => assert_eq!(recovered, outcome),
                Err(CreateError::Expired) => {}
                other => panic!("unexpected result: {other:?}"),
            }
            assert_eq!(core.snapshot().identities().count(), 1);
            assert_eq!(core.snapshot().issued_sequence, outcome.operation.sequence);
            let next = disk.retire_creations(core, &[id]).unwrap();
            assert_eq!(next.epoch, id.scope.epoch + 1);
            assert!(matches!(
                disk.create_or_recover(core, id, intent(), None),
                Err(CreateError::Expired)
            ));
        });
    }
}

#[test]
fn unrelated_core_result_fences_native_pruning_until_its_caller_prunes() {
    let root = Root::new();
    let id = request(scope(root.path()), 1);
    with_core(root.path(), |disk, core| {
        let mut direct_intent = intent();
        direct_intent.metadata = Metadata::new("Independent Rust caller", None).unwrap();
        let command = Command::Create(direct_intent);
        let direct = core
            .reserve(disk, command, CommandDigest([71; 32]))
            .unwrap();
        core.begin(disk, direct, &command).unwrap();
        let ticket = core.action(direct).unwrap().unwrap().ticket();
        let key = disk.publish_key(core, ticket, None).unwrap();
        let direct_result = core.ack_key(disk, ticket, key).unwrap();
        let native = disk.create_or_recover(core, id, intent(), None).unwrap();
        assert_eq!(
            core.resume(direct, &command).unwrap(),
            OperationState::Completed(direct_result)
        );
        assert_eq!(core.snapshot().completed_floor, 0);
        assert!(matches!(
            disk.retire_creations(core, &[id]),
            Err(CreateError::PruningBlocked)
        ));
        assert_eq!(disk.creation_scope().unwrap(), id.scope);
        assert_eq!(
            disk.create_or_recover(core, id, intent(), None).unwrap(),
            native
        );
        assert_eq!(
            core.resume(direct, &command).unwrap(),
            OperationState::Completed(direct_result)
        );

        // Only the independent caller can confirm and prune its own result.
        core.prune_completed(disk, direct.sequence).unwrap();
        assert_eq!(
            disk.create_or_recover(core, id, intent(), None).unwrap(),
            native
        );
        assert_eq!(core.snapshot().completed_floor, native.operation.sequence);
        assert_eq!(core.snapshot().operations().count(), 0);
        assert_eq!(
            disk.retire_creations(core, &[id]).unwrap().epoch,
            id.scope.epoch + 1
        );
        assert!(matches!(
            disk.create_or_recover(core, id, intent(), None),
            Err(CreateError::Expired)
        ));
        assert_eq!(core.snapshot().identities().count(), 2);
    });
}

#[test]
fn failed_request_publication_poisons_until_reload_without_losing_retry_identity() {
    let root = Root::new();
    let id = request(scope(root.path()), 1);
    with_core(root.path(), |disk, core| {
        fs::create_dir(root.path().join("catalog.next")).unwrap();
        assert!(matches!(
            disk.create_or_recover(core, id, intent(), None),
            Err(CreateError::Storage(IdentityError::UnsafePath))
        ));
        assert!(core.is_poisoned());
        assert_eq!(core.snapshot().issued_sequence, 0);
        assert!(key_bytes(root.path()).is_none());
        assert!(matches!(
            disk.create_or_recover(core, id, intent(), None),
            Err(CreateError::Storage(IdentityError::Authority(
                Fault::Poisoned
            )))
        ));
        fs::remove_dir(root.path().join("catalog.next")).unwrap();
        core.reload(disk).unwrap();
        let result = disk.create_or_recover(core, id, intent(), None).unwrap();
        assert_eq!(
            disk.create_or_recover(core, id, intent(), None).unwrap(),
            result
        );
        assert_eq!(core.snapshot().identities().count(), 1);
    });
}

#[test]
fn old_or_corrupt_native_envelope_is_preserved_without_initializing_evidence() {
    let root = Root::new();
    let path = root.path().join("catalog.bin");
    let current = fs::read(&path).unwrap();
    fs::write(&path, &current[..current.len() - 1]).unwrap();
    assert!(matches!(
        Disk::open(root.path()),
        Err(IdentityError::Corrupt)
    ));
    assert!(fs::read(&path).unwrap() == current[..current.len() - 1]);

    // A valid old shared-native envelope, not an app-private compatibility format.
    let catalog = State::new([37; 16]);
    let mut body = vec![0; prns_identities_codec::max_encoded_len::<4, 8, 2>().unwrap()];
    let length = prns_identities_codec::encode(&catalog, &mut body).unwrap();
    body.truncate(length);
    let mut old = b"PNICAT01".to_vec();
    old.extend_from_slice(&(body.len() as u64).to_be_bytes());
    old.extend_from_slice(&body);
    old.extend_from_slice(&prns_core::crypto::sha256(&old));
    fs::write(&path, &old).unwrap();
    assert!(matches!(
        Disk::open(root.path()),
        Err(IdentityError::UnsupportedCatalogFormat)
    ));
    assert!(Disk::create_new(root.path()).is_err());
    assert!(fs::read(&path).unwrap() == old);
}
