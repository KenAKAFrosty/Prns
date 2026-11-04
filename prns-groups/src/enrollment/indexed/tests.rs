#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
#[path = "purge_tests.rs"]
mod purge_tests;
const NOW: u64 = 1_800_000_000;

fn identity() -> IdentityHash {
    IdentityHash::new([71; 16])
}
fn request(n: u32) -> Request {
    let mut entropy = [1; 20];
    entropy[..4].copy_from_slice(&n.to_be_bytes());
    Request {
        id: prns_groups_core::retry::RequestToken::new(NOW, entropy)
            .unwrap()
            .into_bytes(),
        fingerprint: [3; 32],
        result: [4; 32],
    }
}
fn create(db: &Database, limits: CatalogLimits) {
    let tx = db.begin_write().unwrap();
    initialize_in(&tx, identity(), limits).unwrap();
    tx.commit().unwrap();
}
fn read(db: &Database) -> Read {
    Read::open(&db.begin_read().unwrap(), identity()).unwrap()
}
fn commit(db: &Database, edit: write::Edit) -> write::Write {
    let write = edit.finish().unwrap();
    let tx = db.begin_write().unwrap();
    write.stage(&tx, identity()).unwrap();
    tx.commit().unwrap();
    write
}
fn template() -> Entry {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("template")).unwrap();
    let engine = Enrollment::create_new(&db, identity()).unwrap();
    let group = engine
        .create(
            &mut RedbStoreOwner::new(&db),
            request(0).id,
            "template",
            NOW,
        )
        .unwrap();
    Enrollment::find(&engine.read().unwrap(), group.0)
        .unwrap()
        .clone()
}
fn membership(template: &Entry, n: u32) -> (rows::Current, Entry) {
    let mut entry = template.clone();
    entry.group[..4].copy_from_slice(&n.to_be_bytes());
    entry.namespace[..4].copy_from_slice(&n.to_be_bytes());
    entry.incarnation[..4].copy_from_slice(&n.to_be_bytes());
    entry.title = format!("conversation {n:04}");
    let current = rows::Current {
        generation: entry.generation,
        title: entry.title.clone(),
        owner: entry.owner.rns,
        lifecycle: GroupLifecycle::Active,
        ready: true,
        member_count: 1,
        pending_offers: 0,
        head: Some([11; 32]),
        departure: None,
        visible: true,
        purge_through: None,
        invitations_deleted_through: 0,
    };
    (current, entry)
}

#[test]
fn thousand_keyed_rows_reopen_with_bounded_pages_and_exact_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog");
    let db = Database::create(&path).unwrap();
    create(
        &db,
        CatalogLimits {
            max_catalog_bytes: 16 * 1024 * 1024,
            ..Default::default()
        },
    );
    let template = template();
    let started = std::time::Instant::now();
    for n in 0..1000 {
        let (current, entry) = membership(&template, n);
        let row = rows::Receipt {
            request: request(n),
            namespace: Some(rows::NamespaceReceipt {
                group: entry.group,
                generation: entry.generation,
                incarnation: entry.incarnation,
            }),
        };
        let r = read(&db);
        let edit = write::Edit::new(&r)
            .unwrap()
            .put_current(&r, entry.group, &current)
            .unwrap()
            .put_incarnation(
                &r,
                keys::incarnation(entry.group, entry.generation),
                &rows::Incarnation::Live(entry),
            )
            .unwrap()
            .put_receipt(&r, &row)
            .unwrap();
        commit(&db, edit);
    }
    let populated = started.elapsed();
    drop(db);
    let db = Database::open(&path).unwrap();
    let r = read(&db);
    r.inspect().unwrap();
    assert_eq!(r.header.counts.groups, 1000);
    assert_eq!(r.header.counts.live, 1000);
    assert_eq!(r.header.counts.requests, 1000);
    let header = db
        .begin_read()
        .unwrap()
        .open_table(HEADER)
        .unwrap()
        .get(0)
        .unwrap()
        .unwrap();
    assert!(header.value().len() < 4096);

    work::take();
    assert_eq!(
        r.receipt(request(734).id, request(734).fingerprint)
            .unwrap()
            .unwrap()
            .request
            .result,
        [4; 32]
    );
    assert_eq!(
        work::take(),
        1,
        "exact lookup must not scan namespaces or other receipts"
    );
    let engine = Enrollment::open_attached(&db, identity(), r.header.limits).unwrap();
    let mut after = None;
    let mut seen = 0;
    let page_bytes =
        (std::mem::size_of::<crate::ConversationSummary>() + profile::MAX_TITLE_BYTES) as u64 * 3;
    loop {
        let page = engine
            .conversation_summaries(
                after,
                crate::PageLimits {
                    max_rows: 17,
                    max_bytes: page_bytes,
                },
            )
            .unwrap();
        assert!(!page.items.is_empty());
        assert!(
            page.items.len() < 17,
            "the byte bound must actually limit this page"
        );
        let owned = std::mem::size_of::<crate::ConversationPage>()
            + page.items.capacity() * std::mem::size_of::<crate::ConversationSummary>()
            + page
                .items
                .iter()
                .map(|row| row.title.capacity())
                .sum::<usize>();
        assert!(owned <= page_bytes as usize);
        assert!(page
            .items
            .windows(2)
            .all(|pair| pair[0].group.0 < pair[1].group.0));
        if let Some(previous) = after {
            assert!(page.items[0].group.0 > previous.0);
        }
        let decoded = work::take();
        assert!(
            decoded <= page.items.len() + 1,
            "at most one bounded lookahead row"
        );
        seen += page.items.len();
        after = page.next;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(seen, 1000);
    eprintln!("indexed catalog1000 storage-only populate={populated:?}; exact receipt decodes1; pages bounded by actual owned bytes");
}

