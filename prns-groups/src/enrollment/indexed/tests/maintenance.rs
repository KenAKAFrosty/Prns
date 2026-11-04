use super::*;

pub(super) fn incoming(entry: &Entry, peer: u8, n: u32) -> Invite {
    let mut id = [0; 32];
    id[..4].copy_from_slice(&n.to_be_bytes());
    Invite {
        admission_revision: 0,
        offer: Offer {
            id,
            group: entry.group,
            title: entry.title.clone(),
            profile: entry.profile,
            owner: Member {
                rns: [peer; 16],
                ..entry.owner.clone()
            },
            recipient: identity().into_bytes(),
            created: NOW,
            expires: NOW + 60,
        },
        state: InvitationState::Pending,
        acknowledged: false,
        terminal_at: None,
        deferred_join: None,
        dismissed: false,
    }
}
fn cancellation(invite: &Invite) -> Cancellation {
    Cancellation {
        invitation: invite.offer.id,
        group: invite.offer.group,
        owner: invite.offer.owner.rns,
        expires: invite.offer.expires,
    }
}
fn step(db: &Database, now: u64) -> crate::enrollment::Maintenance {
    let r = read(db);
    let plan = r.maintenance(now).unwrap();
    let tx = db.begin_write().unwrap();
    plan.write.stage(&tx, identity()).unwrap();
    tx.commit().unwrap();
    plan.result
}
fn phase(db: &Database, selected: super::super::maintenance::Phase) {
    // Only deterministic internal progress is selected; policy/counts/payloads
    // are untouched. This lets each ownership boundary be tested directly.
    let mut header = read(db).header;
    header.maintenance_phase = selected;
    let tx = db.begin_write().unwrap();
    tx.open_table(HEADER)
        .unwrap()
        .insert(0, encode(&header).unwrap().as_slice())
        .unwrap();
    tx.commit().unwrap();
}

#[test]
fn thousand_receipts_retire_in_bounded_prefixes_with_atomic_floors_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    let db = Database::create(&path).unwrap();
    create(
        &db,
        CatalogLimits {
            max_catalog_bytes: 16 * 1024 * 1024,
            max_maintenance_rows: 7,
            ..Default::default()
        },
    );
    let entry = seed_membership(&db);
    for start in (0..1000).step_by(32) {
        let r = read(&db);
        let mut edit = write::Edit::new(&r).unwrap();
        for n in start..(start + 32).min(1000) {
            edit = edit
                .put_receipt(
                    &r,
                    &rows::Receipt {
                        request: request(n),
                        namespace: None,
                    },
                )
                .unwrap();
        }
        commit(&db, edit);
    }
    let r = read(&db);
    let bound = rows::Receipt {
        request: request(1001),
        namespace: Some(rows::NamespaceReceipt {
            group: entry.group,
            generation: entry.generation,
            incarnation: entry.incarnation,
        }),
    };
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_receipt(&r, &bound)
            .unwrap(),
    );
    let later = NOW + crate::enrollment::RETRY_HORIZON_SECS;
    let r = read(&db);
    let original = r.header.fingerprint().unwrap();
    work::take();
    let plan = r.maintenance(later).unwrap();
    assert_eq!(plan.result.retired_requests, 7);
    assert!(
        work::take() <= 7,
        "bounded candidate decoding is independent of 1000 stored receipts"
    );
    let tx = db.begin_write().unwrap();
    plan.write.stage(&tx, identity()).unwrap();
    tx.abort().unwrap();
    assert_eq!(read(&db).header.fingerprint().unwrap(), original);
    let tx = db.begin_write().unwrap();
    plan.write.stage(&tx, identity()).unwrap();
    tx.commit().unwrap();
    let r = read(&db);
    assert_eq!(r.header.request_retired_before, NOW);
    assert_eq!(r.header.counts.requests, 994);
    assert!(
        r.receipt(request(9).id, request(9).fingerprint)
            .unwrap()
            .is_some(),
        "same-second remaining receipts replay by exact key before the inclusive floor"
    );
    drop(r);
    drop(db);
    let db = Database::open(&path).unwrap();
    let mut retired = 7;
    for _ in 0..1000 {
        // Clock rollback cannot resurrect already removed receipts; the exact
        // persisted floor permits completing the safe same-second prefix.
        let result = step(&db, NOW);
        assert!(result.retired_requests <= 7);
        retired += result.retired_requests;
        if retired == 1000 {
            break;
        }
    }
    assert_eq!(retired, 1000);
    let r = read(&db);
    assert_eq!(r.header.counts.requests, 1);
    assert!(
        r.receipt(bound.request.id, bound.request.fingerprint)
            .unwrap()
            .is_some(),
        "namespace-backed MAX sentinel is never ordinary expiry work"
    );
    r.inspect().unwrap();
}

