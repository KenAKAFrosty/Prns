use super::*;

fn terminal(entry: &Entry, kind: TerminalKind, revision: u64) -> Terminal {
    let statement = TerminalStatement {
        version: WIRE_VERSION,
        group: entry.group,
        title: entry.title.clone(),
        owner: entry.owner.clone(),
        head: [21; 32],
        revision,
        kind,
    };
    let signature = entry
        .identity()
        .unwrap()
        .sign_enrollment(&encode(&statement).unwrap())
        .unwrap();
    Terminal {
        statement,
        signature,
    }
}
fn exact_quota(db: &Database) -> u64 {
    let mut header = read(db).header;
    let quota = header.bytes + header.reserved;
    assert!(
        quota >= u64::from(header.limits.max_row_bytes),
        "exact quota {quota} is smaller than selected row limit {}",
        header.limits.max_row_bytes
    );
    header.limits.max_catalog_bytes = quota;
    let tx = db.begin_write().unwrap();
    tx.open_table(HEADER)
        .unwrap()
        .insert(0, encode(&header).unwrap().as_slice())
        .unwrap();
    tx.commit().unwrap();
    quota
}
fn tick(db: &Database) {
    let r = read(db);
    let plan = r.maintenance(NOW).unwrap();
    let tx = db.begin_write().unwrap();
    plan.write.stage(&tx, identity()).unwrap();
    tx.commit().unwrap();
}

#[test]
fn full_catalog_remove_and_close_spend_distinct_funded_proofs_notices_and_indexes() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(
        &db,
        CatalogLimits {
            max_maintenance_rows: 1,
            max_row_bytes: 4096,
            ..Default::default()
        },
    );
    let entry = seed_membership(&db);
    let mut first = entry.local.clone();
    first.rns = [8; 16];
    first.endpoint = [18; 32];
    let mut second = entry.local.clone();
    second.rns = [9; 16];
    second.endpoint = [19; 32];
    for (id, member) in [([8; 32], first.clone()), ([9; 32], second.clone())] {
        let r = read(&db);
        let binding = rows::InvitationBinding {
            namespace: rows::NamespaceReceipt {
                group: entry.group,
                generation: entry.generation,
                incarnation: entry.incarnation,
            },
            member: member.clone(),
        };
        commit(
            &db,
            write::Edit::new(&r)
                .unwrap()
                .bind_invitation(&r, id, &binding)
                .unwrap()
                .admit_audience(&r, entry.group, member.rns)
                .unwrap(),
        );
    }
    assert_eq!(read(&db).header.counts.terminal_obligations, 3);
    read(&db).inspect().unwrap();
    let quota = exact_quota(&db);
    let removed = terminal(&entry, TerminalKind::Removed(first.clone()), 1);
    let r = read(&db);
    let before = r.header.fingerprint().unwrap();
    let write = write::Edit::new(&r)
        .unwrap()
        .put_terminal(
            &r,
            &rows::TerminalRow {
                proof: removed.clone(),
                after: None,
                complete: true,
            },
        )
        .unwrap()
        .removal_notice(&r, &removed, first.rns)
        .unwrap()
        .put_receipt(
            &r,
            &rows::Receipt {
                request: request(21),
                namespace: Some(rows::NamespaceReceipt {
                    group: entry.group,
                    generation: entry.generation,
                    incarnation: entry.incarnation,
                }),
            },
        )
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
    r.inspect().unwrap();
    assert!(
        r.audience(entry.group, first.rns)
            .unwrap()
            .unwrap()
            .close_notice_funded
    );
    assert_eq!(r.header.counts.notices, 1);
    assert_eq!(r.header.counts.terminal_obligations, 2);
    assert!(r.header.bytes + r.header.reserved <= quota);
    let closed = terminal(&entry, TerminalKind::Closed, 2);
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
                    proof: closed.clone(),
                    after: None,
                    complete: false,
                },
            )
            .unwrap()
            .put_receipt(
                &r,
                &rows::Receipt {
                    request: request(22),
                    namespace: Some(rows::NamespaceReceipt {
                        group: entry.group,
                        generation: entry.generation,
                        incarnation: entry.incarnation,
                    }),
                },
            )
            .unwrap(),
    );
    for _ in 0..36 {
        tick(&db);
        let r = read(&db);
        r.inspect().unwrap();
        assert!(r.header.bytes + r.header.reserved <= quota);
        if r.terminal(closed.id().unwrap()).unwrap().unwrap().complete
            && r.header.counts.terminal_obligations == 0
        {
            break;
        }
    }
    let r = read(&db);
    assert!(r.terminal(closed.id().unwrap()).unwrap().unwrap().complete);
    assert_eq!(
        r.header.counts.terminal_obligations, 0,
        "unused Remove liability refunds after Close"
    );
    assert_eq!(
        r.header.counts.notices, 3,
        "Remove plus Close for both historical recipients"
    );
    assert_eq!(r.header.counts.work_ids, 3);
    for member in [first, second] {
        assert!(
            !r.audience(entry.group, member.rns)
                .unwrap()
                .unwrap()
                .close_notice_funded
        );
    }
}

