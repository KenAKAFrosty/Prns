use super::*;
use prns_groups_core::retry::RetryError;

// Each public step owns one bounded category/namespace. These tiny fixtures
// deliberately drive repeated steps instead of assuming one call drains all.
fn rounds(device: &mut Device<'_>, now: u64) -> Maintenance {
    let mut result = Maintenance::default();
    for _ in 0..32 {
        let step = device.engine.maintain(&mut device.owner, now).unwrap();
        result.retired_requests += step.retired_requests;
        result.retired_invitations += step.retired_invitations;
        result.retired_outputs += step.retired_outputs;
        result.request_retired_before = step.request_retired_before;
    }
    result
}

#[test]
fn receipt_reserve_keeps_last_slot_for_cleanup_that_releases_the_oldest_output() {
    assert!(Enrollment::ordinary_receipt_available(4093, 4096));
    assert!(!Enrollment::ordinary_receipt_available(4094, 4096));
    assert!(Enrollment::cleanup_receipt_available(4094, 4096, false));
    assert!(!Enrollment::cleanup_receipt_available(4095, 4096, false));
    assert!(Enrollment::cleanup_receipt_available(4095, 4096, true));
    assert!(!Enrollment::cleanup_receipt_available(4096, 4096, true));
    assert!(!Enrollment::ordinary_receipt_available(0, 1));
}

#[test]
fn deletion_and_retry_fence_publish_together_across_abort_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let db = Database::create(&path).unwrap();
    let mut a = Device::new(&db, 1);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "durable", NOW)
        .unwrap();
    let fingerprint = hash(b"catalog-only retained input");
    {
        let read = a.engine.read().unwrap();
        let edit = Enrollment::remember(
            &read,
            indexed::Edit::new(&read).unwrap(),
            request(2),
            fingerprint,
            group.0,
        )
        .unwrap();
        a.engine.write(edit.finish().unwrap()).unwrap();
    }
    let later = NOW + RETRY_HORIZON_SECS;
    // Stop at the requests phase without replacing the owning maintenance plan.
    let plan = (0..8)
        .find_map(|_| {
            let plan = a.engine.read().unwrap().maintenance(later).unwrap();
            if plan.result.retired_requests != 0 {
                Some(plan)
            } else {
                a.engine.maintain(&mut a.owner, later).unwrap();
                None
            }
        })
        .unwrap();
    assert_eq!(plan.result.retired_requests, 2);
    assert_eq!(plan.result.request_retired_before, NOW);
    {
        let tx = db.begin_write().unwrap();
        plan.write.stage(&tx, a.identity).unwrap();
        tx.abort().unwrap();
    }
    drop(plan);
    drop(a);
    drop(db);
    let db = Database::open(&path).unwrap();
    let mut a = Device::new(&db, 1);
    {
        let prior = a.engine.read().unwrap();
        assert_eq!(prior.header.counts.requests, 2);
        for id in [request(1), request(2)] {
            let row = prior.requests.get(id.as_slice()).unwrap().unwrap();
            let receipt: indexed::rows::Receipt = serde_json::from_slice(row.value()).unwrap();
            assert!(
                receipt.namespace.is_none(),
                "this fixture uses catalog-only results"
            );
        }
        assert_eq!(prior.header.request_retired_before, 0);
        assert_eq!(
            Enrollment::replay(&prior, request(2), fingerprint).unwrap(),
            Some(group.0)
        );
    }
    rounds(&mut a, later);
    drop(a);
    drop(db);
    let db = Database::open(&path).unwrap();
    let mut a = Device::new(&db, 1);
    // Both catalog-only receipts expire in the same projection as their replay
    // floor. The managed namespace and its admitted group remain independent.
    assert_eq!(a.engine.read().unwrap().header.counts.requests, 0);
    assert!(matches!(
        a.engine
            .create(&mut a.owner, request(2), "expired new input", NOW),
        Err(EnrollmentError::Retry(RetryError::Expired))
    ));
    assert_eq!(a.ready_groups(), vec![group]);
}

