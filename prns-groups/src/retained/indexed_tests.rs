#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::ExecutionContext;
use prns_groups_core::Timestamp;
use prns_groups_core::{ClockInterval, ClockReading};
const NOW: u64 = 1_800_000_000;
fn identity() -> IdentityHash {
    IdentityHash::new([42; 16])
}
fn context(now: u64) -> ExecutionContext {
    ExecutionContext {
        clock: ClockReading::Trusted(ClockInterval::new(Timestamp(now), Timestamp(now)).unwrap()),
    }
}
fn operation(n: u32, now: u64) -> Operation {
    let mut entropy = [1; 20];
    entropy[..4].copy_from_slice(&n.to_be_bytes());
    let mut group = [0; 32];
    group[..4].copy_from_slice(&n.to_be_bytes());
    Operation {
        request: Request {
            id: crate::RequestToken::new(now, entropy).unwrap().into(),
            intent: Intent::Respond {
                invitation: crate::InvitationId(group),
                accept: true,
            },
            original_scope: None,
        },
        state: State::Pending,
        scope: OperationScope::Group(GroupId(group)),
    }
}
fn create(db: &Database, limits: Limits) {
    let tx = db.begin_write().unwrap();
    initialize_in(&tx, identity(), limits).unwrap();
    tx.commit().unwrap();
}
fn page(rows: u32) -> crate::PageLimits {
    crate::PageLimits {
        max_rows: rows,
        max_bytes: 2 * 1024 * 1024,
    }
}
fn finish(store: &Store<'_>, row: &mut Operation) {
    row.state = State::Completed(Outcome::Saved);
    store.save(row).unwrap();
}
fn header(db: &Database) -> Header {
    Snapshot::open(&db.begin_read().unwrap(), identity())
        .unwrap()
        .unwrap()
        .header
}
fn rewrite_header(db: &Database, f: impl FnOnce(&mut Header)) {
    let mut h = header(db);
    f(&mut h);
    let bytes = encode(&h).unwrap();
    let tx = db.begin_write().unwrap();
    tx.open_table(HEADER)
        .unwrap()
        .insert(0, bytes.as_slice())
        .unwrap();
    tx.commit().unwrap();
}
#[test]
fn large_count_streams_cold_once_and_exact_pages_do_not_rescan() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retained");
    let db = Database::create(&path).unwrap();
    let limits = Limits {
        max_operations: 4096,
        max_bytes: 64 * 1024 * 1024,
        ..Default::default()
    };
    create(&db, limits);
    let store = Store::open(&db, identity()).unwrap().unwrap();
    let start = std::time::Instant::now();
    for n in 0..2048 {
        let mut row = operation(n, NOW);
        store.accept(&row, context(NOW)).unwrap();
        if n % 2 == 0 {
            finish(&store, &mut row);
        }
    }
    let populate = start.elapsed();
    drop(store);
    drop(db);
    let db = Database::open(&path).unwrap();
    work::take();
    let start = std::time::Instant::now();
    let limits = inspect_in(&db.begin_read().unwrap(), identity()).unwrap();
    let cold = start.elapsed();
    assert_eq!(work::take(), (2048, 0));
    let mut after = None;
    let mut seen = 0;
    for _ in 0..32 {
        let store = Store::open_attached(&db, identity(), limits)
            .unwrap()
            .unwrap();
        let rows = store.list(after, page(64)).unwrap();
        assert_eq!(rows.len(), 64);
        assert!(rows
            .windows(2)
            .all(|p| p[0].request.id.0 < p[1].request.id.0));
        after = rows.last().map(|row| row.request.id);
        seen += rows.len();
    }
    assert_eq!(seen, 2048);
    assert_eq!(work::take(), (2048, 0));
    let store = Store::open_attached(&db, identity(), limits)
        .unwrap()
        .unwrap();
    assert!(store.list(after, page(64)).unwrap().is_empty());
    assert_eq!(
        store
            .operation(operation(1234, NOW).request.id)
            .unwrap()
            .unwrap()
            .request
            .id,
        operation(1234, NOW).request.id
    );
    assert_eq!(work::take(), (1, 0));
    let mut dependencies = crate::Dependencies::empty(prns_groups_store_redb::change::Stamp {
        incarnation: prns_groups_store::StoreIncarnation::new([1; 24]).unwrap(),
        revision: 0,
    });
    store.dependencies(&mut dependencies).unwrap();
    assert_eq!(dependencies.unresolved_operations, 1024);
    assert_eq!(dependencies.terminal_operations, 1024);
    assert_eq!(work::take(), (0, 0));
    eprintln!("retained2048 populate={populate:?} cold={cold:?}; cold rows=2048,32 lexical pages=2048,exact=1,dependencies=0");
}
#[test]
fn indexes_counters_markers_and_noncanonical_rows_fail_closed() {
    for case in 0..7 {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("corrupt")).unwrap();
        create(&db, Limits::default());
        let store = Store::open(&db, identity()).unwrap().unwrap();
        let row = operation(1, NOW);
        store.accept(&row, context(NOW)).unwrap();
        match case {
            0 => rewrite_header(&db, |h| h.reserved += 1),
            1 => rewrite_header(&db, |h| h.active = 0),
            2 => {
                let tx = db.begin_write().unwrap();
                tx.open_table(ACTIVE)
                    .unwrap()
                    .insert(
                        scope_key(row.scope).as_slice(),
                        operation(2, NOW).request.id.0.as_slice(),
                    )
                    .unwrap();
                tx.commit().unwrap();
            }
            3 => {
                let tx = db.begin_write().unwrap();
                tx.open_table(TERMINAL)
                    .unwrap()
                    .insert(row.request.id.0.as_slice(), 1)
                    .unwrap();
                tx.commit().unwrap();
            }
            4 => {
                let tx = db.begin_write().unwrap();
                tx.open_table(OPERATIONS)
                    .unwrap()
                    .remove(b"".as_slice())
                    .unwrap();
                tx.commit().unwrap();
            }
            5 => {
                let mut bytes = encode_operation(&row, Limits::default()).unwrap();
                bytes.push(b' ');
                let tx = db.begin_write().unwrap();
                tx.open_table(OPERATIONS)
                    .unwrap()
                    .insert(row.request.id.0.as_slice(), bytes.as_slice())
                    .unwrap();
                tx.commit().unwrap();
            }
            _ => rewrite_header(&db, |h| h.version = 2),
        }
        assert!(Store::open(&db, identity()).is_err(), "case{case}");
    }
}
#[test]
fn missing_empty_index_cannot_be_recreated_by_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("missing")).unwrap();
    create(&db, Limits::default());
    let store = Store::open(&db, identity()).unwrap().unwrap();
    let tx = db.begin_write().unwrap();
    tx.delete_table(TERMINAL).unwrap();
    tx.commit().unwrap();
    assert!(matches!(
        store.accept(&operation(1, NOW), context(NOW)),
        Err(Error::Corrupt)
    ));
    assert!(matches!(
        db.begin_read().unwrap().open_table(TERMINAL),
        Err(redb::TableError::TableDoesNotExist(_))
    ));
}
#[test]
fn before_and_after_commit_preserve_exact_atomic_evidence() {
    for cut in [
        fault::Cut::BeforeStorageCommit,
        fault::Cut::AfterStorageCommit,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("atomic")).unwrap();
        create(&db, Limits::default());
        let store = Store::open(&db, identity()).unwrap().unwrap();
        let mut row = operation(1, NOW);
        fault::set(cut);
        assert!(store.accept(&row, context(NOW)).is_err());
        let accepted = store.operation(row.request.id).unwrap().is_some();
        assert_eq!(accepted, cut == fault::Cut::AfterStorageCommit);
        inspect_in(&db.begin_read().unwrap(), identity()).unwrap();
        if !accepted {
            store.accept(&row, context(NOW)).unwrap();
        }
        row.state = State::Completed(Outcome::Saved);
        fault::set(cut);
        assert!(store.save(&row).is_err());
        assert_eq!(
            store
                .operation(row.request.id)
                .unwrap()
                .unwrap()
                .state
                .is_terminal(),
            cut == fault::Cut::AfterStorageCommit
        );
        inspect_in(&db.begin_read().unwrap(), identity()).unwrap();
        if cut == fault::Cut::BeforeStorageCommit {
            store.save(&row).unwrap();
        }
        fault::set(cut);
        assert!(store.dismiss(row.request.id).is_err());
        assert_eq!(
            store.operation(row.request.id).unwrap().is_none(),
            cut == fault::Cut::AfterStorageCommit
        );
        inspect_in(&db.begin_read().unwrap(), identity()).unwrap();
    }
}
#[test]
fn active_scope_conflict_and_terminal_immutability_are_transactional() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("scopes")).unwrap();
    create(&db, Limits::default());
    let store = Store::open(&db, identity()).unwrap().unwrap();
    let mut a = operation(1, NOW);
    let mut b = operation(2, NOW);
    b.scope = a.scope;
    store.accept(&a, context(NOW)).unwrap();
    assert!(matches!(store.accept(&b, context(NOW)), Err(Error::Busy)));
    let mut altered = a.clone();
    altered.request.intent = Intent::Respond {
        invitation: crate::InvitationId([9; 32]),
        accept: false,
    };
    assert!(matches!(
        store.save(&altered),
        Err(Error::IdempotencyConflict)
    ));
    finish(&store, &mut a);
    store.accept(&b, context(NOW)).unwrap();
    a.state = State::Pending;
    assert!(matches!(store.save(&a), Err(Error::Corrupt)));
    assert_eq!(header(&db).active, 1);
}
fn publication(n: u32, body: Vec<u8>) -> Operation {
    let mut row = operation(n, NOW);
    let group = GroupId([4; 32]);
    row.scope = OperationScope::Group(group);
    row.request.intent = Intent::Publish(crate::PublicationIntent {
        group,
        namespace: prns_groups_core::NamespaceId(1),
        schema: prns_groups_core::SchemaId(1),
        targets: crate::Targets::OtherMembers,
        body,
    });
    row.request.original_scope = Some(crate::OriginalScope::Control {
        control: prns_groups_core::ControlReference {
            head: prns_groups_core::ControlId([6; 32]),
            era: prns_groups_core::AuthorizationEra(prns_groups_core::ControlId([5; 32])),
        },
        incarnation: prns_groups_store::StoreIncarnation::new([1; 24]).unwrap(),
    });
    row
}
#[test]
fn page_accounts_real_capacity_and_admission_proves_decode_fit() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("pages")).unwrap();
    let row = publication(1, vec![7; 513]);
    let encoded = encode_operation(&row, Limits::default()).unwrap();
    let decoded: Operation = decode(&encoded).unwrap();
    let required = owned_bytes(&decoded).unwrap();
    assert!(required >= owned_bytes(&row).unwrap());
    let limits = Limits {
        max_operation_bytes: required as u64,
        ..Default::default()
    };
    create(&db, limits);
    let store = Store::open(&db, identity()).unwrap().unwrap();
    store.accept(&row, context(NOW)).unwrap();
    let page = store
        .list(
            None,
            crate::PageLimits {
                max_rows: 10,
                max_bytes: required as u64,
            },
        )
        .unwrap();
    assert_eq!(page.len(), 1);
    let actual = page.capacity() * std::mem::size_of::<Operation>()
        + page
            .iter()
            .map(|r| crate::client::intent_heap_bytes(&r.request.intent).unwrap())
            .sum::<usize>();
    assert!(actual <= required);
    assert!(matches!(
        store.list(
            None,
            crate::PageLimits {
                max_rows: 1,
                max_bytes: required as u64 - 1
            }
        ),
        Err(Error::InvalidInput)
    ));
    let dir2 = tempfile::tempdir().unwrap();
    let db2 = Database::create(dir2.path().join("small")).unwrap();
    create(
        &db2,
        Limits {
            max_operation_bytes: required as u64 - 1,
            ..Default::default()
        },
    );
    let store2 = Store::open(&db2, identity()).unwrap().unwrap();
    assert!(matches!(
        store2.accept(&row, context(NOW)),
        Err(Error::StorageFull)
    ));
    assert_eq!(header(&db2).count, 0);
}
#[test]
fn bounded_pruning_aborts_all_floor_and_counter_changes_when_insufficient() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("prune")).unwrap();
    let small = operation(1, NOW);
    let reservation = accounted(&small, Limits::default()).unwrap();
    let limits = Limits {
        max_operations: 10,
        max_bytes: reservation * 4,
        max_prune_rows: 1,
        ..Default::default()
    };
    create(&db, limits);
    let store = Store::open(&db, identity()).unwrap().unwrap();
    for n in 1..=4 {
        let mut row = operation(n, NOW);
        store.accept(&row, context(NOW)).unwrap();
        finish(&store, &mut row);
    }
    let large = publication(5, vec![255; 300]);
    work::take();
    assert!(matches!(
        store.accept(&large, context(NOW + 1)),
        Err(Error::StorageFull)
    ));
    assert_eq!(work::take().1, 1);
    let h = header(&db);
    assert_eq!(h.count, 4);
    assert_eq!(h.reserved, reservation * 4);
    assert_eq!(h.retired_before, 0);
    inspect_in(&db.begin_read().unwrap(), identity()).unwrap();
    let mut next = operation(5, NOW + 1);
    store.accept(&next, context(NOW + 1)).unwrap();
    assert_eq!(header(&db).retired_before, NOW);
    next.state = State::Uncertain(prns_groups_core::ApiError::Unavailable);
    store.save(&next).unwrap();
    assert!(matches!(store.dismiss(next.request.id), Err(Error::Busy)));
}