#[test]
fn stale_projection_and_late_missing_marker_abort_all_staged_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let r = read(&db);
    let receipt = rows::Receipt {
        request: request(1),
        namespace: None,
    };
    let stale = write::Edit::new(&r)
        .unwrap()
        .put_receipt(&r, &receipt)
        .unwrap()
        .finish()
        .unwrap();
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_receipt(
                &r,
                &rows::Receipt {
                    request: request(2),
                    namespace: None,
                },
            )
            .unwrap(),
    );
    let before = read(&db).header.fingerprint().unwrap();
    let tx = db.begin_write().unwrap();
    assert!(matches!(
        stale.stage(&tx, identity()),
        Err(EnrollmentError::Conflict)
    ));
    tx.abort().unwrap();
    assert_eq!(read(&db).header.fingerprint().unwrap(), before);
    assert!(read(&db)
        .receipt(receipt.request.id, receipt.request.fingerprint)
        .unwrap()
        .is_none());

    let r = read(&db);
    let write = write::Edit::new(&r)
        .unwrap()
        .put_receipt(&r, &receipt)
        .unwrap()
        .finish()
        .unwrap();
    let tx = db.begin_write().unwrap();
    tx.open_table(REQUESTS)
        .unwrap()
        .remove(b"".as_slice())
        .unwrap();
    tx.commit().unwrap();
    let tx = db.begin_write().unwrap();
    assert!(matches!(
        write.stage(&tx, identity()),
        Err(EnrollmentError::Corrupt)
    ));
    tx.abort().unwrap();
    let tx = db.begin_read().unwrap();
    assert_eq!(
        tx.open_table(REQUEST_EXPIRY).unwrap().len().unwrap(),
        2,
        "earlier staged index insertion must roll back"
    );
    assert_eq!(
        tx.open_table(REQUESTS).unwrap().len().unwrap(),
        1,
        "missing marker must not be recreated"
    );
}

#[test]
fn unchanged_exact_receipt_is_idempotent_and_conflicting_input_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let receipt = rows::Receipt {
        request: request(5),
        namespace: None,
    };
    let r = read(&db);
    let first = commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_receipt(&r, &receipt)
            .unwrap(),
    );
    let r = read(&db);
    let duplicate = commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_receipt(&r, &receipt)
            .unwrap(),
    );
    assert_eq!(first.after, duplicate.after);
    assert_eq!(duplicate.before, duplicate.after);
    assert_eq!(read(&db).header.counts.requests, 1);
    let mut changed = receipt.clone();
    changed.request.fingerprint = [8; 32];
    assert!(matches!(
        write::Edit::new(&r).unwrap().put_receipt(&r, &changed),
        Err(EnrollmentError::RequestConflict)
    ));
    assert!(matches!(
        r.receipt(receipt.request.id, [8; 32]),
        Err(EnrollmentError::RequestConflict)
    ));
}