#[test]
fn admitted_create_recovery_survives_catalog_receipt_pruning() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("db")).unwrap();
    let mut a = Device::new(&db, 1);
    let group = [7; 32];
    let entry = a
        .engine
        .entry(group, "admitted", Default::default(), None, NOW)
        .unwrap();
    let expected_key = entry.local.key;
    let fingerprint = hash(b"admitted create");
    {
        // Model death after exact catalog admission and before MLS creation.
        // No opaque legacy receipt or migration path is part of schema v5.
        let read = a.engine.read().unwrap();
        let edit = indexed::Edit::new(&read)
            .unwrap()
            .put_current(&read, group, &indexed::rows::Current::pending(&entry))
            .unwrap()
            .put_incarnation(
                &read,
                indexed::keys::incarnation(group, 0),
                &indexed::rows::Incarnation::Live(entry.clone()),
            )
            .unwrap();
        let edit = Enrollment::remember(&read, edit, request(1), fingerprint, group).unwrap();
        a.engine.write(edit.finish().unwrap()).unwrap();
    }
    assert!(
        a.ready_groups().is_empty(),
        "pure summary does not recover a namespace"
    );
    rounds(&mut a, NOW + RETRY_HORIZON_SECS);
    assert_eq!(a.engine.read().unwrap().header.counts.requests, 0);
    assert_eq!(a.ready_groups(), vec![GroupId(group)]);
    assert_eq!(
        a.descriptor(GroupId(group)).identity.binding().signing_key,
        expected_key
    );
    assert!(matches!(
        a.engine.create(&mut a.owner, request(1), "admitted", NOW),
        Err(EnrollmentError::Retry(RetryError::Expired))
    ));
    assert!(matches!(
        a.engine.create(&mut a.owner, [1; 32], "admitted", NOW),
        Err(EnrollmentError::Retry(RetryError::InvalidToken))
    ));
}

#[test]
fn unsolicited_inbox_has_separate_peer_and_total_quotas_and_cannot_replay_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("db")).unwrap();
    let mut a = Device::new(&db, 1);
    let group_a = a
        .engine
        .create(&mut a.owner, request(1), "local a", NOW)
        .unwrap();
    let group_b = a
        .engine
        .create(&mut a.owner, request(2), "local b", NOW)
        .unwrap();
    let make_offer = |peer: u8, id: u8| {
        let entry = a
            .engine
            .entry([id; 32], "incoming", Default::default(), None, NOW)
            .unwrap();
        let mut member = entry.local.clone();
        member.rns = [peer; 16];
        Offer {
            profile: crate::GroupProfile::default(),
            id: [id; 32],
            group: [id; 32],
            title: "incoming".into(),
            owner: member,
            recipient: a.identity.into_bytes(),
            created: NOW,
            expires: NOW + INVITATION_LIFETIME_SECS,
        }
    };
    let offers = [
        make_offer(2, 10),
        make_offer(2, 11),
        make_offer(2, 12),
        make_offer(3, 13),
        make_offer(3, 14),
        make_offer(4, 15),
    ];
    for index in [0, 1] {
        let offer = &offers[index];
        a.engine
            .handle(
                &mut a.owner,
                IdentityHash::new(offer.owner.rns),
                &frame(&Frame::Offer(offer.clone())).unwrap(),
                NOW,
            )
            .unwrap();
    }
    // The third offer from one peer is rejected even while total space remains.
    {
        let offer = &offers[2];
        assert!(matches!(
            a.engine.handle(
                &mut a.owner,
                IdentityHash::new(offer.owner.rns),
                &frame(&Frame::Offer(offer.clone())).unwrap(),
                NOW
            ),
            Err(EnrollmentError::Capacity)
        ));
    }
    for index in [3, 4] {
        let offer = &offers[index];
        a.engine
            .handle(
                &mut a.owner,
                IdentityHash::new(offer.owner.rns),
                &frame(&Frame::Offer(offer.clone())).unwrap(),
                NOW,
            )
            .unwrap();
    }
    {
        let offer = &offers[5];
        assert!(matches!(
            a.engine.handle(
                &mut a.owner,
                IdentityHash::new(offer.owner.rns),
                &frame(&Frame::Offer(offer.clone())).unwrap(),
                NOW
            ),
            Err(EnrollmentError::Capacity)
        ));
    }
    // Exact duplicate does not charge another quota slot.
    let old = frame(&Frame::Offer(offers[0].clone())).unwrap();
    a.engine
        .handle(&mut a.owner, IdentityHash::new([2; 16]), &old, NOW)
        .unwrap();
    for id in 20..32 {
        let group = if id < 26 { group_a } else { group_b };
        a.engine
            .invite(
                &mut a.owner,
                request(id),
                group,
                IdentityHash::new([id; 16]),
                NOW,
            )
            .unwrap();
    }
    assert_eq!(a.engine.read().unwrap().header.counts.invitations, 16);
    let retired = rounds(&mut a, NOW + INVITATION_LIFETIME_SECS);
    assert_eq!(retired.retired_invitations, 16);
    assert!(matches!(
        a.engine
            .handle(&mut a.owner, IdentityHash::new([2; 16]), &old, NOW),
        Err(EnrollmentError::Expired)
    ));
    assert_eq!(a.engine.read().unwrap().header.counts.invitations, 0);
}

