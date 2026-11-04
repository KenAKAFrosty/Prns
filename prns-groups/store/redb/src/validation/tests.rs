#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::{metadata::TableNames, EnrollmentSnapshot, RedbLimits, RedbMaintenanceRequest};
use prns_groups_store::{
    AtomicBackend, CommitOutcome, MaintenanceBackend, Mutation, OperationTicket, RecoveryOutcome,
    Revision, StoreIncarnation,
};
use redb::ReadableDatabase;
use std::cell::Cell;

const NAMESPACE: StoreNamespace = StoreNamespace([3; 16]);
thread_local! {
    static FAILURE: Cell<Option<&'static str>> = const { Cell::new(None) };
}
pub(crate) fn fail_point(point: &'static str) -> Result<(), RedbError> {
    if FAILURE.with(|failure| failure.get() == Some(point)) {
        FAILURE.with(|failure| failure.set(None));
        return Err(RedbError::Database(
            std::io::Error::other("validation cache commit cut").into(),
        ));
    }
    Ok(())
}
fn incarnation() -> StoreIncarnation {
    StoreIncarnation::new([9; 24]).unwrap()
}
fn ticket(sequence: u64) -> OperationTicket {
    OperationTicket::new(incarnation(), sequence).unwrap()
}
fn limits() -> RedbLimits {
    RedbLimits {
        max_changes: 32,
        max_key_bytes: 32,
        max_value_bytes: 128,
        max_staged_bytes: 4096,
        max_live_objects: 1024,
        max_live_bytes: 256 * 1024,
        max_receipts: 1024,
    }
}
fn database() -> (tempfile::TempDir, Arc<Database>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::create(dir.path().join("cache.redb")).unwrap());
    (dir, db)
}
fn seed(db: &Database, namespace: StoreNamespace, count: u32) {
    let values: Vec<_> = (0..count)
        .map(|key| (key.to_be_bytes().to_vec(), vec![7; 128]))
        .collect();
    let mut state = State::initial(incarnation(), limits().checked().unwrap()).unwrap();
    state.objects = count as usize;
    state.bytes = count as usize * 132;
    state.revision = Revision(u64::from(count));
    let snapshot = EnrollmentSnapshot {
        metadata: state.encode().unwrap().to_vec(),
        values,
        receipts: (1..=u64::from(count)).map(|seq| (seq, seq)).collect(),
    };
    RedbStoreOwner::new(db)
        .import_enrollment(namespace, incarnation(), limits(), &snapshot)
        .unwrap();
}
fn make_cache(db: &Arc<Database>, capacity: usize) -> ValidationCache {
    ValidationCache::new(Arc::clone(db), NonZeroUsize::new(capacity).unwrap()).unwrap()
}

#[test]
fn lexical_reopen_checks_metadata_without_rescanning_history_and_advances_after_writes() {
    let (_dir, db) = database();
    seed(&db, NAMESPACE, 256);
    let mut cache = make_cache(&db, 2);
    cache
        .owner()
        .open(NAMESPACE, incarnation(), limits())
        .unwrap();
    let first = cache.stats();
    assert_eq!(first.full_validations, 1);
    assert_eq!(first.values_inspected, 256);
    assert_eq!(first.receipts_inspected, 256);
    assert_eq!(first.payload_bytes_inspected, 256 * 132);
    for _ in 0..100 {
        let mut owner = cache.owner();
        let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
        assert_eq!(backend.head().unwrap(), Revision(256));
        assert_eq!(
            backend.read(&255_u32.to_be_bytes(), 128).unwrap(),
            Some(vec![7; 128])
        );
        assert!(matches!(
            backend.recover(ticket(256).operation_id()).unwrap(),
            RecoveryOutcome::Committed(_)
        ));
    }
    assert_eq!(cache.stats().full_validations, first.full_validations);
    assert_eq!(cache.stats().values_inspected, first.values_inspected);
    assert_eq!(cache.stats().receipts_inspected, first.receipts_inspected);
    assert!(cache.stats().cache_hits >= 400);
    {
        let mut owner = cache.owner();
        let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
        assert!(matches!(
            backend
                .commit(
                    Revision(256),
                    ticket(257).operation_id(),
                    &[Mutation::Put {
                        key: 255_u32.to_be_bytes().to_vec(),
                        value: vec![9; 16]
                    },]
                )
                .unwrap(),
            CommitOutcome::Committed(_)
        ));
        backend
            .maintain(RedbMaintenanceRequest::FinalizeThrough(ticket(128)))
            .unwrap();
    }
    let usage = cache
        .owner()
        .open(NAMESPACE, incarnation(), limits())
        .unwrap()
        .usage()
        .unwrap();
    assert_eq!(usage.live_bytes, 256 * 132 - 112);
    assert_eq!(usage.receipts, 129);
    assert_eq!(usage.revision, Revision(257));
    assert_eq!(cache.stats().full_validations, 1);
    cache
        .owner()
        .open(NAMESPACE, incarnation(), limits())
        .unwrap()
        .inspect()
        .unwrap();
    assert_eq!(cache.stats().full_validations, 2);
}

