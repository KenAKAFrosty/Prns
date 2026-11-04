use super::*;

fn generations(db: &Database, base: &Entry, last: u32) -> Entry {
    let mut latest = base.clone();
    for generation in 0..=last {
        let mut entry = base.clone();
        entry.generation = generation;
        entry.namespace[..4].copy_from_slice(&(generation + 1).to_be_bytes());
        entry.incarnation[..4].copy_from_slice(&(generation + 1).to_be_bytes());
        let r = read(db);
        let mut current = rows::Current::pending(&entry);
        current.ready = true;
        current.head = Some([generation as u8 + 1; 32]);
        let mut edit = Edit::new(&r).unwrap();
        if generation > 0 {
            edit = edit
                .put_incarnation(
                    &r,
                    keys::incarnation(base.group, generation - 1),
                    &rows::Incarnation::Archived(latest),
                )
                .unwrap();
        }
        edit = edit
            .put_current(&r, entry.group, &current)
            .unwrap()
            .put_incarnation(
                &r,
                keys::incarnation(entry.group, generation),
                &rows::Incarnation::Live(entry.clone()),
            )
            .unwrap();
        commit(db, edit);
        latest = entry;
    }
    latest
}

#[test]
fn delete_fences_all_generations_before_bounded_cleanup_and_preserves_rejoin() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(
        &db,
        CatalogLimits {
            max_maintenance_rows: 2,
            ..Default::default()
        },
    );
    let base = template();
    let latest = generations(&db, &base, 5);
    let r = read(&db);
    commit(
        &db,
        Edit::new(&r)
            .unwrap()
            .fence_delete(&r, base.group)
            .unwrap()
            .put_incarnation(
                &r,
                keys::incarnation(base.group, 5),
                &rows::Incarnation::Retired(rows::RetiredIncarnation::from_entry(&latest, false)),
            )
            .unwrap(),
    );
    let r = read(&db);
    r.inspect().unwrap();
    assert!(r.entry(base.group).unwrap().is_none());
    for generation in 0..=5 {
        assert!(r.generation_deleted(base.group, generation).unwrap());
    }

    let mut rejoined = latest.clone();
    rejoined.generation = 6;
    rejoined.namespace[..4].copy_from_slice(&7u32.to_be_bytes());
    rejoined.incarnation[..4].copy_from_slice(&7u32.to_be_bytes());
    let mut current = rows::Current::pending(&rejoined);
    current.purge_through = Some(5);
    current.invitations_deleted_through = r
        .current(base.group)
        .unwrap()
        .unwrap()
        .invitations_deleted_through;
    commit(
        &db,
        Edit::new(&r)
            .unwrap()
            .put_current(&r, base.group, &current)
            .unwrap()
            .put_incarnation(
                &r,
                keys::incarnation(base.group, 6),
                &rows::Incarnation::Live(rejoined.clone()),
            )
            .unwrap(),
    );
    for expected in 0..=5 {
        let r = read(&db);
        let plan = r.purge_plan().unwrap();
        let (key, mut row) = plan.target.unwrap();
        assert_eq!(
            keys::split_incarnation(&key).unwrap(),
            (base.group, expected)
        );
        row.purged = true;
        commit(
            &db,
            Edit::new(&r)
                .unwrap()
                .put_incarnation(&r, key, &rows::Incarnation::Retired(row))
                .unwrap()
                .purge_progress(plan.after),
        );
        let r = read(&db);
        r.inspect().unwrap();
        assert_eq!(
            r.entry(base.group).unwrap().unwrap().incarnation,
            rejoined.incarnation
        );
    }
    let r = read(&db);
    assert!(r.purge_plan().unwrap().target.is_none());
    assert!(!r.generation_deleted(base.group, 6).unwrap());
}

#[test]
fn purge_cursor_bounds_skipped_rows_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    let db = Database::create(&path).unwrap();
    create(
        &db,
        CatalogLimits {
            max_maintenance_rows: 2,
            ..Default::default()
        },
    );
    let base = template();
    for n in 0..7 {
        let (current, entry) = membership(&base, n);
        let r = read(&db);
        let edit = Edit::new(&r)
            .unwrap()
            .put_current(&r, entry.group, &current)
            .unwrap()
            .put_incarnation(
                &r,
                keys::incarnation(entry.group, entry.generation),
                &rows::Incarnation::Live(entry.clone()),
            )
            .unwrap();
        commit(&db, edit);
        if n == 6 {
            let r = read(&db);
            commit(
                &db,
                Edit::new(&r)
                    .unwrap()
                    .fence_delete(&r, entry.group)
                    .unwrap()
                    .put_incarnation(
                        &r,
                        keys::incarnation(entry.group, entry.generation),
                        &rows::Incarnation::Retired(rows::RetiredIncarnation::from_entry(
                            &entry, false,
                        )),
                    )
                    .unwrap(),
            );
        }
    }
    for _ in 0..3 {
        let r = read(&db);
        let plan = r.purge_plan().unwrap();
        assert!(plan.target.is_none());
        assert!(plan.after.is_some());
        commit(&db, Edit::new(&r).unwrap().purge_progress(plan.after));
    }
    drop(db);
    let db = Database::open(path).unwrap();
    let r = read(&db);
    r.inspect().unwrap();
    let (key, _) = r.purge_plan().unwrap().target.unwrap();
    assert_eq!(
        &keys::split_incarnation(&key).unwrap().0[..4],
        &6u32.to_be_bytes()
    );
}

