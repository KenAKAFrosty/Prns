#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use prns_groups_store::{Coordinator, Limits, StoreError};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const NAMESPACE: StoreNamespace = StoreNamespace([3; 16]);
fn incarnation() -> StoreIncarnation {
    StoreIncarnation::new([9; 24]).unwrap()
}
fn ticket(sequence: u64) -> OperationTicket {
    OperationTicket::new(incarnation(), sequence).unwrap()
}
fn limits() -> RedbLimits {
    RedbLimits {
        max_changes: 8,
        max_key_bytes: 32,
        max_value_bytes: 128,
        max_staged_bytes: 512,
        max_live_objects: 8,
        max_live_bytes: 256,
        max_receipts: 4,
    }
}
fn staging_limits() -> Limits {
    Limits {
        max_changes: 8,
        max_key_bytes: 32,
        max_value_bytes: 128,
        max_staged_bytes: 512,
    }
}
fn put(key: &[u8], value: &[u8]) -> Mutation {
    Mutation::Put {
        key: key.to_vec(),
        value: value.to_vec(),
    }
}
fn receipt(sequence: u64) -> CommitReceipt {
    CommitReceipt {
        operation_id: ticket(sequence).operation_id(),
        revision: Revision(sequence),
    }
}

#[test]
fn ordinary_reserves_reject_atomically_and_preserve_management_capacity() {
    for reserve in [
        RedbReserve {
            objects: 7,
            bytes: 0,
            receipts: 0,
        },
        RedbReserve {
            objects: 0,
            bytes: 248,
            receipts: 0,
        },
        RedbReserve {
            objects: 0,
            bytes: 0,
            receipts: 3,
        },
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reserve.redb");
        {
            let db = Database::create(&path).unwrap();
            let mut owner = RedbStoreOwner::new(&db);
            let backend = owner
                .initialize(NAMESPACE, incarnation(), limits())
                .unwrap()
                .reserve_capacity(reserve)
                .unwrap();
            let coordinator = Coordinator::new(backend, staging_limits()).unwrap();
            let first = coordinator
                .begin(ticket(1).operation_id(), Revision(0))
                .unwrap();
            first.put(b"a", b"1234567").unwrap();
            assert_eq!(
                first.commit().unwrap(),
                CommitOutcome::Committed(receipt(1))
            );
            let second = coordinator
                .begin(ticket(2).operation_id(), Revision(1))
                .unwrap();
            second.put(b"a", b"changed").unwrap();
            second.put(b"b", b"v").unwrap();
            assert_eq!(
                second.commit().unwrap(),
                CommitOutcome::Rejected(OperationRejection::ReservedCapacity)
            );
            assert_eq!(coordinator.head().unwrap(), Revision(1));
            assert_eq!(
                coordinator.recover(ticket(2).operation_id()).unwrap(),
                RecoveryOutcome::NotCommitted {
                    revision: Revision(1)
                }
            );
        }
        let db = Database::open(&path).unwrap();
        let mut owner = RedbStoreOwner::new(&db);
        {
            let mut management = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
            assert_eq!(
                management.usage().unwrap(),
                RedbUsage {
                    revision: Revision(1),
                    retired_through: 0,
                    live_objects: 1,
                    live_bytes: 8,
                    receipts: 1,
                }
            );
            assert_eq!(
                management.read(b"a", 128).unwrap(),
                Some(b"1234567".to_vec())
            );
            assert_eq!(management.read(b"b", 128).unwrap(), None);
            assert_eq!(
                management
                    .commit(Revision(1), ticket(2).operation_id(), &[put(b"b", b"v")])
                    .unwrap(),
                CommitOutcome::Committed(receipt(2))
            );
        }
        let mut ordinary = owner
            .open(NAMESPACE, incarnation(), limits())
            .unwrap()
            .reserve_capacity(reserve)
            .unwrap();
        assert_eq!(
            ordinary.recover(ticket(2).operation_id()).unwrap(),
            RecoveryOutcome::Committed(receipt(2))
        );
        assert_eq!(
            ordinary
                .commit(
                    Revision(2),
                    ticket(2).operation_id(),
                    &[put(b"b", b"different")]
                )
                .unwrap(),
            CommitOutcome::Committed(receipt(2))
        );
        assert_eq!(ordinary.read(b"b", 128).unwrap(), Some(b"v".to_vec()));
    }
}