#[test]
fn invitation_cursor_crosses_nonretirable_rows_and_categories_do_not_starve() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(
        &db,
        CatalogLimits {
            max_maintenance_rows: 1,
            ..Default::default()
        },
    );
    let entry = template();
    for n in 1..=9 {
        let mut invite = incoming(&entry, 4, n);
        if n != 9 {
            invite.state = InvitationState::JoinRequested;
        }
        let r = read(&db);
        commit(
            &db,
            write::Edit::new(&r)
                .unwrap()
                .put_invitation(&r, &invite)
                .unwrap(),
        );
    }
    let cancel = cancellation(&incoming(&entry, 8, 20));
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_cancellation(&r, &cancel)
            .unwrap()
            .put_receipt(
                &r,
                &rows::Receipt {
                    request: request(1),
                    namespace: None,
                },
            )
            .unwrap(),
    );
    for _ in 0..80 {
        work::take();
        step(&db, NOW + crate::enrollment::RETRY_HORIZON_SECS);
        assert!(
            work::take() < 24,
            "one-candidate policy bounds work even when earlier accepted invitations cannot retire"
        );
    }
    let r = read(&db);
    assert_eq!(r.header.counts.invitations, 8);
    assert_eq!(r.header.counts.requests, 0);
    assert_eq!(r.header.counts.cancellations, 0);
    assert_eq!(r.header.counts.pending_invitations, 0);
    for n in 1..9 {
        assert!(r
            .invitation(incoming(&entry, 4, n).offer.id)
            .unwrap()
            .is_some());
    }
    assert!(r
        .invitation(incoming(&entry, 4, 9).offer.id)
        .unwrap()
        .is_none());
    r.inspect().unwrap();
}

#[test]
fn pending_counts_coalesce_and_terminal_presentation_does_not_spend_admission() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let entry = template();
    let mut first = incoming(&entry, 9, 1);
    let second = incoming(&entry, 9, 2);
    let third = incoming(&entry, 9, 3);
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_invitation(&r, &first)
            .unwrap()
            .put_invitation(&r, &second)
            .unwrap(),
    );
    let r = read(&db);
    assert_eq!(r.header.counts.pending_invitations, 2);
    assert_eq!(r.header.counts.incoming_peers, 1);
    assert!(matches!(
        write::Edit::new(&r).unwrap().put_invitation(&r, &third),
        Err(EnrollmentError::Capacity)
    ));
    assert_eq!(
        read(&db).header.fingerprint().unwrap(),
        r.header.fingerprint().unwrap()
    );
    first.state = InvitationState::Declined;
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_invitation(&r, &first)
            .unwrap()
            .put_invitation(&r, &third)
            .unwrap(),
    );
    let r = read(&db);
    assert_eq!(r.header.counts.invitations, 3);
    assert_eq!(r.header.counts.pending_invitations, 2);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .remove_invitation(&r, second.offer.id)
            .unwrap()
            .remove_invitation(&r, third.offer.id)
            .unwrap(),
    );
    let r = read(&db);
    assert_eq!(r.header.counts.pending_invitations, 0);
    assert_eq!(r.header.counts.incoming_peers, 0);
    assert!(r.invitation(first.offer.id).unwrap().is_some());
    r.inspect().unwrap();
}