#[test]
fn old_departure_remains_exact_after_delete_and_new_current_generation() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let mut base = template();
    base.owner.rns = [88; 16];
    let latest = generations(&db, &base, 0);
    let departure = Departure {
        id: [41; 32],
        group: base.group,
        owner: base.owner.clone(),
        member: base.local.clone(),
        known_head: [1; 32],
    };
    let r = read(&db);
    commit(
        &db,
        Edit::new(&r)
            .unwrap()
            .put_departure(&r, &departure)
            .unwrap(),
    );
    let r = read(&db);
    commit(
        &db,
        Edit::new(&r)
            .unwrap()
            .fence_delete(&r, base.group)
            .unwrap()
            .put_incarnation(
                &r,
                keys::incarnation(base.group, 0),
                &rows::Incarnation::Retired(rows::RetiredIncarnation::from_entry(&latest, true)),
            )
            .unwrap(),
    );
    let mut rejoined = latest;
    rejoined.generation = 1;
    rejoined.namespace[..4].copy_from_slice(&2u32.to_be_bytes());
    rejoined.incarnation[..4].copy_from_slice(&2u32.to_be_bytes());
    let mut current = rows::Current::pending(&rejoined);
    current.purge_through = Some(0);
    let r = read(&db);
    current.invitations_deleted_through = r
        .current(base.group)
        .unwrap()
        .unwrap()
        .invitations_deleted_through;
    commit(
        &db,
        Edit::new(&r)
            .unwrap()
            .put_current(&r, base.group, &current)
            .unwrap()
            .put_incarnation(
                &r,
                keys::incarnation(base.group, 1),
                &rows::Incarnation::Live(rejoined),
            )
            .unwrap(),
    );
    let r = read(&db);
    r.inspect().unwrap();
    assert!(r.departure(departure.id).unwrap().unwrap().member == departure.member);
    assert_eq!(
        r.work_source(work_id(b"leave", &departure.id)).unwrap(),
        Some(work::WorkSource::Departure(departure.id))
    );
    commit(
        &db,
        Edit::new(&r)
            .unwrap()
            .clear_departure_id(&r, departure.id)
            .unwrap(),
    );
    let r = read(&db);
    r.inspect().unwrap();
    assert_eq!(r.current(base.group).unwrap().unwrap().generation, 1);
    assert!(r.departure(departure.id).unwrap().is_none());
}

#[test]
fn delete_hides_unbound_offers_but_preserves_later_fresh_offers_across_rejoin() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let mut base = template();
    base.owner.rns = [88; 16];
    let latest = generations(&db, &base, 0);
    let invite = Invite {
        admission_revision: 0,
        offer: Offer {
            id: [31; 32],
            group: base.group,
            title: base.title.clone(),
            profile: base.profile,
            owner: base.owner.clone(),
            recipient: identity().into_bytes(),
            created: NOW,
            expires: NOW + 100,
        },
        state: InvitationState::Pending,
        acknowledged: true,
        terminal_at: None,
        deferred_join: None,
        dismissed: false,
    };
    let r = read(&db);
    commit(
        &db,
        Edit::new(&r).unwrap().put_invitation(&r, &invite).unwrap(),
    );
    let r = read(&db);
    assert!(r.invitation_binding(invite.offer.id).unwrap().is_none());
    commit(
        &db,
        Edit::new(&r)
            .unwrap()
            .fence_delete(&r, base.group)
            .unwrap()
            .put_incarnation(
                &r,
                keys::incarnation(base.group, 0),
                &rows::Incarnation::Retired(rows::RetiredIncarnation::from_entry(&latest, true)),
            )
            .unwrap(),
    );
    let engine = Enrollment::open_attached(&db, identity(), CatalogLimits::default()).unwrap();
    let limits = crate::PageLimits {
        max_rows: 8,
        max_bytes: 16 * 1024,
    };
    assert!(engine
        .invitations_page(NOW, None, limits)
        .unwrap()
        .items
        .is_empty());

    let mut fresh = invite.clone();
    fresh.offer.id = [32; 32];
    let r = read(&db);
    commit(
        &db,
        Edit::new(&r).unwrap().put_invitation(&r, &fresh).unwrap(),
    );
    let mut rejoined = latest;
    rejoined.generation = 1;
    rejoined.namespace[..4].copy_from_slice(&2u32.to_be_bytes());
    rejoined.incarnation[..4].copy_from_slice(&2u32.to_be_bytes());
    let r = read(&db);
    let mut current = rows::Current::pending(&rejoined);
    current.purge_through = Some(0);
    current.invitations_deleted_through = r
        .current(base.group)
        .unwrap()
        .unwrap()
        .invitations_deleted_through;
    commit(
        &db,
        Edit::new(&r)
            .unwrap()
            .put_current(&r, base.group, &current)
            .unwrap()
            .put_incarnation(
                &r,
                keys::incarnation(base.group, 1),
                &rows::Incarnation::Live(rejoined),
            )
            .unwrap(),
    );
    let page = engine.invitations_page(NOW, None, limits).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id.0, fresh.offer.id);
    assert_eq!(page.items[0].state, InvitationState::Pending);
    let r = read(&db);
    r.inspect().unwrap();
    assert!(r
        .invitation_deleted(&r.invitation(invite.offer.id).unwrap().unwrap())
        .unwrap());
    assert!(!r
        .invitation_deleted(&r.invitation(fresh.offer.id).unwrap().unwrap())
        .unwrap());
}