#[test]
fn thousand_namespace_working_set_and_small_cache_eviction_report_bounded_search_work() {
    let (_dir, db) = database();
    let namespaces: Vec<_> = (0_u32..1000)
        .map(|index| {
            let mut bytes = [0; 16];
            bytes[..4].copy_from_slice(&index.to_be_bytes());
            StoreNamespace(bytes)
        })
        .collect();
    // Cache workload fixture, not a managed-enrollment capacity claim. Seed all
    // valid namespaces atomically; every cache still has to inspect each on miss.
    let write = db.begin_write().unwrap();
    for namespace in &namespaces {
        let names = TableNames::new(*namespace);
        let mut state = State::initial(incarnation(), limits().checked().unwrap()).unwrap();
        state.objects = 1;
        state.bytes = 132;
        state.revision = Revision(1);
        write
            .open_table(names.metadata())
            .unwrap()
            .insert(0, state.encode().unwrap().as_slice())
            .unwrap();
        write
            .open_table(names.values())
            .unwrap()
            .insert(0_u32.to_be_bytes().as_slice(), [7; 128].as_slice())
            .unwrap();
        write
            .open_table(names.receipts())
            .unwrap()
            .insert(1, 1)
            .unwrap();
    }
    write.commit().unwrap();
    let mut resident = make_cache(&db, 1000);
    for namespace in &namespaces {
        resident
            .owner()
            .open(*namespace, incarnation(), limits())
            .unwrap();
    }
    let initial = resident.stats();
    assert_eq!(initial.full_validations, 1000);
    for namespace in namespaces.iter().rev() {
        let mut owner = resident.owner();
        let mut backend = owner.open(*namespace, incarnation(), limits()).unwrap();
        backend.head().unwrap();
    }
    let warm = resident.stats();
    assert_eq!(warm.full_validations, 1000);
    assert_eq!(warm.values_inspected, 1000);
    assert_eq!(warm.receipts_inspected, 1000);
    assert!(warm.key_comparisons - initial.key_comparisons <= 22_000);
    assert_eq!(warm.index_entries_shifted, initial.index_entries_shifted);
    assert_eq!(warm.evictions, 0);
    // Invalidation for a hot write also uses the index, not a resident-set scan.
    let before = resident.stats();
    resident
        .owner()
        .open(namespaces[500], incarnation(), limits())
        .unwrap()
        .commit(Revision(1), ticket(2).operation_id(), &[])
        .unwrap();
    assert!(resident.stats().key_comparisons - before.key_comparisons <= 44);
    assert_eq!(
        resident.stats().index_entries_shifted,
        before.index_entries_shifted
    );

    let mut small = make_cache(&db, 16);
    for namespace in namespaces.iter().cycle().take(2000) {
        small
            .owner()
            .open(*namespace, incarnation(), limits())
            .unwrap();
    }
    let pressured = small.stats();
    assert_eq!(small.entries.borrow().rows.len(), 16);
    assert!(pressured.full_validations > 1900);
    assert!(pressured.evictions > 1800);
    assert!(pressured.key_comparisons <= 30_000);
    assert!(pressured.index_entries_shifted <= 64_000);
    println!("validation_work resident_capacity=1000 warm={warm:?} small_capacity=16 pressure={pressured:?}");
}

#[test]
fn bounded_eviction_never_substitutes_another_namespace_or_configuration() {
    let (_dir, db) = database();
    let namespaces = [StoreNamespace([1; 16]), StoreNamespace([2; 16]), NAMESPACE];
    for namespace in namespaces {
        seed(&db, namespace, 8);
    }
    let mut cache = make_cache(&db, 2);
    for namespace in namespaces {
        cache
            .owner()
            .open(namespace, incarnation(), limits())
            .unwrap();
    }
    assert_eq!(cache.stats().full_validations, 3);
    assert_eq!(cache.entries.borrow().rows.len(), 2);
    cache
        .owner()
        .open(namespaces[0], incarnation(), limits())
        .unwrap();
    assert_eq!(cache.stats().full_validations, 4);
    assert!(matches!(
        cache.owner().open(
            namespaces[0],
            StoreIncarnation::new([8; 24]).unwrap(),
            limits()
        ),
        Err(RedbError::ForeignIncarnation)
    ));
    assert!(matches!(
        cache.owner().open(
            NAMESPACE,
            incarnation(),
            RedbLimits {
                max_live_bytes: limits().max_live_bytes + 1,
                ..limits()
            }
        ),
        Err(RedbError::Configuration)
    ));
    // A separate strict owner also cannot carry cache proof between attachments.
    let mut fresh = make_cache(&db, 2);
    fresh
        .owner()
        .open(NAMESPACE, incarnation(), limits())
        .unwrap();
    assert_eq!(fresh.stats().full_validations, 1);
}