#[test]
fn partial_schema_wrong_identity_and_old_version_are_not_repaired() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    let tx = db.begin_write().unwrap();
    tx.open_table(INCARNATIONS).unwrap();
    tx.commit().unwrap();
    let tx = db.begin_write().unwrap();
    assert!(matches!(
        initialize_in(&tx, identity(), CatalogLimits::default()),
        Err(EnrollmentError::AlreadyInitialized)
    ));
    tx.abort().unwrap();
    assert!(matches!(
        Read::open(&db.begin_read().unwrap(), identity()),
        Err(EnrollmentError::MissingCatalog)
    ));

    let db2 = Database::create(dir.path().join("other")).unwrap();
    create(&db2, CatalogLimits::default());
    assert!(matches!(
        Read::open(&db2.begin_read().unwrap(), IdentityHash::new([1; 16])),
        Err(EnrollmentError::Authentication)
    ));
    let mut header = read(&db2).header;
    header.version = VERSION - 1;
    let tx = db2.begin_write().unwrap();
    tx.open_table(HEADER)
        .unwrap()
        .insert(0, encode(&header).unwrap().as_slice())
        .unwrap();
    tx.commit().unwrap();
    assert!(matches!(
        Read::open(&db2.begin_read().unwrap(), identity()),
        Err(EnrollmentError::UnsupportedVersion)
    ));
}

#[test]
fn row_transaction_and_page_budgets_fail_without_publishing() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(
        &db,
        CatalogLimits {
            max_transaction_rows: 1,
            ..Default::default()
        },
    );
    let r = read(&db);
    let receipt = rows::Receipt {
        request: request(1),
        namespace: None,
    };
    assert!(matches!(
        write::Edit::new(&r).unwrap().put_receipt(&r, &receipt),
        Err(EnrollmentError::Capacity)
    ));
    assert_eq!(read(&db).header.counts.requests, 0);
    assert!(matches!(
        Enrollment::open_attached(&db, identity(), r.header.limits)
            .unwrap()
            .conversation_summaries(
                None,
                crate::PageLimits {
                    max_rows: 1,
                    max_bytes: 1
                }
            ),
        Err(EnrollmentError::Invalid)
    ));
}

#[test]
fn reciprocal_index_and_accounting_corruption_fail_cold_validation() {
    for corrupt_index in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("catalog")).unwrap();
        create(&db, CatalogLimits::default());
        let receipt = rows::Receipt {
            request: request(1),
            namespace: None,
        };
        let r = read(&db);
        commit(
            &db,
            write::Edit::new(&r)
                .unwrap()
                .put_receipt(&r, &receipt)
                .unwrap(),
        );
        let tx = db.begin_write().unwrap();
        if corrupt_index {
            tx.open_table(REQUEST_EXPIRY)
                .unwrap()
                .insert(
                    receipt.retirement_key().unwrap().as_slice(),
                    [99; 32].as_slice(),
                )
                .unwrap();
        } else {
            let mut header = read(&db).header;
            header.bytes += 1;
            tx.open_table(HEADER)
                .unwrap()
                .insert(0, encode(&header).unwrap().as_slice())
                .unwrap();
        }
        tx.commit().unwrap();
        assert!(matches!(read(&db).inspect(), Err(EnrollmentError::Corrupt)));
    }
}

fn seed_membership(db: &Database) -> Entry {
    let (current, entry) = membership(&template(), 1);
    let r = read(db);
    commit(
        db,
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
    entry
}

#[test]
fn added_invitation_binding_survives_pruned_presentation_without_namespace_scan() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let entry = seed_membership(&db);
    let id = [5; 32];
    let invite = Invite {
        admission_revision: 0,
        offer: Offer {
            id,
            group: entry.group,
            title: entry.title.clone(),
            profile: entry.profile,
            owner: entry.owner.clone(),
            recipient: [9; 16],
            created: NOW,
            expires: NOW + 60,
        },
        state: InvitationState::Pending,
        acknowledged: false,
        terminal_at: None,
        deferred_join: None,
        dismissed: false,
    };
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_invitation(&r, &invite)
            .unwrap(),
    );
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
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .bind_invitation(&r, id, &binding)
            .unwrap()
            .remove_invitation(&r, id)
            .unwrap(),
    );
    let r = read(&db);
    r.inspect().unwrap();
    assert!(r.invitation(id).unwrap().is_none());
    work::take();
    assert!(r.invitation_binding(id).unwrap().as_ref() == Some(&binding));
    assert_eq!(
        work::take(),
        2,
        "one binding and its exact incarnation, independent of catalog size"
    );
    let mut wrong = binding.clone();
    wrong.namespace.generation += 1;
    assert!(matches!(
        write::Edit::new(&r)
            .unwrap()
            .bind_invitation(&r, id, &wrong),
        Err(EnrollmentError::Conflict)
    ));
}