#[test]
fn reserves_check_final_usage_and_allow_receipt_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("reserve-cleanup.redb")).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    let mut backend = owner
        .initialize(NAMESPACE, incarnation(), limits())
        .unwrap()
        .reserve_capacity(RedbReserve {
            objects: 7,
            bytes: 248,
            receipts: 3,
        })
        .unwrap();
    assert_eq!(
        backend
            .commit(
                Revision(0),
                ticket(1).operation_id(),
                &[put(b"a", b"1234567")]
            )
            .unwrap(),
        CommitOutcome::Committed(receipt(1))
    );
    assert!(matches!(
        backend
            .maintain(RedbMaintenanceRequest::FinalizeThrough(ticket(1)))
            .unwrap(),
        MaintenanceOutcome::Complete(_)
    ));
    assert_eq!(
        backend
            .commit(
                Revision(1),
                ticket(2).operation_id(),
                &[
                    put(b"b", b"1234567"),
                    Mutation::Delete { key: b"a".to_vec() }
                ]
            )
            .unwrap(),
        CommitOutcome::Committed(receipt(2))
    );
    assert_eq!(
        backend.usage().unwrap(),
        RedbUsage {
            revision: Revision(2),
            retired_through: 1,
            live_objects: 1,
            live_bytes: 8,
            receipts: 1,
        }
    );
}

#[test]
fn reserves_cannot_exceed_persisted_limits() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("invalid-reserve.redb")).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    owner
        .initialize(NAMESPACE, incarnation(), limits())
        .unwrap();
    for reserve in [
        RedbReserve {
            objects: 9,
            bytes: 0,
            receipts: 0,
        },
        RedbReserve {
            objects: 0,
            bytes: 257,
            receipts: 0,
        },
        RedbReserve {
            objects: 0,
            bytes: 0,
            receipts: 5,
        },
    ] {
        assert!(matches!(
            owner
                .open(NAMESPACE, incarnation(), limits())
                .unwrap()
                .reserve_capacity(reserve),
            Err(RedbError::Configuration)
        ));
    }
    assert_eq!(
        owner
            .open(NAMESPACE, incarnation(), limits())
            .unwrap()
            .head()
            .unwrap(),
        Revision(0)
    );
}

#[test]
fn deletion_is_scoped_to_exact_incarnation_and_idempotent_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("delete.redb");
    let other = StoreNamespace([4; 16]);
    {
        let db = Database::create(&path).unwrap();
        let mut owner = RedbStoreOwner::new(&db);
        for namespace in [NAMESPACE, other] {
            let mut backend = owner
                .initialize(namespace, incarnation(), limits())
                .unwrap();
            backend
                .commit(
                    Revision(0),
                    ticket(1).operation_id(),
                    &[put(b"history", b"retained")],
                )
                .unwrap();
        }
        assert!(matches!(
            owner.validate_delete(NAMESPACE, StoreIncarnation::new([8; 24]).unwrap(), limits()),
            Err(RedbError::ForeignIncarnation)
        ));
        assert!(matches!(
            owner.delete(NAMESPACE, StoreIncarnation::new([8; 24]).unwrap(), limits()),
            Err(RedbError::ForeignIncarnation)
        ));
        assert!(matches!(
            owner.delete(
                NAMESPACE,
                incarnation(),
                RedbLimits {
                    max_receipts: 5,
                    ..limits()
                }
            ),
            Err(RedbError::Configuration)
        ));
        assert_eq!(
            owner
                .open(NAMESPACE, incarnation(), limits())
                .unwrap()
                .read(b"history", 128)
                .unwrap(),
            Some(b"retained".to_vec())
        );
        owner
            .validate_delete(NAMESPACE, incarnation(), limits())
            .unwrap();
        owner.delete(NAMESPACE, incarnation(), limits()).unwrap();
    }
    let db = Database::open(&path).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    owner
        .validate_delete(NAMESPACE, incarnation(), limits())
        .unwrap();
    owner.delete(NAMESPACE, incarnation(), limits()).unwrap();
    assert!(matches!(
        owner.open(NAMESPACE, incarnation(), limits()),
        Err(RedbError::MissingNamespace)
    ));
    assert_eq!(
        owner
            .open(other, incarnation(), limits())
            .unwrap()
            .read(b"history", 128)
            .unwrap(),
        Some(b"retained".to_vec())
    );
}