#[test]
fn exact_allocation_binding_and_drop_release_the_physical_lock() {
    let (dir, db) = database();
    seed(&db, NAMESPACE, 2);
    let (_other_dir, other) = database();
    seed(&other, NAMESPACE, 2); // same namespace/incarnation/metadata/rows
    let cache = make_cache(&db, 2);
    assert!(cache.matches_database(&db));
    assert!(!cache.matches_database(&other));
    drop(db);
    assert!(Database::open(dir.path().join("cache.redb")).is_err());
    drop(cache);
    Database::open(dir.path().join("cache.redb")).unwrap();
}

#[test]
fn missing_empty_table_is_not_created_by_a_cached_commit() {
    let (_dir, db) = database();
    seed(&db, NAMESPACE, 0);
    let mut cache = make_cache(&db, 2);
    let mut owner = cache.owner();
    let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
    let write = db.begin_write().unwrap();
    write
        .delete_table(TableNames::new(NAMESPACE).values())
        .unwrap();
    write.commit().unwrap();
    assert!(matches!(
        backend.commit(Revision(0), ticket(1).operation_id(), &[]),
        Err(RedbError::MissingNamespace)
    ));
    assert!(matches!(
        db.begin_read()
            .unwrap()
            .open_table(TableNames::new(NAMESPACE).values()),
        Err(redb::TableError::TableDoesNotExist(_))
    ));
}

#[test]
fn exact_recovery_checks_receipt_and_full_inspection_detects_out_of_contract_edits() {
    let (_dir, db) = database();
    seed(&db, NAMESPACE, 8);
    let mut cache = make_cache(&db, 2);
    cache
        .owner()
        .open(NAMESPACE, incarnation(), limits())
        .unwrap();
    let names = TableNames::new(NAMESPACE);
    let write = db.begin_write().unwrap();
    write
        .open_table(names.receipts())
        .unwrap()
        .insert(4, 5)
        .unwrap();
    write.commit().unwrap();
    let mut owner = cache.owner();
    let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
    assert!(matches!(
        backend.recover(ticket(4).operation_id()),
        Err(RedbError::Corrupt)
    ));
    assert!(matches!(backend.inspect(), Err(RedbError::Corrupt)));
    drop(backend);
    drop(owner);
    let write = db.begin_write().unwrap();
    write
        .open_table(names.receipts())
        .unwrap()
        .insert(4, 4)
        .unwrap();
    write.commit().unwrap();
    cache
        .owner()
        .open(NAMESPACE, incarnation(), limits())
        .unwrap();
    let write = db.begin_write().unwrap();
    write
        .open_table(names.values())
        .unwrap()
        .insert(7_u32.to_be_bytes().as_slice(), b"changed".as_slice())
        .unwrap();
    write.commit().unwrap();
    // A metadata/count cache is intentionally not a detector of arbitrary raw
    // edits with an unchanged header. Full inspection and a fresh open are.
    let mut owner = cache.owner();
    let backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
    assert_eq!(backend.usage().unwrap().live_objects, 8);
    assert!(matches!(backend.inspect(), Err(RedbError::Corrupt)));
    assert!(matches!(
        RedbStoreOwner::new(&db).open(NAMESPACE, incarnation(), limits()),
        Err(RedbError::Corrupt)
    ));
}

#[test]
fn count_changes_invalidate_even_a_matching_header() {
    let (_dir, db) = database();
    seed(&db, NAMESPACE, 8);
    let mut cache = make_cache(&db, 2);
    cache
        .owner()
        .open(NAMESPACE, incarnation(), limits())
        .unwrap();
    let write = db.begin_write().unwrap();
    write
        .open_table(TableNames::new(NAMESPACE).values())
        .unwrap()
        .remove(7_u32.to_be_bytes().as_slice())
        .unwrap();
    write.commit().unwrap();
    assert!(matches!(
        cache.owner().open(NAMESPACE, incarnation(), limits()),
        Err(RedbError::Corrupt)
    ));
    assert!(cache
        .entries
        .borrow()
        .rows
        .iter()
        .all(|(_, state)| state.is_none()));
}