#[test]
fn admitted_audience_obligation_survives_offer_retirement_without_ceiling_charge() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let entry = seed_membership(&db);
    let baseline = read(&db).header.reserved;
    let peer = [19; 16];
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .offer_audience(&r, entry.group, peer, true)
            .unwrap(),
    );
    let one = read(&db).header.reserved;
    assert_eq!(
        one - baseline,
        obligations::close_notice_bytes(CatalogLimits::default().row_bytes().unwrap()).unwrap()
    );
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .offer_audience(&r, entry.group, peer, true)
            .unwrap(),
    );
    assert_eq!(
        read(&db).header.reserved,
        one,
        "same recipient shares one Close notice"
    );
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .admit_audience(&r, entry.group, peer)
            .unwrap(),
    );
    for _ in 0..2 {
        let r = read(&db);
        commit(
            &db,
            write::Edit::new(&r)
                .unwrap()
                .offer_audience(&r, entry.group, peer, false)
                .unwrap(),
        );
    }
    let r = read(&db);
    assert!(r.audience(entry.group, peer).unwrap().unwrap().admitted);
    assert_eq!(
        r.header.reserved, one,
        "retiring offers does not forget a former member's Close obligation"
    );
    r.inspect().unwrap();
}

#[test]
fn one_edit_coalesces_shared_peer_counters_and_refunds_deleted_projection() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let entry = seed_membership(&db);
    let baseline = read(&db).header.reserved;
    let peer = [31; 16];
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .offer_audience(&r, entry.group, peer, true)
            .unwrap()
            .offer_audience(&r, entry.group, peer, true)
            .unwrap(),
    );
    let r = read(&db);
    assert_eq!(r.audience(entry.group, peer).unwrap().unwrap().offers, 2);
    assert_eq!(r.header.counts.audience, 1);
    assert_eq!(
        r.header.reserved - baseline,
        obligations::close_notice_bytes(CatalogLimits::default().row_bytes().unwrap()).unwrap()
    );
    r.inspect().unwrap();
    let before = r.header.bytes;
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .offer_audience(&r, entry.group, peer, false)
            .unwrap()
            .offer_audience(&r, entry.group, peer, false)
            .unwrap(),
    );
    let r = read(&db);
    assert!(r.audience(entry.group, peer).unwrap().is_none());
    assert_eq!(r.header.counts.audience, 0);
    assert_eq!(r.header.reserved, baseline);
    assert!(r.header.bytes < before);
    r.inspect().unwrap();
}

