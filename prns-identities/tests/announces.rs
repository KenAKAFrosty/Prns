#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use prns_identities::announces::{AnnounceKey, Error, FileAnnounceScheduler};
use prns_identities::Error as StoreError;
use prns_identities_core::{LocalIdentityId, ServiceId};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

type Owner = FileAnnounceScheduler<4>;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "prns-announce-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
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
fn key(id: u64) -> AnnounceKey {
    AnnounceKey {
        identity: LocalIdentityId {
            incarnation: [8; 16],
            sequence: id,
        },
        service: ServiceId(1),
    }
}

#[test]
fn exclusive_owner_partial_results_and_unfinished_reservations_survive_restart() {
    let root = Root::new();
    let mut owner = Owner::create_new(root.path(), 0, Some(100)).unwrap();
    assert!(matches!(
        Owner::open(root.path(), 0, None),
        Err(Error::Storage(StoreError::Busy))
    ));
    owner
        .configure(key(1), 3, Some(900_000), true, 0, None)
        .unwrap();
    owner
        .configure(key(2), 1, Some(900_000), true, 0, None)
        .unwrap();
    let first = owner.reserve_next(0, None, true, 0).unwrap().unwrap();
    owner.complete(first, 1, 2).unwrap();
    drop(owner);
    let mut owner = Owner::open(root.path(), 0, None).unwrap();
    let attempt = owner.snapshot().entries[0].unwrap().last_attempt.unwrap();
    assert_eq!((attempt.requested_mask, attempt.failed_mask), (1, 2));
    assert!(owner.reserve_next(59_999, None, true, 0).unwrap().is_none());
    let next = owner.reserve_next(60_000, None, true, 0).unwrap().unwrap();
    assert_eq!(next.key(), key(2));
    drop(owner); // No completion; reservation still spends the budget.
    let mut owner = Owner::open(root.path(), 500, None).unwrap();
    assert!(
        !owner.snapshot().entries[1]
            .unwrap()
            .last_attempt
            .unwrap()
            .completed
    );
    assert!(owner.reserve_next(60_499, None, true, 0).unwrap().is_none());
    let recovered = owner.reserve_next(60_500, None, true, 0).unwrap().unwrap();
    assert_eq!(recovered.sequence(), next.sequence() + 1);
    assert_eq!(recovered.key(), key(1));
}

#[test]
fn corrupt_missing_and_wrong_capacity_state_never_reinitialize() {
    let root = Root::new();
    drop(Owner::create_new(root.path(), 0, None).unwrap());
    let path = root.path().join("state.bin");
    let original = fs::read(&path).unwrap();
    assert!(FileAnnounceScheduler::<3>::open(root.path(), 0, None).is_err());
    for bad in [Vec::new(), original[..original.len() - 1].to_vec(), {
        let mut bad = original.clone();
        bad[20] ^= 1;
        bad
    }] {
        fs::write(&path, &bad).unwrap();
        assert!(Owner::open(root.path(), 0, None).is_err());
        assert_eq!(fs::read(&path).unwrap(), bad);
    }
    fs::remove_file(&path).unwrap();
    assert!(matches!(
        Owner::open(root.path(), 0, None),
        Err(Error::Storage(StoreError::MissingCatalog))
    ));
    assert!(Owner::create_new(root.path(), 0, None).is_err());
    assert!(!path.exists());
}

#[cfg(feature = "fault-injection")]
#[test]
fn announce_crash_child() {
    let Some(path) = std::env::var_os("PRNS_ANNOUNCE_TEST_ROOT") else {
        return;
    };
    let mut owner = Owner::open(path, 0, None).unwrap();
    // Initial configuration is already durable. Exit can happen at each atomic
    // replacement boundary of the attempt reservation, before dispatch exists.
    owner.reserve_next(60_000, None, true, 0).unwrap().unwrap();
}

#[cfg(feature = "fault-injection")]
#[test]
fn process_crashes_at_every_reservation_publication_boundary_are_recoverable() {
    for point in [
        "before_stage",
        "before_file_sync",
        "after_file_sync",
        "after_rename",
        "after_directory_sync",
    ] {
        let root = Root::new();
        let mut owner = Owner::create_new(root.path(), 0, None).unwrap();
        owner
            .configure(key(1), 3, Some(900_000), true, 0, None)
            .unwrap();
        owner.reserve_next(0, None, true, 0).unwrap().unwrap();
        drop(owner);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "announce_crash_child", "--nocapture"])
            .env("PRNS_ANNOUNCE_TEST_ROOT", root.path())
            .env("PRNS_IDENTITIES_CRASH_AT", format!("announces.{point}"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(91), "{point}: {:?}", output);
        let mut recovered = Owner::open(root.path(), 0, None).unwrap();
        let sequence = recovered.snapshot().last_sequence;
        assert_eq!(
            sequence,
            if matches!(point, "after_rename" | "after_directory_sync") {
                2
            } else {
                1
            }
        );
        assert!(recovered
            .reserve_next(59_999, None, true, 0)
            .unwrap()
            .is_none());
        assert_eq!(
            recovered
                .reserve_next(60_000, None, true, 0)
                .unwrap()
                .unwrap()
                .sequence(),
            sequence + 1
        );
    }
}

#[test]
fn manual_pause_preferences_and_global_fairness_share_the_reopened_budget() {
    use prns_identities::announces::ManualRequest;
    let root = Root::new();
    let mut owner = Owner::create_new(root.path(), 0, None).unwrap();
    for id in [1, 2] {
        owner.configure(key(id), 3, None, true, 0, None).unwrap();
        assert_eq!(
            owner.request_manual(key(id), 0, None).unwrap(),
            ManualRequest::Queued
        );
    }
    // Configured-but-offline interfaces spend neither pending intent nor budget.
    assert!(owner.reserve_next(0, None, false, 0).unwrap().is_none());
    let first = owner.reserve_next(0, None, true, 0).unwrap().unwrap();
    assert_eq!(first.key(), key(1));
    owner.complete(first, 1, 2).unwrap();
    // Pause discards coalesced work. Editing preferences/resuming cannot refund
    // the shared attempt, even for the other identity.
    owner.configure(key(2), 3, None, false, 1, None).unwrap();
    assert_eq!(
        owner.request_manual(key(2), 1, None).unwrap(),
        ManualRequest::Disabled
    );
    owner.configure(key(2), 3, None, true, 2, None).unwrap();
    assert!(owner.reserve_next(60_000, None, true, 0).unwrap().is_none());
    owner
        .configure(key(1), 3, Some(900_000), true, 60_000, None)
        .unwrap();
    assert!(owner.reserve_next(60_000, None, true, 0).unwrap().is_none());
    drop(owner);
    let mut owner = Owner::open(root.path(), 0, None).unwrap();
    assert_eq!(
        owner.snapshot().entries[0]
            .unwrap()
            .configuration
            .interval_ms,
        Some(900_000)
    );
    assert_eq!(
        owner.snapshot().entries[1]
            .unwrap()
            .configuration
            .interval_ms,
        None
    );
    assert_eq!(
        owner.request_manual(key(2), 0, None).unwrap(),
        ManualRequest::Throttled {
            retry_after_ms: 60_000
        }
    );
    assert!(owner.reserve_next(59_999, None, true, 0).unwrap().is_none());
    // The persisted round-robin cursor selects identity two before identity
    // one's automatic startup request. Restart cannot reset that fairness.
    let second = owner.reserve_next(60_000, None, true, 0).unwrap().unwrap();
    assert_eq!(second.key(), key(2));
    assert_eq!(second.sequence(), first.sequence() + 1);
}