#[test]
fn selected_stamp_failure_rolls_back_row_indexes_and_counts() {
    for foreign in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("selected")).unwrap();
        create(&db, Limits::default());
        let expected = prns_groups_store::StoreIncarnation::new([1; 24]).unwrap();
        if foreign {
            let tx = db.begin_write().unwrap();
            prns_groups_store_redb::change::initialize_in(
                &tx,
                prns_groups_store::StoreIncarnation::new([2; 24]).unwrap(),
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let store = Store::open(&db, identity())
            .unwrap()
            .unwrap()
            .with_change_stamp(expected);
        assert!(store.accept(&operation(1, NOW), context(NOW)).is_err());
        assert_eq!(header(&db).count, 0);
        assert_eq!(header(&db).reserved, 0);
        assert!(store
            .operation(operation(1, NOW).request.id)
            .unwrap()
            .is_none());
        inspect_in(&db.begin_read().unwrap(), identity()).unwrap();
    }
}
#[test]
fn attachment_reuses_only_verified_selection_across_lexical_managers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("attached");
    let db = std::sync::Arc::new(Database::create(&path).unwrap());
    let tx = db.begin_write().unwrap();
    crate::enrollment::Enrollment::initialize_in(&tx, identity()).unwrap();
    initialize_in(
        &tx,
        identity(),
        Limits {
            max_operations: 1024,
            ..Default::default()
        },
    )
    .unwrap();
    prns_groups_store_redb::change::initialize_in(
        &tx,
        prns_groups_store::StoreIncarnation::new([3; 24]).unwrap(),
    )
    .unwrap();
    tx.commit().unwrap();
    let store = Store::open(&db, identity()).unwrap().unwrap();
    for n in 0..1024 {
        store.accept(&operation(n, NOW), context(NOW)).unwrap();
    }
    drop(store);
    work::take();
    let mut attachment =
        crate::Attachment::open_cached(db.clone(), identity(), std::num::NonZeroUsize::MIN)
            .unwrap();
    assert_eq!(work::take(), (1024, 0));
    for n in 0..100 {
        let manager = crate::GroupManager::open_attached(&db, &mut attachment).unwrap();
        assert!(manager
            .retained_operation(operation(n, NOW).request.id)
            .unwrap()
            .is_some());
    }
    assert_eq!(work::take(), (100, 0));
    let wrong = Limits {
        max_operations: 99,
        ..Default::default()
    };
    assert!(Store::open_attached(&db, identity(), Some(wrong)).is_err());
    drop(attachment);
    let db = std::sync::Arc::try_unwrap(db).ok().unwrap();
    drop(db);
    let db = Database::open(path).unwrap();
    assert_eq!(
        Store::open(&db, identity())
            .unwrap()
            .unwrap()
            .snapshot()
            .unwrap()
            .header
            .count,
        1024
    );
}