#[test]
fn cancellation_before_offer_and_paired_retirement_keep_exact_peer_accounting() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(
        &db,
        CatalogLimits {
            max_incoming_invitations: 1,
            max_incoming_per_peer: 1,
            ..Default::default()
        },
    );
    let entry = template();
    let invite = incoming(&entry, 10, 1);
    let cancel = cancellation(&invite);
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_cancellation(&r, &cancel)
            .unwrap(),
    );
    assert_eq!(read(&db).header.counts.unsolicited_cancellations, 1);
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_invitation(&r, &invite)
            .unwrap(),
    );
    let r = read(&db);
    assert_eq!(r.header.counts.unsolicited_cancellations, 0);
    assert_eq!(r.header.counts.pending_invitations, 1);
    let other = cancellation(&incoming(&entry, 11, 2));
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_cancellation(&r, &other)
            .unwrap(),
    );
    phase(&db, super::super::maintenance::Phase::Invitations);
    let result = step(&db, NOW + 60);
    assert_eq!(result.retired_invitations, 1);
    let r = read(&db);
    assert_eq!(r.header.counts.pending_invitations, 0);
    assert_eq!(r.header.counts.unsolicited_cancellations, 1);
    assert_eq!(r.header.counts.cancellations, 1);
    assert!(r.cancellation(invite.offer.id).unwrap().is_none());
    assert!(r.cancellation(other.invitation).unwrap().is_some());
    r.inspect().unwrap();
}

#[test]
fn maintenance_missing_marker_aborts_rows_counters_cursor_and_floor() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let entry = template();
    let invite = incoming(&entry, 9, 1);
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_invitation(&r, &invite)
            .unwrap(),
    );
    phase(&db, super::super::maintenance::Phase::Invitations);
    let r = read(&db);
    let before = r.header.fingerprint().unwrap();
    let plan = r.maintenance(NOW + 60).unwrap();
    let tx = db.begin_write().unwrap();
    tx.open_table(PENDING_INVITATIONS)
        .unwrap()
        .remove(b"".as_slice())
        .unwrap();
    tx.commit().unwrap();
    let tx = db.begin_write().unwrap();
    assert!(matches!(
        plan.write.stage(&tx, identity()),
        Err(EnrollmentError::Corrupt)
    ));
    tx.abort().unwrap();
    let tx = db.begin_read().unwrap();
    let header = tx.open_table(HEADER).unwrap();
    assert_eq!(
        Header::decode(header.get(0).unwrap().unwrap().value(), identity())
            .unwrap()
            .fingerprint()
            .unwrap(),
        before
    );
    assert!(tx
        .open_table(INVITATIONS)
        .unwrap()
        .get(invite.offer.id.as_slice())
        .unwrap()
        .is_some());
    assert!(
        Read::open(&tx, identity()).is_err(),
        "cold/open integrity cannot recreate the missing marker"
    );
}

#[test]
fn unsolicited_index_or_counter_corruption_is_rejected_on_cold_inspection() {
    for corrupt_counter in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("catalog")).unwrap();
        create(&db, CatalogLimits::default());
        let entry = template();
        let row = cancellation(&incoming(&entry, 10, 1));
        let r = read(&db);
        commit(
            &db,
            write::Edit::new(&r)
                .unwrap()
                .put_cancellation(&r, &row)
                .unwrap(),
        );
        read(&db).inspect().unwrap();
        let tx = db.begin_write().unwrap();
        if corrupt_counter {
            tx.open_table(INCOMING_PEERS)
                .unwrap()
                .insert(
                    row.owner.as_slice(),
                    encode(&super::super::maintenance::Incoming {
                        invitations: 0,
                        cancellations: 2,
                    })
                    .unwrap()
                    .as_slice(),
                )
                .unwrap();
        } else {
            let key = keys::peer_item(row.owner, row.invitation);
            tx.open_table(UNSOLICITED_CANCELLATIONS)
                .unwrap()
                .insert(key.as_slice(), [2; 32].as_slice())
                .unwrap();
        }
        tx.commit().unwrap();
        assert!(read(&db).inspect().is_err());
    }
}