fn close_fanout_fixture(max_transaction_bytes: u64) -> usize {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(
        &db,
        CatalogLimits {
            max_row_bytes: 4096,
            max_transaction_bytes,
            ..Default::default()
        },
    );
    let entry = seed_membership(&db);
    for n in 0u16..256 {
        let mut peer = [0; 16];
        peer[..2].copy_from_slice(&n.to_be_bytes());
        let r = read(&db);
        commit(
            &db,
            write::Edit::new(&r)
                .unwrap()
                .admit_audience(&r, entry.group, peer)
                .unwrap(),
        );
    }
    let statement = TerminalStatement {
        version: WIRE_VERSION,
        group: entry.group,
        title: entry.title.clone(),
        owner: entry.owner.clone(),
        head: [11; 32],
        revision: 1,
        kind: TerminalKind::Closed,
    };
    let signature = entry
        .identity()
        .unwrap()
        .sign_enrollment(&encode(&statement).unwrap())
        .unwrap();
    let proof = Terminal {
        statement,
        signature,
    };
    let id = proof.id().unwrap();
    let r = read(&db);
    let mut current = r.current(entry.group).unwrap().unwrap();
    current.lifecycle = GroupLifecycle::Closed;
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_current(&r, entry.group, &current)
            .unwrap()
            .put_terminal(
                &r,
                &rows::TerminalRow {
                    proof: proof.clone(),
                    after: None,
                    complete: false,
                },
            )
            .unwrap(),
    );
    // Recreate the selected exact quota boundary without inventing a profile
    // ceiling. This is an isolated storage fixture, not a policy migration API.
    let mut header = read(&db).header;
    header.limits.max_catalog_bytes = header.bytes + header.reserved;
    let tx = db.begin_write().unwrap();
    tx.open_table(HEADER)
        .unwrap()
        .insert(0, encode(&header).unwrap().as_slice())
        .unwrap();
    tx.commit().unwrap();
    let initial = read(&db).header;
    assert_eq!(initial.counts.audience, 256);
    assert_eq!(pending_catalog_notices(&read(&db)), 256);
    let mut steps = 0;
    loop {
        let r = read(&db);
        if r.terminal(id).unwrap().unwrap().complete {
            break;
        }
        let before = r.header.counts.notices;
        commit(
            &db,
            write::Edit::new(&r)
                .unwrap()
                .fanout_close(
                    &r,
                    id,
                    crate::PageLimits {
                        max_rows: 17,
                        max_bytes: 4096,
                    },
                )
                .unwrap(),
        );
        let now = read(&db);
        assert_eq!(pending_catalog_notices(&now), 256);
        assert!(now.header.counts.notices - before <= 17);
        assert!(now.header.bytes + now.header.reserved <= initial.limits.max_catalog_bytes);
        steps += 1;
    }
    let r = read(&db);
    r.inspect().unwrap();
    assert_eq!(r.header.counts.notices, 256);
    let row_bytes = r.header.limits.row_bytes().unwrap();
    assert_eq!(
        r.header.reserved,
        obligations::current_reserve(&r.current(entry.group).unwrap().unwrap(), row_bytes).unwrap()
            + obligations::incarnation_reserve(
                &r.incarnation(keys::incarnation(entry.group, entry.generation))
                    .unwrap()
                    .unwrap(),
                row_bytes
            )
            .unwrap()
    );
    assert_eq!(
        r.terminal_subject(entry.group, &TerminalKind::Closed)
            .unwrap()
            .unwrap()
            .proof
            .id()
            .unwrap(),
        id
    );
    let before = r.header.fingerprint().unwrap();
    let duplicate = commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .fanout_close(
                &r,
                id,
                crate::PageLimits {
                    max_rows: 17,
                    max_bytes: 4096,
                },
            )
            .unwrap(),
    );
    assert_eq!(duplicate.after, before);
    let notification = Notification {
        proof,
        recipient: [0; 16],
        acknowledged: false,
    };
    let notice = Enrollment::notice_id(&notification).unwrap();
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .acknowledge_notice(&r, notice)
            .unwrap(),
    );
    assert!(read(&db).notice(notice).unwrap().unwrap().acknowledged);
    assert_eq!(pending_catalog_notices(&read(&db)), 255);
    steps
}

fn pending_catalog_notices(read: &Read) -> u64 {
    let stamp = prns_groups_store_redb::change::Stamp {
        incarnation: StoreIncarnation::new([1; 24]).unwrap(),
        revision: read.header.revision,
    };
    let mut report = read.dependency_seed(stamp).unwrap();
    let mut cursor = crate::dependencies::CatalogCursor::Incarnations(None);
    loop {
        match read.dependency_step(&mut cursor, &mut report).unwrap() {
            dependencies::Step::Complete => break,
            // This fixture owns catalog accounting only. No namespace rows
            // are forged or interpreted as real service-state evidence.
            dependencies::Step::Namespace { .. } | dependencies::Step::CatalogRow => {}
        }
    }
    report.pending_terminal_notices
}

#[test]
fn close_fanout_to_256_actual_peers_uses_funded_bounded_steps_at_full_quota() {
    assert_eq!(close_fanout_fixture(4 * 1024 * 1024), 16);
}

#[test]
fn small_projection_byte_budget_makes_progress_without_smaller_caller_page() {
    let steps = close_fanout_fixture(4600);
    assert!(
        steps > 16,
        "projection bytes must shorten the requested17-row batch"
    );
    assert!(
        steps <= 256,
        "each successful nonterminal step must advance"
    );
}

