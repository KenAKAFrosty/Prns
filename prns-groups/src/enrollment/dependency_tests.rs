use super::management_tests::{enroll, remove};
use super::*;
use crate::{Dependencies, GroupManager};
use prns_groups_store_redb::change;
use redb::ReadableTable;

fn commit_edit(db: &Database, identity: IdentityHash, edit: indexed::Edit) {
    let write = edit.finish().unwrap();
    let tx = db.begin_write().unwrap();
    write.stage(&tx, identity).unwrap();
    change::commit(tx, None).unwrap();
}

fn stamp(db: &Database, id: u8) {
    let tx = db.begin_write().unwrap();
    change::initialize_in(&tx, StoreIncarnation::new([id; 24]).unwrap()).unwrap();
    tx.commit().unwrap();
}
fn report(db: &Database, identity: IdentityHash) -> Dependencies {
    let mut manager = GroupManager::open_managed(db, identity).unwrap();
    let mut scan = manager.begin_dependency_scan().unwrap();
    loop {
        if let Some(result) = manager.advance_dependency_scan(&mut scan, 1).unwrap() {
            return result;
        }
    }
}

#[test]
fn dependency_scan_requires_selected_provenance_and_reads_do_not_change_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("group");
    let db = Database::create(&path).unwrap();
    let device = Device::new(&db, 1);
    let manager = GroupManager::open_managed(&db, device.identity).unwrap();
    assert!(matches!(
        manager.begin_dependency_scan(),
        Err(crate::Error::Configuration)
    ));
    drop(manager);
    stamp(&db, 1);
    let expected = report(&db, device.identity);
    assert_eq!(expected.active_memberships, 0);
    assert_eq!(expected.unresolved_operations, 0);
    assert_eq!(expected, report(&db, device.identity));
    drop(device);
    drop(db);
    let db = Database::open(path).unwrap();
    assert_eq!(expected, report(&db, IdentityHash::new([1; 16])));
}

#[test]
fn dependency_scan_tracks_retained_acceptance_separately_from_terminal_results() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("group")).unwrap();
    let device = Device::new(&db, 1);
    stamp(&db, 1);
    let tx = db.begin_write().unwrap();
    crate::retained::initialize_in(
        &tx,
        device.identity,
        crate::retained::Limits {
            max_operations: 8,
            max_bytes: 1024 * 1024,
            ..Default::default()
        },
    )
    .unwrap();
    change::commit(tx, None).unwrap();
    let mut manager = GroupManager::open_managed(&db, device.identity).unwrap();
    let mut scan = manager.begin_dependency_scan().unwrap();
    let id = crate::RequestId(request(1));
    let intent = crate::Intent::Create {
        profile: Default::default(),
        title: "retained".into(),
    };
    let context = crate::ExecutionContext {
        clock: prns_groups_core::ClockReading::Trusted(
            prns_groups_core::ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
        ),
    };
    crate::retained::fault::set(crate::retained::fault::Cut::Accepted);
    assert!(matches!(
        manager.submit_retained(
            id,
            intent.clone(),
            context,
            &mut crate::admission::Unmanaged
        ),
        Err(crate::Error::Unavailable)
    ));
    assert!(matches!(
        manager.advance_dependency_scan(&mut scan, 1),
        Err(crate::Error::Busy)
    ));
    drop(manager);
    let pending = report(&db, device.identity);
    assert_eq!(pending.unresolved_operations, 1);
    assert_eq!(pending.active_memberships, 0);
    assert_eq!(pending.terminal_operations, 0);
    let mut manager = GroupManager::open_managed(&db, device.identity).unwrap();
    manager
        .submit_retained(id, intent, context, &mut crate::admission::Unmanaged)
        .unwrap();
    drop(manager);
    let completed = report(&db, device.identity);
    assert_eq!(completed.unresolved_operations, 0);
    assert_eq!(completed.terminal_operations, 1);
    assert_eq!(completed.active_memberships, 1);
    assert_eq!(completed.owner_groups, 1);
    assert_eq!(completed.pending_publications, 0);
    assert_eq!(completed.incomplete_setup, 0);
    assert_eq!(completed, report(&db, device.identity));
}