#[test]
fn invitation_growth_reserve_covers_each_mutable_encoding_and_rejects_short_rows() {
    let invite = incoming(&template(), 9, 51);
    let mut maximum = invite.clone();
    maximum.state = InvitationState::UnsupportedProfile;
    maximum.terminal_at = Some(u64::MAX);
    maximum.deferred_join = Some([u8::MAX; 32]);
    let maximum_bytes = encode(&maximum).unwrap().len();
    for state in [
        InvitationState::Pending,
        InvitationState::JoinRequested,
        InvitationState::Joined,
        InvitationState::Declined,
        InvitationState::Expired,
        InvitationState::Cancelled,
        InvitationState::UnsupportedProfile,
    ] {
        for acknowledged in [false, true] {
            for dismissed in [false, true] {
                for terminal_at in [None, Some(0), Some(u64::MAX)] {
                    for deferred_join in [None, Some([0; 32]), Some([u8::MAX; 32])] {
                        let mut row = invite.clone();
                        row.state = state;
                        row.acknowledged = acknowledged;
                        row.dismissed = dismissed;
                        row.terminal_at = terminal_at;
                        row.deferred_join = deferred_join;
                        let actual = encode(&row).unwrap().len();
                        let reserve = obligations::invitation_reserve(&row, maximum_bytes).unwrap();
                        assert_eq!(
                            u64::try_from(actual).unwrap() + reserve,
                            u64::try_from(maximum_bytes).unwrap()
                        );
                    }
                }
            }
        }
    }
    assert!(encode(&invite).unwrap().len() < maximum_bytes);
    assert!(
        matches!(
            obligations::invitation_reserve(&invite, maximum_bytes - 1),
            Err(EnrollmentError::Capacity)
        ),
        "admission must fund a row that can actually be encoded later"
    );
}

#[test]
fn admitted_invitation_transitions_and_cleanup_fit_exact_saved_byte_quota() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    let mut invite = incoming(&template(), 9, 52);
    let maximum = encode(&invite).unwrap().len()
        + usize::try_from(obligations::invitation_reserve(&invite, 16 * 1024).unwrap()).unwrap();
    create(
        &db,
        CatalogLimits {
            max_row_bytes: u32::try_from(maximum).unwrap(),
            ..Default::default()
        },
    );
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_invitation(&r, &invite)
            .unwrap(),
    );
    let mut header = read(&db).header;
    header.limits.max_catalog_bytes = header.bytes + header.reserved;
    let quota = header.limits.max_catalog_bytes;
    let tx = db.begin_write().unwrap();
    tx.open_table(HEADER)
        .unwrap()
        .insert(0, encode(&header).unwrap().as_slice())
        .unwrap();
    tx.commit().unwrap();
    for state in [
        InvitationState::JoinRequested,
        InvitationState::Joined,
        InvitationState::Cancelled,
        InvitationState::Declined,
        InvitationState::Expired,
        InvitationState::UnsupportedProfile,
    ] {
        invite.state = state;
        invite.terminal_at = Some(u64::MAX);
        invite.deferred_join = Some([u8::MAX; 32]);
        let r = read(&db);
        let before = r.header.fingerprint().unwrap();
        let write = write::Edit::new(&r)
            .unwrap()
            .put_invitation(&r, &invite)
            .unwrap()
            .finish()
            .unwrap();
        let tx = db.begin_write().unwrap();
        write.stage(&tx, identity()).unwrap();
        tx.abort().unwrap();
        assert_eq!(read(&db).header.fingerprint().unwrap(), before);
        let tx = db.begin_write().unwrap();
        write.stage(&tx, identity()).unwrap();
        tx.commit().unwrap();
        let r = read(&db);
        assert!(r.header.bytes + r.header.reserved <= quota);
        for work_id in work::invitation_ids(invite.offer.id) {
            assert_eq!(
                r.work_source(work_id).unwrap(),
                Some(work::WorkSource::Invitation(invite.offer.id))
            );
        }
        r.inspect().unwrap();
    }
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .remove_invitation(&r, invite.offer.id)
            .unwrap(),
    );
    let r = read(&db);
    assert_eq!(r.header.bytes, 0);
    assert_eq!(r.header.reserved, 0);
    assert_eq!(r.header.counts.work_ids, 0);
    r.inspect().unwrap();
}

#[test]
fn same_edit_admission_then_retirement_keeps_original_index_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let invite = incoming(&template(), 9, 53);
    let cancel = cancellation(&invite);
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_cancellation(&r, &cancel)
            .unwrap()
            .put_invitation(&r, &invite)
            .unwrap()
            .remove_invitation(&r, invite.offer.id)
            .unwrap()
            .remove_cancellation(&r, invite.offer.id)
            .unwrap(),
    );
    let r = read(&db);
    assert_eq!(r.header.bytes, 0);
    assert_eq!(r.header.reserved, 0);
    assert_eq!(r.header.counts.work_ids, 0);
    r.inspect().unwrap();
}