#[test]
fn result_headroom_fits_both_row_ceiling_and_total_reservation() {
    let row = publication(1, vec![255; 513]);
    let reserved = accounted(&row, Limits::default()).unwrap();
    for insufficient in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("headroom")).unwrap();
        let limits = Limits {
            max_row_bytes: u32::try_from(reserved - u64::from(insufficient)).unwrap(),
            max_bytes: reserved,
            ..Default::default()
        };
        create(&db, limits);
        let store = Store::open(&db, identity()).unwrap().unwrap();
        if insufficient {
            assert!(matches!(
                store.accept(&row, context(NOW)),
                Err(Error::StorageFull)
            ));
            assert_eq!(header(&db).count, 0);
            continue;
        }
        store.accept(&row, context(NOW)).unwrap();
        let mut changed = row.clone();
        changed.state = State::Uncertain(prns_groups_core::ApiError::Retry {
            reason: prns_groups_core::retry::RetryError::InvalidToken,
        });
        store.save(&changed).unwrap();
        changed.state = State::Completed(Outcome::Published(prns_groups_core::RecordId([255; 32])));
        store.save(&changed).unwrap();
        assert_eq!(header(&db).reserved, reserved);
        assert_eq!(header(&db).active, 0);
        assert_eq!(
            Store::open(&db, identity())
                .unwrap()
                .unwrap()
                .operation(row.request.id)
                .unwrap(),
            Some(changed)
        );
    }
}