#[test]
fn deletion_never_formats_or_removes_partial_namespace_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("partial.redb")).unwrap();
    let names = TableNames::new(NAMESPACE);
    let write = db.begin_write().unwrap();
    write.open_table(names.values()).unwrap();
    write.commit().unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    assert!(matches!(
        owner.validate_delete(NAMESPACE, incarnation(), limits()),
        Err(RedbError::MissingNamespace)
    ));
    assert!(matches!(
        owner.delete(NAMESPACE, incarnation(), limits()),
        Err(RedbError::MissingNamespace)
    ));
    assert!(matches!(
        owner.initialize(NAMESPACE, incarnation(), limits()),
        Err(RedbError::AlreadyInitialized)
    ));
}

#[test]
fn reopen_preserves_atomic_crypto_outbox_and_projection_without_reapplying() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("application.redb");
    {
        let db = Database::create(&path).unwrap();
        let mut owner = RedbStoreOwner::new(&db);
        let backend = owner
            .initialize(NAMESPACE, incarnation(), limits())
            .unwrap();
        let coordinator = Coordinator::new(backend, staging_limits()).unwrap();
        let tx = coordinator
            .begin(ticket(1).operation_id(), Revision(0))
            .unwrap();
        tx.put(b"crypto", b"next ratchet").unwrap();
        tx.put(b"outbox", b"exact ciphertext").unwrap();
        tx.put(b"projection", b"pending record").unwrap();
        assert_eq!(tx.commit().unwrap(), CommitOutcome::Committed(receipt(1)));
    }
    let db = Database::open(&path).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
    assert_eq!(
        backend.recover(ticket(1).operation_id()).unwrap(),
        RecoveryOutcome::Committed(receipt(1))
    );
    assert_eq!(
        backend
            .commit(
                Revision(1),
                ticket(1).operation_id(),
                &[put(b"outbox", b"different ciphertext")]
            )
            .unwrap(),
        CommitOutcome::Committed(receipt(1))
    );
    assert_eq!(backend.head().unwrap(), Revision(1));
    for (key, value) in [
        (b"crypto".as_slice(), b"next ratchet".as_slice()),
        (b"outbox", b"exact ciphertext"),
        (b"projection", b"pending record"),
    ] {
        assert_eq!(backend.read(key, 128).unwrap(), Some(value.to_vec()));
    }
}

#[test]
fn same_database_owner_can_use_unrelated_application_tables() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("application.redb")).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    let mut backend = owner
        .initialize(NAMESPACE, incarnation(), limits())
        .unwrap();
    let application: redb::TableDefinition<&str, &str> =
        redb::TableDefinition::new("application.settings");
    let write = db.begin_write().unwrap();
    {
        write
            .open_table(application)
            .unwrap()
            .insert("name", "Independent app")
            .unwrap();
    }
    write.commit().unwrap();
    backend
        .commit(
            Revision(0),
            ticket(1).operation_id(),
            &[put(b"group", b"state")],
        )
        .unwrap();
    let read = db.begin_read().unwrap();
    assert_eq!(
        read.open_table(application)
            .unwrap()
            .get("name")
            .unwrap()
            .unwrap()
            .value(),
        "Independent app"
    );
}

#[test]
fn final_capacity_and_read_limits_are_deterministic_and_atomic() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("application.redb")).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    let bounded = RedbLimits {
        max_live_objects: 1,
        max_live_bytes: 8,
        ..limits()
    };
    let mut backend = owner.initialize(NAMESPACE, incarnation(), bounded).unwrap();
    backend
        .commit(
            Revision(0),
            ticket(1).operation_id(),
            &[put(b"a", b"1234567")],
        )
        .unwrap();
    assert!(matches!(backend.read(b"a", 6), Err(RedbError::Capacity)));
    assert!(matches!(
        backend.commit(
            Revision(1),
            ticket(2).operation_id(),
            &[put(b"b", b"too large")]
        ),
        Err(RedbError::Capacity)
    ));
    assert_eq!(
        backend.recover(ticket(2).operation_id()).unwrap(),
        RecoveryOutcome::NotCommitted {
            revision: Revision(1)
        }
    );
    assert_eq!(backend.read(b"a", 7).unwrap(), Some(b"1234567".to_vec()));
    assert_eq!(backend.read(b"b", 128).unwrap(), None);
    assert_eq!(
        backend
            .commit(
                Revision(1),
                ticket(2).operation_id(),
                &[
                    put(b"b", b"1234567"),
                    Mutation::Delete { key: b"a".to_vec() }
                ]
            )
            .unwrap(),
        CommitOutcome::Committed(receipt(2))
    );
    assert_eq!(
        backend.usage().unwrap(),
        RedbUsage {
            revision: Revision(2),
            retired_through: 0,
            live_objects: 1,
            live_bytes: 8,
            receipts: 2
        }
    );
}