#[test]
fn commit_uncertainty_discards_proof_before_and_after_the_physical_commit() {
    for point in [
        "before-commit",
        "after-commit",
        "before-retirement",
        "after-retirement",
    ] {
        let (_dir, db) = database();
        seed(&db, NAMESPACE, 8);
        let mut cache = make_cache(&db, 2);
        {
            let mut owner = cache.owner();
            let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
            FAILURE.with(|failure| failure.set(Some(point)));
            let failed = if point.ends_with("retirement") {
                backend
                    .maintain(RedbMaintenanceRequest::FinalizeThrough(ticket(4)))
                    .is_err()
            } else {
                backend
                    .commit(
                        Revision(8),
                        ticket(9).operation_id(),
                        &[Mutation::Put {
                            key: 7_u32.to_be_bytes().to_vec(),
                            value: vec![9; 16],
                        }],
                    )
                    .is_err()
            };
            assert!(failed);
        }
        assert!(cache
            .entries
            .borrow()
            .rows
            .iter()
            .all(|(_, state)| state.is_none()));
        let usage = cache
            .owner()
            .open(NAMESPACE, incarnation(), limits())
            .unwrap()
            .usage()
            .unwrap();
        assert_eq!(cache.stats().full_validations, 2);
        assert_eq!(
            usage.revision,
            Revision(if point == "after-commit" { 9 } else { 8 })
        );
        assert_eq!(
            usage.retired_through,
            if point == "after-retirement" { 4 } else { 0 }
        );
    }
}

#[test]
fn deletion_and_import_never_restore_stale_validation() {
    let (_dir, db) = database();
    seed(&db, NAMESPACE, 8);
    let mut cache = make_cache(&db, 2);
    let snapshot = {
        let mut owner = cache.owner();
        owner
            .open(NAMESPACE, incarnation(), limits())
            .unwrap()
            .export_enrollment()
            .unwrap()
    };
    cache
        .owner()
        .delete(NAMESPACE, incarnation(), limits())
        .unwrap();
    assert!(cache
        .entries
        .borrow()
        .rows
        .iter()
        .all(|(_, state)| state.is_none()));
    cache
        .owner()
        .import_enrollment(NAMESPACE, incarnation(), limits(), &snapshot)
        .unwrap();
    assert!(cache
        .entries
        .borrow()
        .rows
        .iter()
        .all(|(_, state)| state.is_none()));
    let prior = cache.stats().full_validations;
    cache
        .owner()
        .open(NAMESPACE, incarnation(), limits())
        .unwrap();
    assert_eq!(cache.stats().full_validations, prior + 1);
}

struct FailedParticipant(bool);
impl crate::TransactionParticipant for FailedParticipant {
    fn stage(
        &self,
        _: &redb::WriteTransaction,
        _: crate::CommitContext,
        _: &crate::CommitRead<'_>,
        _: OperationTicket,
        _: &[Mutation],
    ) -> Result<Option<prns_groups_store::OperationRejection>, RedbError> {
        if self.0 {
            panic!("participant cut");
        }
        Err(RedbError::Database(
            std::io::Error::other("participant cut").into(),
        ))
    }
    fn delete_in(
        &self,
        _: &redb::WriteTransaction,
        _: crate::CommitContext,
    ) -> Result<(), RedbError> {
        Ok(())
    }
}

#[test]
fn participant_error_or_panic_cannot_leave_cached_uncommitted_state() {
    for panic in [false, true] {
        let (_dir, db) = database();
        seed(&db, NAMESPACE, 8);
        let mut cache = make_cache(&db, 2);
        {
            let mut owner = cache.owner();
            let mut backend = owner
                .open(NAMESPACE, incarnation(), limits())
                .unwrap()
                .with_participant(Arc::new(FailedParticipant(panic)));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                backend.commit(Revision(8), ticket(9).operation_id(), &[])
            }));
            assert!(if panic {
                result.is_err()
            } else {
                result.unwrap().is_err()
            });
        }
        assert!(cache
            .entries
            .borrow()
            .rows
            .iter()
            .all(|(_, state)| state.is_none()));
        let recovered = cache
            .owner()
            .open(NAMESPACE, incarnation(), limits())
            .unwrap()
            .recover(ticket(9).operation_id())
            .unwrap();
        assert_eq!(
            recovered,
            RecoveryOutcome::NotCommitted {
                revision: Revision(8)
            }
        );
        assert_eq!(cache.stats().full_validations, 2);
    }
}