#[test]
fn pending_outgoing_reservations_end_at_exact_added_binding_without_forgetting_work() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let entry = seed_membership(&db);
    let mut invite = incoming(&entry, 9, 54);
    invite.offer.owner = entry.owner.clone();
    invite.offer.recipient = [9; 16];
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_invitation(&r, &invite)
            .unwrap(),
    );
    let r = read(&db);
    assert_eq!(r.current(entry.group).unwrap().unwrap().pending_offers, 1);
    assert_eq!(
        r.pending_recipient(entry.group, [9; 16]).unwrap(),
        Some(invite.offer.id)
    );
    let before = r.header.fingerprint().unwrap();
    let mut competing = invite.clone();
    competing.offer.id = [77; 32];
    assert!(matches!(
        write::Edit::new(&r).unwrap().put_invitation(&r, &competing),
        Err(EnrollmentError::Conflict)
    ));
    assert_eq!(read(&db).header.fingerprint().unwrap(), before);
    let binding = rows::InvitationBinding {
        namespace: rows::NamespaceReceipt {
            group: entry.group,
            generation: entry.generation,
            incarnation: entry.incarnation,
        },
        member: Member {
            rns: [9; 16],
            ..entry.local.clone()
        },
    };
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .bind_invitation(&r, invite.offer.id, &binding)
            .unwrap(),
    );
    let r = read(&db);
    assert_eq!(r.current(entry.group).unwrap().unwrap().pending_offers, 0);
    assert_eq!(r.pending_recipient(entry.group, [9; 16]).unwrap(), None);
    assert!(r.invitation(invite.offer.id).unwrap().is_some());
    assert_eq!(r.header.counts.work_ids, 5);
    r.inspect().unwrap();
    // Repeated state updates and a duplicate Added cannot consume twice or
    // reinstall the reservation while the original Pending presentation stays.
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .bind_invitation(&r, invite.offer.id, &binding)
            .unwrap()
            .put_invitation(&r, &invite)
            .unwrap(),
    );
    let r = read(&db);
    assert_eq!(r.current(entry.group).unwrap().unwrap().pending_offers, 0);
    r.inspect().unwrap();
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .remove_invitation(&r, invite.offer.id)
            .unwrap(),
    );
    let r = read(&db);
    assert_eq!(r.header.counts.work_ids, 0);
    assert!(r.invitation_binding(invite.offer.id).unwrap().is_some());
    r.inspect().unwrap();
}

#[test]
fn work_index_exact_ownership_is_checked_and_sync_transition_coalesces() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let (mut current, mut entry) = membership(&template(), 22);
    current.owner = [4; 16];
    entry.owner.rns = [4; 16];
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_current(&r, entry.group, &current)
            .unwrap()
            .put_incarnation(
                &r,
                keys::incarnation(entry.group, entry.generation),
                &rows::Incarnation::Live(entry.clone()),
            )
            .unwrap(),
    );
    let first = work::sync_id(identity().into_bytes(), &current).unwrap();
    let r = read(&db);
    assert_eq!(
        r.work_source(first).unwrap(),
        Some(work::WorkSource::Sync(entry.group))
    );
    let mut middle = current.clone();
    middle.head = Some([12; 32]);
    let middle_id = work::sync_id(identity().into_bytes(), &middle).unwrap();
    current.head = Some([13; 32]);
    let last = work::sync_id(identity().into_bytes(), &current).unwrap();
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_current(&r, entry.group, &middle)
            .unwrap()
            .put_current(&r, entry.group, &current)
            .unwrap(),
    );
    let r = read(&db);
    assert!(r.work_source(first).unwrap().is_none());
    assert!(r.work_source(middle_id).unwrap().is_none());
    assert_eq!(
        r.work_source(last).unwrap(),
        Some(work::WorkSource::Sync(entry.group))
    );
    assert_eq!(r.header.counts.work_ids, 1);
    r.inspect().unwrap();
    // A wrong-source row is rejected even when its shape and byte length match.
    let tx = db.begin_write().unwrap();
    tx.open_table(WORK_IDS)
        .unwrap()
        .insert(
            last.as_slice(),
            encode(&work::WorkSource::Notice(entry.group))
                .unwrap()
                .as_slice(),
        )
        .unwrap();
    tx.commit().unwrap();
    assert!(matches!(
        read(&db).work_source(last),
        Err(EnrollmentError::Corrupt)
    ));
    assert!(matches!(read(&db).inspect(), Err(EnrollmentError::Corrupt)));
}