#[test]
fn scoped_ordered_tickets_revision_and_retirement_are_durable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("application.redb");
    {
        let db = Database::create(&path).unwrap();
        let mut owner = RedbStoreOwner::new(&db);
        let mut backend = owner
            .initialize(
                NAMESPACE,
                incarnation(),
                RedbLimits {
                    max_receipts: 1,
                    ..limits()
                },
            )
            .unwrap();
        assert_eq!(
            backend.recover(OperationId([0; 32])).unwrap(),
            RecoveryOutcome::Rejected(OperationRejection::InvalidTicket)
        );
        let foreign = StoreIncarnation::new([8; 24]).unwrap().first();
        assert_eq!(
            backend.recover(foreign.operation_id()).unwrap(),
            RecoveryOutcome::Rejected(OperationRejection::ForeignIncarnation)
        );
        assert_eq!(
            backend.recover(ticket(3).operation_id()).unwrap(),
            RecoveryOutcome::Rejected(OperationRejection::OutOfOrder {
                next: Some(ticket(1))
            })
        );
        assert_eq!(
            backend
                .commit(Revision(1), ticket(1).operation_id(), &[])
                .unwrap(),
            CommitOutcome::NotCommitted {
                revision: Revision(0)
            }
        );
        backend
            .commit(
                Revision(0),
                ticket(1).operation_id(),
                &[put(b"state", b"kept")],
            )
            .unwrap();
        assert_eq!(
            backend.recover(ticket(2).operation_id()).unwrap(),
            RecoveryOutcome::Rejected(OperationRejection::RecoveryWindowFull)
        );
        assert_eq!(
            backend
                .commit(Revision(1), ticket(2).operation_id(), &[])
                .unwrap(),
            CommitOutcome::Rejected(OperationRejection::RecoveryWindowFull)
        );
        assert!(matches!(
            backend
                .maintain(RedbMaintenanceRequest::FinalizeThrough(foreign))
                .unwrap(),
            MaintenanceOutcome::Rejected(_)
        ));
        assert!(matches!(
            backend
                .maintain(RedbMaintenanceRequest::FinalizeThrough(ticket(2)))
                .unwrap(),
            MaintenanceOutcome::Rejected(_)
        ));
        assert!(matches!(
            backend
                .maintain(RedbMaintenanceRequest::FinalizeThrough(ticket(1)))
                .unwrap(),
            MaintenanceOutcome::Complete(_)
        ));
    }
    let db = Database::open(&path).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    let mut backend = owner
        .open(
            NAMESPACE,
            incarnation(),
            RedbLimits {
                max_receipts: 1,
                ..limits()
            },
        )
        .unwrap();
    assert_eq!(
        backend.recover(ticket(1).operation_id()).unwrap(),
        RecoveryOutcome::Retired {
            revision: Revision(1),
            retired_through: ticket(1)
        }
    );
    assert_eq!(
        backend
            .commit(
                Revision(1),
                ticket(1).operation_id(),
                &[put(b"state", b"replay")]
            )
            .unwrap(),
        CommitOutcome::Rejected(OperationRejection::RetiredTicket)
    );
    assert_eq!(backend.read(b"state", 128).unwrap(), Some(b"kept".to_vec()));
    backend
        .commit(Revision(1), ticket(2).operation_id(), &[])
        .unwrap();
    let coordinator = Coordinator::new(backend, staging_limits()).unwrap();
    assert!(matches!(
        coordinator.begin(ticket(1).operation_id(), Revision(2)),
        Err(StoreError::OutsideRecoveryWindow { .. })
    ));
}