#[test]
fn signed_terminal_must_match_full_known_owner_not_only_its_identity_hash() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let entry = seed_membership(&db);
    let other = LocalIdentity::generate([99; 32], 1).unwrap();
    let (binding, secret) = other.into_parts();
    let owner = Member::new(identity(), binding);
    assert_eq!(owner.rns, entry.owner.rns);
    assert_ne!(owner.key, entry.owner.key);
    let statement = TerminalStatement {
        version: WIRE_VERSION,
        group: entry.group,
        title: entry.title.clone(),
        owner: owner.clone(),
        head: [11; 32],
        revision: 1,
        kind: TerminalKind::Closed,
    };
    let other = LocalIdentity::from_parts(owner.binding(), secret).unwrap();
    let signature = other.sign_enrollment(&encode(&statement).unwrap()).unwrap();
    let proof = Terminal {
        statement,
        signature,
    };
    proof.verify(entry.group, &owner).unwrap();
    let r = read(&db);
    assert!(matches!(
        write::Edit::new(&r).unwrap().put_terminal(
            &r,
            &rows::TerminalRow {
                proof: proof.clone(),
                after: None,
                complete: false,
            }
        ),
        Err(EnrollmentError::Corrupt)
    ));
    let mut canonical = proof.clone();
    canonical.statement.owner = entry.owner.clone();
    canonical.signature = entry
        .identity()
        .unwrap()
        .sign_enrollment(&encode(&canonical.statement).unwrap())
        .unwrap();
    let old_id = canonical.id().unwrap();
    let old_row = rows::TerminalRow {
        proof: canonical,
        after: None,
        complete: false,
    };
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_terminal(&r, &old_row)
            .unwrap(),
    );
    read(&db).inspect().unwrap();
    // Replace all reciprocal indexes and byte accounting too. The foreign
    // proof is cryptographically valid; only its retained full owner differs.
    let new_id = proof.id().unwrap();
    let new_row = rows::TerminalRow {
        proof,
        after: None,
        complete: false,
    };
    let old_bytes = encode(&old_row).unwrap();
    let new_bytes = encode(&new_row).unwrap();
    let mut header = read(&db).header;
    header.bytes = header.bytes - old_bytes.len() as u64 + new_bytes.len() as u64;
    let tx = db.begin_write().unwrap();
    {
        let mut table = tx.open_table(TERMINALS).unwrap();
        table.remove(old_id.as_slice()).unwrap();
        table
            .insert(new_id.as_slice(), new_bytes.as_slice())
            .unwrap();
    }
    {
        let mut table = tx.open_table(TERMINAL_GROUPS).unwrap();
        table
            .remove(keys::group_item(entry.group, old_id).as_slice())
            .unwrap();
        table
            .insert(
                keys::group_item(entry.group, new_id).as_slice(),
                new_id.as_slice(),
            )
            .unwrap();
    }
    tx.open_table(TERMINAL_SUBJECTS)
        .unwrap()
        .insert(
            keys::terminal_subject(entry.group, &TerminalKind::Closed).as_slice(),
            new_id.as_slice(),
        )
        .unwrap();
    tx.open_table(HEADER)
        .unwrap()
        .insert(0, encode(&header).unwrap().as_slice())
        .unwrap();
    tx.commit().unwrap();
    assert!(matches!(read(&db).inspect(), Err(EnrollmentError::Corrupt)));
}

#[test]
fn unused_outbox_rows_cannot_hide_malformed_payload_behind_correct_counts() {
    for departure in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("catalog")).unwrap();
        create(&db, CatalogLimits::default());
        let mut header = read(&db).header;
        let table = if departure {
            header.counts.departures = 1;
            DEPARTURES
        } else {
            header.counts.cancellations = 1;
            CANCELLATIONS
        };
        header.bytes += 36;
        let tx = db.begin_write().unwrap();
        tx.open_table(table)
            .unwrap()
            .insert([1; 32].as_slice(), b"null".as_slice())
            .unwrap();
        tx.open_table(HEADER)
            .unwrap()
            .insert(0, encode(&header).unwrap().as_slice())
            .unwrap();
        tx.commit().unwrap();
        assert!(matches!(
            Read::open(&db.begin_read().unwrap(), identity()).and_then(|read| read.inspect()),
            Err(EnrollmentError::Corrupt)
        ));
    }
}

mod maintenance;

mod terminal_obligations;