#[test]
fn enrollment_output_cleanup_preserves_service_facts_and_unowned_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("db")).unwrap();
    let mut a = Device::new(&db, 1);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "ownership", NOW)
        .unwrap();
    let entry = a.engine.read().unwrap().entry(group.0).unwrap().unwrap();
    // A service operation and its application fact occupy the same namespace.
    let operation = a
        .engine
        .adapter(&mut a.owner, &entry, None, |adapter, _| {
            let operation = Enrollment::next(adapter, &entry, a.engine.selection.namespace)?;
            let mut key = [0; 33];
            key[0] = b'o';
            key[1..].copy_from_slice(&operation.0);
            adapter
                .compose(
                    operation,
                    Command::Send {
                        group,
                        body: b"application",
                    },
                    |_, _, _| Ok::<_, EnrollmentError>(Admission::Reject(1)),
                    |tx, _| {
                        tx.put(&key, b"service recovery fact")
                            .map_err(|_| EnrollmentError::Corrupt)
                    },
                )
                .unwrap();
            Ok(operation)
        })
        .unwrap();
    assert_eq!(rounds(&mut a, NOW).retired_outputs, 1);
    a.engine
        .adapter(&mut a.owner, &entry, None, |adapter, snapshot| {
            assert_eq!(adapter.pending_operations().unwrap(), vec![operation]);
            // Prefix finalization discharged Create's receipt, but the service
            // operation and the later cleanup transaction remain behind its fence.
            assert_eq!(
                Enrollment::finalize_prefix(adapter, &entry, &[operation]).unwrap(),
                (2, Some(operation))
            );
            let output = adapter.retry(operation).unwrap().unwrap();
            assert_eq!(output.event, prns_groups_mls::Event::Sent);
            // Defense in depth: a missing service fact is not permission to erase
            // ciphertext. Neither Sent nor an unsupported control counts as proof.
            assert!(
                !Enrollment::output_replay_is_preserved(adapter, &entry, &snapshot, &output)
                    .unwrap()
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(rounds(&mut a, NOW + 1).retired_outputs, 0);
}

#[test]
fn repeated_invitation_cycles_retire_more_than_the_api_receipt_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("db")).unwrap();
    let mut a = Device::new(&db, 1);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "cycles", NOW)
        .unwrap();
    for cycle in 0..12 {
        let now = NOW + cycle * (RETRY_HORIZON_SECS + 1);
        rounds(&mut a, now);
        for id in 2..9 {
            a.engine
                .invite(
                    &mut a.owner,
                    request_at(id, now),
                    group,
                    IdentityHash::new([id; 16]),
                    now,
                )
                .unwrap();
        }
        let catalog = a.engine.read().unwrap();
        assert_eq!(catalog.header.counts.invitations, 7);
        assert!(catalog.header.counts.requests <= 8);
    }
    let now = NOW + 12 * (RETRY_HORIZON_SECS + 1);
    rounds(&mut a, now);
    assert_eq!(
        a.engine.read().unwrap().header.counts.requests,
        0,
        "catalog-only Create and invitation receipts expire without deleting the group"
    );
    assert_eq!(a.engine.read().unwrap().header.counts.invitations, 0);
    assert_eq!(a.ready_groups(), vec![group]);
}