#[test]
fn missing_corrupt_and_mismatched_metadata_never_reinitialize() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("application.redb")).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    assert!(matches!(
        owner.open(NAMESPACE, incarnation(), limits()),
        Err(RedbError::MissingNamespace)
    ));
    drop(
        owner
            .initialize(NAMESPACE, incarnation(), limits())
            .unwrap(),
    );
    assert!(matches!(
        owner.initialize(NAMESPACE, incarnation(), limits()),
        Err(RedbError::AlreadyInitialized)
    ));
    assert!(matches!(
        owner.open(NAMESPACE, StoreIncarnation::new([2; 24]).unwrap(), limits()),
        Err(RedbError::ForeignIncarnation)
    ));
    assert!(matches!(
        owner.open(
            NAMESPACE,
            incarnation(),
            RedbLimits {
                max_receipts: 8,
                ..limits()
            }
        ),
        Err(RedbError::Configuration)
    ));
    let names = TableNames::new(NAMESPACE);
    let write = db.begin_write().unwrap();
    {
        write
            .open_table(names.metadata())
            .unwrap()
            .remove(0)
            .unwrap();
    }
    write.commit().unwrap();
    assert!(matches!(
        owner.open(NAMESPACE, incarnation(), limits()),
        Err(RedbError::Corrupt)
    ));
    assert!(matches!(
        owner.initialize(NAMESPACE, incarnation(), limits()),
        Err(RedbError::AlreadyInitialized)
    ));
}

#[test]
fn missing_receipt_is_corruption_not_permission_to_reexecute() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("application.redb")).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    let mut backend = owner
        .initialize(NAMESPACE, incarnation(), limits())
        .unwrap();
    backend
        .commit(
            Revision(0),
            ticket(1).operation_id(),
            &[put(b"state", b"kept")],
        )
        .unwrap();
    drop(backend);
    let names = TableNames::new(NAMESPACE);
    let write = db.begin_write().unwrap();
    {
        write
            .open_table(names.receipts())
            .unwrap()
            .remove(1)
            .unwrap();
    }
    write.commit().unwrap();
    assert!(matches!(
        owner.open(NAMESPACE, incarnation(), limits()),
        Err(RedbError::Corrupt)
    ));
}

struct LostReply<'a> {
    backend: RedbBackend<'a>,
}
impl AtomicBackend for LostReply<'_> {
    type Error = RedbError;
    fn head(&mut self) -> Result<Revision, Self::Error> {
        self.backend.head()
    }
    fn read(&mut self, key: &[u8], max: usize) -> Result<Option<Vec<u8>>, Self::Error> {
        self.backend.read(key, max)
    }
    fn commit(
        &mut self,
        revision: Revision,
        operation: OperationId,
        changes: &[Mutation],
    ) -> Result<CommitOutcome, Self::Error> {
        let result = self.backend.commit(revision, operation, changes)?;
        assert!(matches!(result, CommitOutcome::Committed(_)));
        Ok(CommitOutcome::Uncertain)
    }
    fn recover(&mut self, operation: OperationId) -> Result<RecoveryOutcome, Self::Error> {
        self.backend.recover(operation)
    }
}
#[test]
fn lost_commit_response_fences_coordinator_until_durable_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("application.redb")).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    let backend = owner
        .initialize(NAMESPACE, incarnation(), limits())
        .unwrap();
    let coordinator = Coordinator::new(LostReply { backend }, staging_limits()).unwrap();
    let tx = coordinator
        .begin(ticket(1).operation_id(), Revision(0))
        .unwrap();
    tx.put(b"outbox", b"exact bytes").unwrap();
    assert_eq!(tx.commit().unwrap(), CommitOutcome::Uncertain);
    assert!(matches!(
        coordinator.begin(ticket(2).operation_id(), Revision(0)),
        Err(StoreError::RecoveryRequired(_))
    ));
    assert_eq!(
        coordinator.recover(ticket(1).operation_id()).unwrap(),
        RecoveryOutcome::Committed(receipt(1))
    );
    assert!(matches!(
        coordinator.begin(ticket(1).operation_id(), Revision(1)),
        Err(StoreError::AlreadyCommitted(_))
    ));
}

