#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use std::{num::NonZeroUsize, panic::AssertUnwindSafe};

const NAMESPACE: StoreNamespace = StoreNamespace([7; 16]);
fn incarnation() -> StoreIncarnation {
    StoreIncarnation::new([3; 24]).unwrap()
}
fn limits() -> RedbLimits {
    RedbLimits {
        max_changes: 32,
        max_key_bytes: 16,
        max_value_bytes: 64,
        max_staged_bytes: 4096,
        max_live_objects: 64,
        max_live_bytes: 8192,
        max_receipts: 64,
    }
}
fn fixture() -> (tempfile::TempDir, Arc<Database>, ValidationCache) {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(Database::create(directory.path().join("inspection.redb")).unwrap());
    let mut cache = ValidationCache::new(database.clone(), NonZeroUsize::MIN).unwrap();
    {
        let mut owner = cache.owner();
        let mut backend = owner
            .initialize(NAMESPACE, incarnation(), limits())
            .unwrap();
        let mutations: Vec<_> = (0..16)
            .map(|key| Mutation::Put {
                key: vec![key],
                value: vec![9; 64],
            })
            .collect();
        assert!(matches!(
            backend
                .commit(
                    Revision(0),
                    OperationTicket::new(incarnation(), 1)
                        .unwrap()
                        .operation_id(),
                    &mutations
                )
                .unwrap(),
            CommitOutcome::Committed(_)
        ));
    }
    (directory, database, cache)
}

#[test]
fn borrowed_inspection_reuses_one_strict_pass_even_with_a_warm_cache() {
    let (_directory, _database, mut cache) = fixture();
    let before = cache.stats();
    let mut visited = 0;
    {
        let mut owner = cache.owner();
        let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
        let usage = backend
            .inspect_with(|key, value| {
                assert_eq!(key, &[visited]);
                assert_eq!(value.len(), 64);
                visited += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(usage.live_objects, 16);
    }
    assert_eq!(visited, 16);
    assert_eq!(cache.stats().full_validations - before.full_validations, 1);
    assert_eq!(cache.stats().values_inspected - before.values_inspected, 16);
    assert_eq!(
        cache.stats().receipts_inspected - before.receipts_inspected,
        1
    );
    let validated = cache.stats();
    cache
        .owner()
        .open(NAMESPACE, incarnation(), limits())
        .unwrap();
    assert_eq!(cache.stats().full_validations, validated.full_validations);
}

#[test]
fn rejected_or_panicked_inspection_never_retains_cached_proof() {
    for panic in [false, true] {
        let (_directory, _database, mut cache) = fixture();
        let before = cache.stats();
        {
            let mut owner = cache.owner();
            let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                backend.inspect_with(|key, _| {
                    if key == [7] {
                        if panic {
                            panic!("inspection callback interrupted");
                        }
                        return Err(RedbError::Corrupt);
                    }
                    Ok(())
                })
            }));
            if panic {
                assert!(result.is_err());
            } else {
                assert!(matches!(result.unwrap(), Err(RedbError::Corrupt)));
            }
        }
        assert_eq!(cache.stats().values_inspected - before.values_inspected, 8);
        cache
            .owner()
            .open(NAMESPACE, incarnation(), limits())
            .unwrap();
        assert_eq!(cache.stats().full_validations - before.full_validations, 2);
        assert_eq!(cache.stats().values_inspected - before.values_inspected, 24);
    }
}

#[test]
fn native_corruption_is_rejected_before_the_callback_sees_the_row() {
    let (_directory, database, mut cache) = fixture();
    let names = TableNames::new(NAMESPACE);
    let write = database.begin_write().unwrap();
    write
        .open_table(names.values())
        .unwrap()
        .insert([0].as_slice(), [9; 65].as_slice())
        .unwrap();
    write.commit().unwrap();
    {
        let mut owner = cache.owner();
        let mut backend = owner.open(NAMESPACE, incarnation(), limits()).unwrap();
        let mut visited = 0;
        assert!(matches!(
            backend.inspect_with(|_, _| {
                visited += 1;
                Ok(())
            }),
            Err(RedbError::Corrupt)
        ));
        assert_eq!(visited, 0);
    }
    assert!(matches!(
        cache.owner().open(NAMESPACE, incarnation(), limits()),
        Err(RedbError::Corrupt)
    ));
}