#[test]
fn unfinished_welcome_survives_retirement_and_finished_join_retries_after_catalog_pruning() {
    let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let dbs = [
        Database::create(dirs[0].path().join("a")).unwrap(),
        Database::create(dirs[1].path().join("b")).unwrap(),
    ];
    let (mut a, mut b) = (Device::new(&dbs[0], 1), Device::new(&dbs[1], 2));
    let group = a
        .engine
        .create(&mut a.owner, request(1), "late", NOW)
        .unwrap();
    let invitation = offer(&mut a, &mut b, group, 2);
    b.engine
        .respond(&mut b.owner, request(3), invitation, true, NOW)
        .unwrap();
    let join = b.work("join");
    let exact = a.receive(b.identity, &join, NOW);
    let later = NOW + RETRY_HORIZON_SECS + 1;
    rounds(&mut a, later);
    rounds(&mut b, later);
    assert_eq!(a.engine.read().unwrap().header.counts.invitations, 1);
    assert_eq!(b.engine.read().unwrap().header.counts.invitations, 1);
    assert_eq!(a.receive(b.identity, &join, later), exact);
    b.complete(&join, &exact, later);
    let joined = b.work("joined");
    let ack = a.receive(b.identity, &joined, later);
    b.complete(&joined, &ack, later);
    let prune = later + RETRY_HORIZON_SECS;
    rounds(&mut a, prune);
    rounds(&mut b, prune);
    assert_eq!(a.engine.read().unwrap().header.counts.invitations, 0);
    assert_eq!(b.engine.read().unwrap().header.counts.invitations, 0);
    assert_eq!(a.receive(b.identity, &join, prune), exact);
    assert_eq!(a.receive(b.identity, &joined, prune), ack);
    assert_eq!(b.descriptor(group).readers.len(), 2);
    // Enrollment outputs are redundant once Snapshot owns the exact retry data.
    for device in [&mut a, &mut b] {
        let entry = device
            .engine
            .read()
            .unwrap()
            .entry(group.0)
            .unwrap()
            .unwrap();
        device
            .engine
            .adapter(&mut device.owner, &entry, None, |adapter, _| {
                assert!(adapter.pending_operations().unwrap().is_empty());
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn work_pages_are_read_only_including_empty_continuations_and_future_clock() {
    use prns_groups_store::AtomicBackend;
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("read-only-work")).unwrap();
    let mut a = Device::new(&db, 1);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "work pages", NOW)
        .unwrap();
    for id in 2..=4 {
        a.engine
            .invite(
                &mut a.owner,
                request(id),
                group,
                IdentityHash::new([id; 16]),
                NOW,
            )
            .unwrap();
    }
    let cfg = a.descriptor(group);
    let before = a.engine.read().unwrap().header.fingerprint().unwrap();
    let head = a
        .owner
        .open(cfg.namespace, cfg.incarnation, cfg.store_limits)
        .unwrap()
        .head()
        .unwrap();
    for now in [NOW, NOW + RETRY_HORIZON_SECS] {
        let mut after = None;
        let mut pages = 0;
        let mut items = 0;
        let mut empty_continuations = 0;
        loop {
            let page = a
                .engine
                .pending(
                    &mut a.owner,
                    now,
                    after,
                    crate::PageLimits {
                        max_rows: 1,
                        max_bytes: 2 * 1024 * 1024,
                    },
                )
                .unwrap();
            pages += 1;
            items += page.items.len();
            if page.items.is_empty() && page.next.is_some() {
                empty_continuations += 1;
            }
            assert!(pages <= 15, "exclusive cursor must make bounded progress");
            match page.next {
                Some(next) => {
                    assert!(after.is_none_or(|old| next > old));
                    after = Some(next);
                }
                None => break,
            }
        }
        assert_eq!(pages, 15);
        assert_eq!(items, if now == NOW { 3 } else { 0 });
        assert!(empty_continuations > 0);
        assert_eq!(
            a.engine.read().unwrap().header.fingerprint().unwrap(),
            before
        );
        assert_eq!(
            a.owner
                .open(cfg.namespace, cfg.incarnation, cfg.store_limits)
                .unwrap()
                .head()
                .unwrap(),
            head
        );
    }
    // The explicit owner tick retains the durable clock/admission fence.
    a.engine
        .maintain(&mut a.owner, NOW + RETRY_HORIZON_SECS)
        .unwrap();
    assert_eq!(
        a.engine.read().unwrap().header.invitation_time_floor,
        NOW + RETRY_HORIZON_SECS
    );
    assert_ne!(
        a.engine.read().unwrap().header.fingerprint().unwrap(),
        before
    );
}