#[test]
fn future_notice_reserve_includes_completion_index_and_checks_selected_row_size() {
    let notice = rows::Notice {
        terminal: [u8::MAX; 32],
        recipient: [u8::MAX; 16],
        acknowledged: false,
    };
    let work = work::WorkSource::Notice([u8::MAX; 32]);
    let encoded_notice = encode(&notice).unwrap().len();
    let encoded_work = encode(&work).unwrap().len();
    let limit = encoded_notice.max(encoded_work);
    assert_eq!(
        obligations::notice_bytes(limit).unwrap(),
        u64::try_from(32 + encoded_notice + 32 + encoded_work).unwrap()
    );
    assert!(matches!(
        obligations::notice_bytes(limit - 1),
        Err(EnrollmentError::Capacity)
    ));
}

#[test]
fn deleted_offers_refund_pending_admission_in_bounded_turns_without_hiding_fresh_offers() {
    for outgoing in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("catalog")).unwrap();
        create(
            &db,
            CatalogLimits {
                max_maintenance_rows: 1,
                max_incoming_invitations: 4,
                max_incoming_per_peer: 4,
                ..Default::default()
            },
        );
        let (mut current, mut entry) = membership(&template(), 91);
        if !outgoing {
            entry.owner.rns = [9; 16];
            current.owner = entry.owner.rns;
        }
        let r = read(&db);
        commit(
            &db,
            write::Edit::new(&r)
                .unwrap()
                .put_current(&r, entry.group, &current)
                .unwrap()
                .put_incarnation(
                    &r,
                    keys::incarnation(entry.group, entry.generation),
                    &rows::Incarnation::Live(entry.clone()),
                )
                .unwrap(),
        );
        let mut offers = Vec::new();
        for n in 0..2 {
            let mut invite = incoming(&entry, 9, 90 + n);
            invite.offer.owner = entry.owner.clone();
            if outgoing {
                invite.offer.recipient = [10 + u8::try_from(n).unwrap(); 16];
            }
            let r = read(&db);
            commit(
                &db,
                write::Edit::new(&r)
                    .unwrap()
                    .put_invitation(&r, &invite)
                    .unwrap(),
            );
            offers.push(invite);
        }
        let r = read(&db);
        current = r.current(entry.group).unwrap().unwrap();
        current.invitations_deleted_through = r.header.revision;
        commit(
            &db,
            write::Edit::new(&r)
                .unwrap()
                .put_current(&r, entry.group, &current)
                .unwrap(),
        );
        let mut fresh = offers[0].clone();
        fresh.offer.id = [99; 32];
        if outgoing {
            fresh.offer.recipient = [12; 16];
        }
        let r = read(&db);
        commit(
            &db,
            write::Edit::new(&r)
                .unwrap()
                .put_invitation(&r, &fresh)
                .unwrap(),
        );
        for expected in [2, 1] {
            phase(&db, super::super::maintenance::Phase::Invitations);
            step(&db, NOW + 1);
            let r = read(&db);
            r.inspect().unwrap();
            assert_eq!(
                if outgoing {
                    r.header.counts.pending_recipients
                } else {
                    r.header.counts.pending_invitations
                },
                expected
            );
        }
        let r = read(&db);
        for invite in offers {
            let retained = r.invitation(invite.offer.id).unwrap().unwrap();
            assert_eq!(retained.state, InvitationState::Cancelled);
            assert!(r.invitation_deleted(&retained).unwrap());
        }
        let fresh = r.invitation(fresh.offer.id).unwrap().unwrap();
        assert_eq!(fresh.state, InvitationState::Pending);
        assert!(!r.invitation_deleted(&fresh).unwrap());
    }
}