#[test]
fn admitted_current_funds_delete_receipt_and_fence_at_exact_durable_quota() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let base = template();
    let latest = generations(&db, &base, 0);
    for n in 1..=40 {
        let r = read(&db);
        commit(
            &db,
            Edit::new(&r)
                .unwrap()
                .put_receipt(
                    &r,
                    &rows::Receipt {
                        request: request(n),
                        namespace: None,
                    },
                )
                .unwrap(),
        );
    }
    // Select the already-measured exact logical ceiling in this storage-only
    // fixture. No extra ordinary byte remains, while funded cleanup is intact.
    let r = read(&db);
    let mut header = r.header.clone();
    header.limits.max_catalog_bytes = header.bytes + header.reserved;
    header.limits.validate().unwrap();
    let tx = db.begin_write().unwrap();
    tx.open_table(HEADER)
        .unwrap()
        .insert(0, encode(&header).unwrap().as_slice())
        .unwrap();
    tx.commit().unwrap();
    let r = read(&db);
    assert!(matches!(
        Edit::new(&r)
            .unwrap()
            .put_receipt(
                &r,
                &rows::Receipt {
                    request: request(41),
                    namespace: None,
                }
            )
            .and_then(Edit::finish),
        Err(EnrollmentError::Capacity)
    ));
    let before = r.header.counts.requests;
    let mut changed = r.current(base.group).unwrap().unwrap();
    changed.lifecycle = GroupLifecycle::Removed;
    changed.ready = false;
    changed.head = Some([u8::MAX; 32]);
    commit(
        &db,
        Edit::new(&r)
            .unwrap()
            .put_current(&r, base.group, &changed)
            .unwrap(),
    );
    let r = read(&db);
    let edit = Edit::new(&r)
        .unwrap()
        .fence_delete(&r, base.group)
        .unwrap()
        .put_incarnation(
            &r,
            keys::incarnation(base.group, 0),
            &rows::Incarnation::Retired(rows::RetiredIncarnation::from_entry(&latest, false)),
        )
        .unwrap()
        .put_receipt(
            &r,
            &rows::Receipt {
                request: request(42),
                namespace: None,
            },
        )
        .unwrap();
    commit(&db, edit);
    let r = read(&db);
    r.inspect().unwrap();
    assert_eq!(r.header.counts.requests, before + 1);
    assert!(r.header.bytes + r.header.reserved <= r.header.limits.max_catalog_bytes);
    assert!(r.entry(base.group).unwrap().is_none());
}

#[test]
fn cold_inspection_rejects_nonexistent_purge_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let r = read(&db);
    let cursor = purge::Cursor::from_key(keys::incarnation([99; 32], 9)).unwrap();
    commit(&db, Edit::new(&r).unwrap().purge_progress(Some(cursor)));
    assert!(matches!(read(&db).inspect(), Err(EnrollmentError::Corrupt)));
}

#[test]
fn exact_invitation_read_rejects_zero_and_future_admission_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let base = template();
    let invite = Invite {
        admission_revision: 0,
        offer: Offer {
            id: [31; 32],
            group: base.group,
            title: base.title.clone(),
            profile: base.profile,
            owner: Member {
                rns: [88; 16],
                ..base.owner.clone()
            },
            recipient: identity().into_bytes(),
            created: NOW,
            expires: NOW + 100,
        },
        state: InvitationState::Pending,
        acknowledged: true,
        terminal_at: None,
        deferred_join: None,
        dismissed: false,
    };
    let r = read(&db);
    commit(
        &db,
        Edit::new(&r).unwrap().put_invitation(&r, &invite).unwrap(),
    );
    let r = read(&db);
    let mut stored = r.invitation(invite.offer.id).unwrap().unwrap();
    for revision in [0, r.header.revision + 1] {
        stored.admission_revision = revision;
        let tx = db.begin_write().unwrap();
        tx.open_table(INVITATIONS)
            .unwrap()
            .insert(
                invite.offer.id.as_slice(),
                encode(&stored).unwrap().as_slice(),
            )
            .unwrap();
        tx.commit().unwrap();
        assert!(matches!(
            read(&db).invitation(invite.offer.id),
            Err(EnrollmentError::Corrupt)
        ));
    }
}