#[test]
fn dependency_scan_pages_archives_and_rejects_mixed_revisions_or_late_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let adb = Database::create(dir.path().join("a")).unwrap();
    let bdb = Database::create(dir.path().join("b")).unwrap();
    let mut a = Device::new(&adb, 1);
    let mut b = Device::new(&bdb, 2);
    stamp(&adb, 1);
    stamp(&bdb, 2);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "churn", NOW)
        .unwrap();
    for i in 0..6u8 {
        enroll(&mut a, &mut b, group, 2 + i * 3);
        remove(&mut a, &mut b, group, 4 + i * 3);
    }
    let mut manager = GroupManager::open_managed(&bdb, b.identity).unwrap();
    let mut scan = manager.begin_dependency_scan().unwrap();
    assert!(matches!(
        manager.advance_dependency_scan(&mut scan, 0),
        Err(crate::Error::InvalidInput)
    ));
    assert!(matches!(
        manager.advance_dependency_scan(&mut scan, 5),
        Err(crate::Error::InvalidInput)
    ));
    assert!(manager
        .advance_dependency_scan(&mut scan, 4)
        .unwrap()
        .is_none());
    let done = loop {
        if let Some(done) = manager.advance_dependency_scan(&mut scan, 4).unwrap() {
            break done;
        }
    };
    assert_eq!(done.archived_incarnations, 5);
    assert_eq!(done.active_memberships, 0);
    assert_eq!(done.pending_publications, 0);
    assert_eq!(done.incomplete_setup, 0);
    let mut stale = manager.begin_dependency_scan().unwrap();
    assert!(manager
        .advance_dependency_scan(&mut stale, 4)
        .unwrap()
        .is_none());
    drop(manager);
    // A revision change invalidates complete dependency accounting even if it
    // does not alter this page's exact membership keys.
    change::commit(bdb.begin_write().unwrap(), None).unwrap();
    let mut manager = GroupManager::open_managed(&bdb, b.identity).unwrap();
    assert!(matches!(
        manager.advance_dependency_scan(&mut stale, 4),
        Err(crate::Error::Busy)
    ));
    drop(manager);
    let mut manager = GroupManager::open_managed(&bdb, b.identity).unwrap();
    let mut scan = manager.begin_dependency_scan().unwrap();
    // Inject physical corruption after strict attachment, without advancing
    // the revision. The bounded scan must reject it when that row is visited.
    let read = b.engine.read().unwrap();
    let (key, value) = read.incarnations.iter().unwrap().last().unwrap().unwrap();
    let key = key.value().to_vec();
    let mut broken: indexed::rows::Incarnation = serde_json::from_slice(value.value()).unwrap();
    match &mut broken {
        indexed::rows::Incarnation::Live(entry) | indexed::rows::Incarnation::Archived(entry) => {
            entry.secret.clear()
        }
        indexed::rows::Incarnation::Retired(_) => panic!("expected a retained namespace"),
    }
    let tx = bdb.begin_write().unwrap();
    tx.open_table(indexed::INCARNATIONS)
        .unwrap()
        .insert(key.as_slice(), encode(&broken).unwrap().as_slice())
        .unwrap();
    tx.commit().unwrap();
    // The corrupt sixth namespace is not touched by a four-namespace step.
    assert!(manager
        .advance_dependency_scan(&mut scan, 4)
        .unwrap()
        .is_none());
    let before = (scan.next, scan.result);
    assert!(manager.advance_dependency_scan(&mut scan, 4).is_err());
    assert_eq!(
        (scan.next, scan.result),
        before,
        "failed pages keep no partial progress"
    );
}

#[test]
fn dependency_scan_keeps_departures_and_terminal_notices_after_local_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let adb = Database::create(dir.path().join("a")).unwrap();
    let bdb = Database::create(dir.path().join("b")).unwrap();
    let mut a = Device::new(&adb, 1);
    let mut b = Device::new(&bdb, 2);
    stamp(&adb, 1);
    stamp(&bdb, 2);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "delete", NOW)
        .unwrap();
    enroll(&mut a, &mut b, group, 2);
    b.engine
        .leave(&mut b.owner, request(40), group, NOW)
        .unwrap();
    b.engine
        .delete_local(&mut b.owner, request(41), group, NOW)
        .unwrap();
    let departed = report(&bdb, b.identity);
    assert_eq!(departed.active_memberships, 0);
    assert_eq!(departed.pending_departures, 1);
    assert_eq!(departed.retired_incarnations, 1);
    assert_eq!(departed.pending_local_purges, 0);
    assert!(departed.passive_fences > 0);
    let head = a.descriptor(group).control.head.0;
    a.engine
        .close(&mut a.owner, request(42), group, head, NOW)
        .unwrap();
    let closed = report(&adb, a.identity);
    assert_eq!(closed.active_memberships, 0);
    assert_eq!(closed.owner_groups, 0);
    assert_eq!(closed.pending_terminal_notices, 1);
    a.engine
        .delete_local(&mut a.owner, request(43), group, NOW)
        .unwrap();
    let deleted = report(&adb, a.identity);
    assert_eq!(deleted.active_memberships, 0);
    assert_eq!(deleted.pending_terminal_notices, 1);
    assert_eq!(deleted.pending_local_purges, 0);
}

#[test]
fn dependency_scan_reports_unfinished_setup_and_never_initializes_missing_namespaces() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("group")).unwrap();
    let mut device = Device::new(&db, 1);
    stamp(&db, 1);
    let entry = device
        .engine
        .entry([9; 32], "unfinished", Default::default(), None, NOW)
        .unwrap();
    let read = device.engine.read().unwrap();
    let current = indexed::rows::Current {
        generation: entry.generation,
        title: entry.title.clone(),
        owner: entry.owner.rns,
        lifecycle: GroupLifecycle::Active,
        ready: false,
        member_count: 0,
        pending_offers: 0,
        head: None,
        departure: None,
        visible: true,
        purge_through: None,
        invitations_deleted_through: 0,
    };
    let edit = indexed::Edit::new(&read)
        .unwrap()
        .put_current(&read, entry.group, &current)
        .unwrap()
        .put_incarnation(
            &read,
            indexed::keys::incarnation(entry.group, entry.generation),
            &indexed::rows::Incarnation::Live(entry.clone()),
        )
        .unwrap();
    commit_edit(&db, device.identity, edit);
    let before = change::read(&db).unwrap();
    let mut manager = GroupManager::open_managed(&db, device.identity).unwrap();
    let mut scan = manager.begin_dependency_scan().unwrap();
    assert!(manager.advance_dependency_scan(&mut scan, 1).is_err());
    assert_eq!(change::read(&db).unwrap(), before);
    drop(manager);
    device.engine.initialize(&mut device.owner, &entry).unwrap();
    let incomplete = report(&db, device.identity);
    assert_eq!(incomplete.active_memberships, 0);
    assert_eq!(incomplete.incomplete_setup, 1);
    assert_eq!(incomplete, report(&db, device.identity));
}