// These barriers compile only in the crate's unit-test binary. Production has
// neither a fault hook nor environment-controlled behavior.
pub(super) fn crash_point(point: &str) {
    if std::env::var("PRNS_NATIVE_CRASH_POINT").ok().as_deref() != Some(point) {
        return;
    }
    std::fs::write(std::env::var_os("PRNS_NATIVE_MARKER").unwrap(), point).unwrap();
    loop {
        std::thread::park();
    }
}
#[test]
#[ignore = "subprocess helper invoked by process_death_at_durable_boundaries"]
fn crash_child() {
    let Some(path) = std::env::var_os("PRNS_NATIVE_DB") else {
        return;
    };
    let db = Database::open(path).unwrap();
    let mut owner = RedbStoreOwner::new(&db);
    let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
    match std::env::var("PRNS_NATIVE_CRASH_POINT").unwrap().as_str() {
        "before-commit" | "after-commit" => {
            backend
                .commit(
                    Revision(1),
                    ticket(2).operation_id(),
                    &[
                        put(b"crypto", b"next ratchet"),
                        put(b"outbox", b"exact ciphertext"),
                        put(b"projection", b"pending record"),
                    ],
                )
                .unwrap();
        }
        "before-retirement" | "after-retirement" => {
            backend
                .maintain(RedbMaintenanceRequest::FinalizeThrough(ticket(1)))
                .unwrap();
        }
        _ => panic!("invalid test boundary"),
    }
    panic!("child did not reach its boundary");
}
#[test]
fn process_death_at_durable_boundaries_preserves_all_or_none_and_receipt_fences() {
    for point in [
        "before-commit",
        "after-commit",
        "before-retirement",
        "after-retirement",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("application.redb");
        let marker = directory.path().join("barrier");
        {
            let db = Database::create(&path).unwrap();
            let mut owner = RedbStoreOwner::new(&db);
            let mut backend = owner
                .initialize(NAMESPACE, incarnation(), limits())
                .unwrap();
            backend
                .commit(
                    Revision(0),
                    ticket(1).operation_id(),
                    &[put(b"existing", b"kept")],
                )
                .unwrap();
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::crash_child", "--ignored", "--nocapture"])
            .env("PRNS_NATIVE_DB", &path)
            .env("PRNS_NATIVE_MARKER", &marker)
            .env("PRNS_NATIVE_CRASH_POINT", point)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !marker.exists() && Instant::now() < deadline {
            assert!(
                child.try_wait().unwrap().is_none(),
                "child exited before {point}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(marker.exists(), "child did not reach {point}");
        let db = Database::open(&path).unwrap();
        let mut owner = RedbStoreOwner::new(&db);
        let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
        assert_eq!(
            backend.read(b"existing", 128).unwrap(),
            Some(b"kept".to_vec())
        );
        let committed = point == "after-commit";
        assert_eq!(
            backend.head().unwrap(),
            Revision(if committed { 2 } else { 1 })
        );
        for (key, value) in [
            (b"crypto".as_slice(), b"next ratchet".as_slice()),
            (b"outbox", b"exact ciphertext"),
            (b"projection", b"pending record"),
        ] {
            assert_eq!(
                backend.read(key, 128).unwrap(),
                committed.then(|| value.to_vec()),
                "{point}"
            );
        }
        if point.ends_with("commit") {
            assert_eq!(
                backend.recover(ticket(2).operation_id()).unwrap(),
                if committed {
                    RecoveryOutcome::Committed(receipt(2))
                } else {
                    RecoveryOutcome::NotCommitted {
                        revision: Revision(1),
                    }
                }
            );
        }
        if point == "after-retirement" {
            assert_eq!(
                backend.recover(ticket(1).operation_id()).unwrap(),
                RecoveryOutcome::Retired {
                    revision: Revision(1),
                    retired_through: ticket(1)
                }
            );
        } else {
            assert_eq!(
                backend.recover(ticket(1).operation_id()).unwrap(),
                RecoveryOutcome::Committed(receipt(1))
            );
        }
    }
}

#[test]
fn write_operations_never_recreate_missing_empty_reserved_tables() {
    for missing_receipts in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::create(directory.path().join("application.redb")).unwrap();
        let mut owner = RedbStoreOwner::new(&db);
        let mut backend = owner
            .initialize(NAMESPACE, incarnation(), limits())
            .unwrap();
        let names = TableNames::new(NAMESPACE);
        if missing_receipts {
            backend
                .commit(Revision(0), ticket(1).operation_id(), &[])
                .unwrap();
            backend
                .maintain(RedbMaintenanceRequest::FinalizeThrough(ticket(1)))
                .unwrap();
        }
        let write = db.begin_write().unwrap();
        if missing_receipts {
            write.delete_table(names.receipts()).unwrap();
        } else {
            write.delete_table(names.values()).unwrap();
        }
        write.commit().unwrap();
        if missing_receipts {
            assert!(matches!(
                backend.maintain(RedbMaintenanceRequest::FinalizeThrough(ticket(1))),
                Err(RedbError::MissingNamespace)
            ));
        } else {
            assert!(matches!(
                backend.commit(Revision(0), ticket(1).operation_id(), &[]),
                Err(RedbError::MissingNamespace)
            ));
        }
        assert!(matches!(backend.head(), Err(RedbError::MissingNamespace)));
    }
}

struct InspectStagedValues {
    reject: bool,
}
impl TransactionParticipant for InspectStagedValues {
    fn stage(
        &self,
        tx: &redb::WriteTransaction,
        context: CommitContext,
        values: &CommitRead<'_>,
        ticket: OperationTicket,
        _: &[Mutation],
    ) -> Result<Option<OperationRejection>, RedbError> {
        assert_eq!(context.namespace, NAMESPACE);
        assert_eq!(context.incarnation, incarnation());
        assert_eq!(ticket.sequence(), 2);
        assert!(values.with_value(b"keep", |value| value == Some(b"old".as_slice()))?);
        assert!(values.with_value(b"replace", |value| value == Some(b"new".as_slice()))?);
        assert!(values.with_value(b"insert", |value| value == Some(b"added".as_slice()))?);
        assert!(values.with_value(b"delete", |value| value.is_none())?);
        assert!(matches!(
            values.with_value(&[0; 33], |_| ()),
            Err(RedbError::Capacity)
        ));
        tx.open_table(redb::TableDefinition::<u8, u8>::new("participant-proof"))
            .map_err(db_error)?
            .insert(0, 1)
            .map_err(db_error)?;
        Ok(self.reject.then_some(OperationRejection::StorageCapacity))
    }
    fn delete_in(&self, _: &redb::WriteTransaction, _: CommitContext) -> Result<(), RedbError> {
        Ok(())
    }
}

#[test]
fn participant_reads_exact_staged_values_and_rejection_rolls_back_both_owners() {
    for reject in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::create(directory.path().join("participant-read.redb")).unwrap();
        let mut owner = RedbStoreOwner::new(&db);
        {
            let mut backend = owner
                .initialize(NAMESPACE, incarnation(), limits())
                .unwrap();
            assert_eq!(
                backend
                    .commit(
                        Revision(0),
                        ticket(1).operation_id(),
                        &[
                            put(b"keep", b"old"),
                            put(b"replace", b"old"),
                            put(b"delete", b"old")
                        ]
                    )
                    .unwrap(),
                CommitOutcome::Committed(receipt(1))
            );
        }
        {
            let mut backend = owner
                .open(NAMESPACE, incarnation(), limits())
                .unwrap()
                .with_participant(Arc::new(InspectStagedValues { reject }));
            let result = backend
                .commit(
                    Revision(1),
                    ticket(2).operation_id(),
                    &[
                        put(b"replace", b"new"),
                        put(b"insert", b"added"),
                        Mutation::Delete {
                            key: b"delete".to_vec(),
                        },
                    ],
                )
                .unwrap();
            if reject {
                assert_eq!(
                    result,
                    CommitOutcome::Rejected(OperationRejection::StorageCapacity)
                );
            } else {
                assert_eq!(result, CommitOutcome::Committed(receipt(2)));
            }
        }
        let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
        assert_eq!(backend.read(b"keep", 128).unwrap(), Some(b"old".to_vec()));
        assert_eq!(
            backend.read(b"replace", 128).unwrap(),
            Some(if reject { b"old" } else { b"new" }.to_vec())
        );
        assert_eq!(
            backend.read(b"delete", 128).unwrap(),
            reject.then(|| b"old".to_vec())
        );
        assert_eq!(
            backend.read(b"insert", 128).unwrap(),
            (!reject).then(|| b"added".to_vec())
        );
        let read = db.begin_read().unwrap();
        let proof = read.open_table(redb::TableDefinition::<u8, u8>::new("participant-proof"));
        if reject {
            assert!(matches!(proof, Err(redb::TableError::TableDoesNotExist(_))));
        } else {
            assert_eq!(proof.unwrap().get(0).unwrap().unwrap().value(), 1);
        }
    }
}