#[test]
fn cold_inspection_rejects_missing_funding_even_with_consistent_byte_counters() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(&db, CatalogLimits::default());
    let entry = seed_membership(&db);
    let key = keys::terminal_subject(entry.group, &TerminalKind::Closed);
    let r = read(&db);
    let statement = r.obligation_statement(key, entry.group).unwrap();
    let mut header = r.header.clone();
    header.counts.terminal_obligations -= 1;
    header.bytes -= u64::try_from(key.len() + entry.group.len()).unwrap();
    header.reserved -=
        obligations::terminal_bytes(&statement, header.local, header.limits.row_bytes().unwrap())
            .unwrap();
    let tx = db.begin_write().unwrap();
    tx.open_table(TERMINAL_OBLIGATIONS)
        .unwrap()
        .remove(key.as_slice())
        .unwrap();
    tx.open_table(HEADER)
        .unwrap()
        .insert(0, encode(&header).unwrap().as_slice())
        .unwrap();
    tx.commit().unwrap();
    assert!(matches!(read(&db).inspect(), Err(EnrollmentError::Corrupt)));
}

#[test]
fn received_remove_and_close_fit_full_catalog_without_outgoing_notice_liability() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(
        &db,
        CatalogLimits {
            max_row_bytes: 4096,
            ..Default::default()
        },
    );
    let (mut current, mut entry) = membership(&template(), 31);
    entry.owner.rns = [33; 16];
    entry.owner.endpoint = [33; 32];
    current.owner = entry.owner.rns;
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
            .unwrap()
            .bind_invitation(
                &r,
                [31; 32],
                &rows::InvitationBinding {
                    namespace: rows::NamespaceReceipt {
                        group: entry.group,
                        generation: entry.generation,
                        incarnation: entry.incarnation,
                    },
                    member: entry.local.clone(),
                },
            )
            .unwrap(),
    );
    let quota = exact_quota(&db);
    read(&db).inspect().unwrap();
    for (revision, kind, lifecycle) in [
        (
            1,
            TerminalKind::Removed(entry.local.clone()),
            GroupLifecycle::Removed,
        ),
        (2, TerminalKind::Closed, GroupLifecycle::Closed),
    ] {
        let proof = terminal(&entry, kind, revision);
        let r = read(&db);
        current.lifecycle = lifecycle;
        commit(
            &db,
            write::Edit::new(&r)
                .unwrap()
                .put_current(&r, entry.group, &current)
                .unwrap()
                .put_terminal(
                    &r,
                    &rows::TerminalRow {
                        proof,
                        after: None,
                        complete: true,
                    },
                )
                .unwrap(),
        );
        let r = read(&db);
        r.inspect().unwrap();
        assert!(r.header.bytes + r.header.reserved <= quota);
        assert_eq!(r.header.counts.notices, 0);
    }
    assert_eq!(read(&db).header.counts.terminal_obligations, 0);
}

#[test]
fn incoming_offer_funds_close_before_acceptance_and_transfers_without_a_scan() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("catalog")).unwrap();
    create(
        &db,
        CatalogLimits {
            max_row_bytes: 3072,
            ..Default::default()
        },
    );
    let entry = template();
    let mut invite = super::maintenance::incoming(&entry, 33, 88);
    let mut later = invite.clone();
    later.offer.id = [89; 32];
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_invitation(&r, &invite)
            .unwrap()
            .put_invitation(&r, &later)
            .unwrap(),
    );
    let r = read(&db);
    r.inspect().unwrap();
    assert_eq!(r.header.counts.terminal_obligations, 1);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .remove_invitation(&r, invite.offer.id)
            .unwrap(),
    );
    let r = read(&db);
    r.inspect().unwrap();
    let key = keys::terminal_subject(entry.group, &TerminalKind::Closed);
    assert_eq!(
        r.terminal_obligations
            .get(key.as_slice())
            .unwrap()
            .unwrap()
            .value(),
        later.offer.id
    );
    let quota = exact_quota(&db);
    let statement = TerminalStatement {
        version: WIRE_VERSION,
        group: entry.group,
        title: entry.title.clone(),
        owner: later.offer.owner.clone(),
        head: [21; 32],
        revision: 9,
        kind: TerminalKind::Closed,
    };
    let signature = entry
        .identity()
        .unwrap()
        .sign_enrollment(&encode(&statement).unwrap())
        .unwrap();
    let r = read(&db);
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_terminal(
                &r,
                &rows::TerminalRow {
                    proof: Terminal {
                        statement,
                        signature,
                    },
                    after: None,
                    complete: true,
                },
            )
            .unwrap(),
    );
    let r = read(&db);
    r.inspect().unwrap();
    assert!(r.header.bytes + r.header.reserved <= quota);
    assert_eq!(r.header.counts.terminal_obligations, 0);
    invite = later;
    invite.state = InvitationState::Cancelled;
    commit(
        &db,
        write::Edit::new(&r)
            .unwrap()
            .put_invitation(&r, &invite)
            .unwrap()
            .remove_invitation(&r, invite.offer.id)
            .unwrap(),
    );
    read(&db).inspect().unwrap();
}
