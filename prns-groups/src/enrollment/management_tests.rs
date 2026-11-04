use super::*;

pub(super) fn enroll(
    a: &mut Device<'_>,
    b: &mut Device<'_>,
    group: GroupId,
    token: u8,
) -> [u8; 32] {
    let id = offer(a, b, group, token);
    b.engine
        .respond(&mut b.owner, request(token.wrapping_add(1)), id, true, NOW)
        .unwrap();
    let join = b.work("join");
    let reply = a.receive(b.identity, &join, NOW);
    b.complete(&join, &reply, NOW);
    let joined = b.work("joined");
    let reply = a.receive(b.identity, &joined, NOW);
    b.complete(&joined, &reply, NOW);
    id
}
pub(super) fn remove(a: &mut Device<'_>, b: &mut Device<'_>, group: GroupId, token: u8) {
    let member = b.descriptor(group).identity.binding();
    let head = a.descriptor(group).control.head.0;
    a.engine
        .remove_member(
            &mut a.owner,
            request(token),
            group,
            Member::new(b.identity, member).endpoint().unwrap(),
            head,
            NOW,
        )
        .unwrap();
    let sync = b.work("sync");
    let reply = a.receive(b.identity, &sync, NOW);
    b.complete(&sync, &reply, NOW);
}

#[test]
fn removal_is_owner_ordered_and_former_member_receives_terminal_proof() {
    let directory = tempfile::tempdir().unwrap();
    let dbs = ["a", "b", "c"].map(|name| Database::create(directory.path().join(name)).unwrap());
    let (mut a, mut b, mut c) = (
        Device::new(&dbs[0], 1),
        Device::new(&dbs[1], 2),
        Device::new(&dbs[2], 3),
    );
    let group = a
        .engine
        .create(&mut a.owner, request(1), "management", NOW)
        .unwrap();
    enroll(&mut a, &mut b, group, 2);
    enroll(&mut a, &mut c, group, 4);
    let old_head = a.descriptor(group).control.head.0;
    let bob = Member::new(b.identity, b.descriptor(group).identity.binding())
        .endpoint()
        .unwrap();
    let bob_head = b.descriptor(group).control.head.0;
    assert!(matches!(
        b.engine
            .remove_member(&mut b.owner, request(40), group, bob, bob_head, NOW),
        Err(EnrollmentError::NotOwner)
    ));
    assert!(matches!(
        a.engine
            .remove_member(&mut a.owner, request(41), group, bob, [0; 32], NOW),
        Err(EnrollmentError::StaleMembership)
    ));
    remove(&mut a, &mut b, group, 42);
    assert_eq!(b.descriptor(group).lifecycle, GroupLifecycle::Removed);
    assert_eq!(a.descriptor(group).readers.len(), 2);
    let sync = c.work("sync");
    let reply = a.receive(c.identity, &sync, NOW);
    c.complete(&sync, &reply, NOW);
    assert_eq!(c.descriptor(group).readers.len(), 2);
    assert_eq!(c.descriptor(group).control, a.descriptor(group).control);
    a.engine
        .remove_member(&mut a.owner, request(42), group, bob, old_head, NOW)
        .unwrap();
    assert_eq!(a.descriptor(group).readers.len(), 2);
}

#[test]
fn authenticated_leave_cannot_substitute_another_members_endpoint() {
    let directory = tempfile::tempdir().unwrap();
    let dbs = ["a", "b", "c"].map(|name| Database::create(directory.path().join(name)).unwrap());
    let (mut a, mut b, mut c) = (
        Device::new(&dbs[0], 1),
        Device::new(&dbs[1], 2),
        Device::new(&dbs[2], 3),
    );
    let group = a
        .engine
        .create(&mut a.owner, request(1), "leave auth", NOW)
        .unwrap();
    enroll(&mut a, &mut b, group, 2);
    enroll(&mut a, &mut c, group, 4);
    let ac = a.engine.read().unwrap();
    let bc = b.engine.read().unwrap();
    let cc = c.engine.read().unwrap();
    let mut target = cc.entry(group.0).unwrap().unwrap().local.clone();
    target.rns = b.identity.into_bytes();
    let departure = Departure {
        id: [9; 32],
        group: group.0,
        owner: ac.entry(group.0).unwrap().unwrap().owner.clone(),
        member: target,
        known_head: a.descriptor(group).control.head.0,
    };
    assert!(matches!(
        a.engine.handle(
            &mut a.owner,
            b.identity,
            &frame(&Frame::Leave(departure)).unwrap(),
            NOW
        ),
        Err(EnrollmentError::Authentication)
    ));
    assert_eq!(a.descriptor(group).readers.len(), 3);
    assert_eq!(
        bc.entry(group.0).unwrap().unwrap().local.rns,
        b.identity.into_bytes()
    );
}

#[test]
fn leave_outbox_survives_local_delete_restart_and_lost_owner_reply() {
    let directory = tempfile::tempdir().unwrap();
    let ap = directory.path().join("a");
    let bp = directory.path().join("b");
    let adb = Database::create(&ap).unwrap();
    let bdb = Database::create(&bp).unwrap();
    let mut a = Device::new(&adb, 1);
    let mut b = Device::new(&bdb, 2);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "leave then delete", NOW)
        .unwrap();
    enroll(&mut a, &mut b, group, 2);
    b.engine
        .leave(&mut b.owner, request(40), group, NOW)
        .unwrap();
    assert_eq!(b.descriptor(group).lifecycle, GroupLifecycle::Leaving);
    let leave = b.work("leave");
    b.engine
        .delete_local(&mut b.owner, request(41), group, NOW)
        .unwrap();
    assert!(b.ready_groups().is_empty());
    assert!(
        matches!(b.engine.read().unwrap().incarnation(indexed::keys::incarnation(group.0, 0)).unwrap(),
        Some(indexed::rows::Incarnation::Retired(row)) if row.purged)
    );
    drop(b);
    drop(bdb);
    let bdb = Database::open(&bp).unwrap();
    let mut b = Device::new(&bdb, 2);
    let retry = b.work("leave");
    assert_eq!(retry.frame, leave.frame);
    let lost = a.receive(b.identity, &retry, NOW);
    assert_eq!(a.descriptor(group).readers.len(), 1);
    let repeated = a.receive(b.identity, &retry, NOW);
    assert_eq!(lost, repeated);
    b.complete(&retry, &repeated, NOW);
    assert_eq!(b.engine.read().unwrap().header.counts.departures, 0);
    assert!(b.ready_groups().is_empty());
    // A deliberate new invitation creates a fresh membership; the old Leave
    // remains a terminal query and cannot remove the new incarnation.
    enroll(&mut a, &mut b, group, 50);
    let new_member = b.descriptor(group).identity.binding();
    assert_ne!(
        new_member.endpoint,
        decode::<Frame>(&leave.frame)
            .ok()
            .and_then(|f| if let Frame::Leave(d) = f {
                Some(d.member.endpoint)
            } else {
                None
            })
            .unwrap()
    );
    let old = a.receive(b.identity, &leave, NOW);
    assert_eq!(old, lost);
    assert_eq!(a.descriptor(group).readers.len(), 2);
}

#[test]
fn cancellation_suppresses_late_offer_and_allows_fresh_invitation() {
    let directory = tempfile::tempdir().unwrap();
    let adb = Database::create(directory.path().join("a")).unwrap();
    let bdb = Database::create(directory.path().join("b")).unwrap();
    let mut a = Device::new(&adb, 1);
    let mut b = Device::new(&bdb, 2);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "cancel", NOW)
        .unwrap();
    let id = a
        .engine
        .invite(&mut a.owner, request(2), group, b.identity, NOW)
        .unwrap();
    let delayed = a.work("offer");
    a.engine
        .cancel_invitation(&mut a.owner, request(3), id, NOW)
        .unwrap();
    let cancel = a.work("cancel");
    let ack = b.receive(a.identity, &cancel, NOW);
    a.complete(&cancel, &ack, NOW);
    b.receive(a.identity, &delayed, NOW);
    assert!(b.invitations(NOW).is_empty());
    let fresh = offer(&mut a, &mut b, group, 4);
    b.engine
        .respond(&mut b.owner, request(5), fresh, true, NOW)
        .unwrap();
    let pending = b.work("join");
    a.engine
        .cancel_invitation(&mut a.owner, request(6), fresh, NOW)
        .unwrap();
    let reply = a.receive(b.identity, &pending, NOW);
    assert!(matches!(decode::<Reply>(&reply).unwrap(), Reply::Cancelled));
    b.complete(&pending, &reply, NOW);
    enroll(&mut a, &mut b, group, 7);
    assert_eq!(b.descriptor(group).lifecycle, GroupLifecycle::Active);
    let joined = a
        .invitations(NOW)
        .into_iter()
        .find(|i| i.state == InvitationState::Joined)
        .unwrap();
    assert!(!joined.can_cancel);
    assert!(joined.can_dismiss);
    assert!(matches!(
        a.engine
            .cancel_invitation(&mut a.owner, request(20), joined.id.0, NOW),
        Err(EnrollmentError::AlreadyJoined)
    ));
}

#[test]
fn owner_close_is_terminal_and_notification_survives_local_purge() {
    let directory = tempfile::tempdir().unwrap();
    let adb = Database::create(directory.path().join("a")).unwrap();
    let bdb = Database::create(directory.path().join("b")).unwrap();
    let mut a = Device::new(&adb, 1);
    let mut b = Device::new(&bdb, 2);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "close", NOW)
        .unwrap();
    enroll(&mut a, &mut b, group, 2);
    assert!(matches!(
        a.engine.delete_local(&mut a.owner, request(40), group, NOW),
        Err(EnrollmentError::OwnerMustClose)
    ));
    let head = a.descriptor(group).control.head.0;
    a.engine
        .close(&mut a.owner, request(41), group, head, NOW)
        .unwrap();
    assert_eq!(a.descriptor(group).lifecycle, GroupLifecycle::Closed);
    a.engine
        .delete_local(&mut a.owner, request(42), group, NOW)
        .unwrap();
    // Close fanout is phased work even after local namespace purge. A full
    // bounded maintenance cycle materializes this fixture's one recipient.
    for _ in 0..6 {
        a.engine.maintain(&mut a.owner, NOW).unwrap();
    }
    let terminal = a.work("terminal");
    let ack = b.receive(a.identity, &terminal, NOW);
    a.complete(&terminal, &ack, NOW);
    assert_eq!(b.descriptor(group).lifecycle, GroupLifecycle::Closed);
    b.engine
        .delete_local(&mut b.owner, request(43), group, NOW)
        .unwrap();
    assert!(b
        .engine
        .read()
        .unwrap()
        .terminal_subject(group.0, &TerminalKind::Closed)
        .unwrap()
        .is_some());
    assert!(a.ready_groups().is_empty());
    assert!(a.engine.read().unwrap().entry(group.0).unwrap().is_none());
    // Retained Create replay reports its prior result without recreating state.
    assert_eq!(
        a.engine
            .create(&mut a.owner, request(1), "close", NOW)
            .unwrap(),
        group
    );
    assert!(a.ready_groups().is_empty());
}

#[test]
fn repeated_readmission_preserves_old_namespaces_and_passes_seven_controls() {
    let directory = tempfile::tempdir().unwrap();
    let adb = Database::create(directory.path().join("a")).unwrap();
    let bdb = Database::create(directory.path().join("b")).unwrap();
    let mut a = Device::new(&adb, 1);
    let mut b = Device::new(&bdb, 2);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "churn", NOW)
        .unwrap();
    for i in 0..6u8 {
        enroll(&mut a, &mut b, group, 2 + i * 3);
        remove(&mut a, &mut b, group, 4 + i * 3);
    }
    let catalog = a.engine.read().unwrap();
    let controls = a
        .engine
        .adapter(
            &mut a.owner,
            &catalog.entry(group.0).unwrap().unwrap(),
            None,
            |_, s| Ok(s.control_count),
        )
        .unwrap();
    assert_eq!(controls, 12);
    assert_eq!(b.engine.read().unwrap().header.counts.archived, 5);
    assert_eq!(a.descriptor(group).readers.len(), 1);
    // Cold validation checks all retained signed controls and each old member
    // association, including the recipient's archived namespace generations.
    for device in [&mut a, &mut b] {
        let catalog = device.engine.read().unwrap();
        let latest = catalog.current(group.0).unwrap().unwrap().generation;
        for generation in 0..=latest {
            let entry = match catalog
                .incarnation(indexed::keys::incarnation(group.0, generation))
                .unwrap()
                .unwrap()
            {
                indexed::rows::Incarnation::Live(entry)
                | indexed::rows::Incarnation::Archived(entry) => entry,
                indexed::rows::Incarnation::Retired(_) => panic!("unexpected retired namespace"),
            };
            device
                .engine
                .adapter(&mut device.owner, &entry, None, |adapter, state| {
                    state.inspect(adapter, &entry)
                })
                .unwrap();
            let mut backend = device
                .owner
                .open(
                    StoreNamespace(entry.namespace),
                    StoreIncarnation::new(entry.incarnation).unwrap(),
                    profile::store_limits(),
                )
                .unwrap();
            let mut inspection = state::Inspection::new();
            backend
                .inspect_with(|key, value| inspection.observe(key, value))
                .unwrap();
            inspection.finish().unwrap();
        }
    }
}

#[test]
fn ordinary_catalog_capacity_preserves_remove_close_and_delete_requests() {
    let directory = tempfile::tempdir().unwrap();
    let adb = Database::create(directory.path().join("a")).unwrap();
    let bdb = Database::create(directory.path().join("b")).unwrap();
    let tx = adb.begin_write().unwrap();
    Enrollment::initialize_with_limits_in(
        &tx,
        IdentityHash::new([1; 16]),
        CatalogLimits {
            max_catalog_bytes: 64 * 1024,
            ..Default::default()
        },
    )
    .unwrap();
    tx.commit().unwrap();
    let mut a = Device::new(&adb, 1);
    let mut b = Device::new(&bdb, 2);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "reserved", NOW)
        .unwrap();
    enroll(&mut a, &mut b, group, 2);
    for n in 0u64.. {
        let catalog = a.engine.read().unwrap();
        let mut entropy = [91; 20];
        entropy[..8].copy_from_slice(&n.to_be_bytes());
        let row = indexed::rows::Receipt {
            request: Request {
                id: prns_groups_core::retry::RequestToken::new(NOW, entropy)
                    .unwrap()
                    .into_bytes(),
                fingerprint: [91; 32],
                result: group.0,
            },
            namespace: None,
        };
        match Edit::new(&catalog)
            .unwrap()
            .put_receipt(&catalog, &row)
            .and_then(Edit::finish)
        {
            Ok(write) => a.engine.write(write).unwrap(),
            Err(EnrollmentError::Capacity) => break,
            Err(error) => panic!("unexpected admission: {error}"),
        }
    }
    let before = a.engine.read().unwrap().header.counts.requests;
    assert!(matches!(
        a.engine.invite(
            &mut a.owner,
            request(200),
            group,
            IdentityHash::new([3; 16]),
            NOW
        ),
        Err(EnrollmentError::Capacity)
    ));
    remove(&mut a, &mut b, group, 201);
    let head = a.descriptor(group).control.head.0;
    a.engine
        .close(&mut a.owner, request(202), group, head, NOW)
        .unwrap();
    a.engine
        .delete_local(&mut a.owner, request(203), group, NOW)
        .unwrap();
    assert!(a.ready_groups().is_empty());
    assert_eq!(a.engine.read().unwrap().header.counts.requests, before + 3);
}

#[test]
fn unsolicited_cancellation_quota_cannot_consume_known_invitation_cancellation() {
    let directory = tempfile::tempdir().unwrap();
    let adb = Database::create(directory.path().join("a")).unwrap();
    let bdb = Database::create(directory.path().join("b")).unwrap();
    let mut a = Device::new(&adb, 1);
    let mut b = Device::new(&bdb, 2);
    let attacker = IdentityHash::new([9; 16]);
    for n in 0..MAX_INCOMING_PER_PEER {
        let frame = frame(&Frame::Cancel {
            invitation: [n as u8; 32],
            group: [42; 32],
            expires: NOW + INVITATION_LIFETIME_SECS,
        })
        .unwrap();
        b.engine
            .handle(&mut b.owner, attacker, &frame, NOW)
            .unwrap();
    }
    let frame = frame(&Frame::Cancel {
        invitation: [88; 32],
        group: [42; 32],
        expires: NOW + INVITATION_LIFETIME_SECS,
    })
    .unwrap();
    assert!(matches!(
        b.engine.handle(&mut b.owner, attacker, &frame, NOW),
        Err(EnrollmentError::Capacity)
    ));
    let group = a
        .engine
        .create(&mut a.owner, request(1), "real", NOW)
        .unwrap();
    let id = offer(&mut a, &mut b, group, 2);
    a.engine
        .cancel_invitation(&mut a.owner, request(3), id, NOW)
        .unwrap();
    let cancel = a.work("cancel");
    b.receive(a.identity, &cancel, NOW);
    assert_eq!(b.invitations(NOW)[0].state, InvitationState::Cancelled);
}

#[test]
fn close_reaches_a_pending_acceptance_after_owner_purge() {
    let directory = tempfile::tempdir().unwrap();
    let adb = Database::create(directory.path().join("a")).unwrap();
    let bdb = Database::create(directory.path().join("b")).unwrap();
    let mut a = Device::new(&adb, 1);
    let mut b = Device::new(&bdb, 2);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "pending close", NOW)
        .unwrap();
    let id = offer(&mut a, &mut b, group, 2);
    b.engine
        .respond(&mut b.owner, request(3), id, true, NOW)
        .unwrap();
    let join = b.work("join");
    let head = a.descriptor(group).control.head.0;
    a.engine
        .close(&mut a.owner, request(4), group, head, NOW)
        .unwrap();
    a.engine
        .delete_local(&mut a.owner, request(5), group, NOW)
        .unwrap();
    let response = a.receive(b.identity, &join, NOW);
    assert!(matches!(
        decode::<Reply>(&response).unwrap(),
        Reply::Terminal(_)
    ));
    b.complete(&join, &response, NOW);
    assert!(b.ready_groups().is_empty());
    assert_eq!(b.invitations(NOW)[0].state, InvitationState::Cancelled);
}

#[test]
fn owner_closed_pending_invitation_is_dismissible_and_no_longer_offered() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("owner")).unwrap();
    let mut a = Device::new(&db, 1);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "close offer", NOW)
        .unwrap();
    let id = a
        .engine
        .invite(
            &mut a.owner,
            request(2),
            group,
            IdentityHash::new([2; 16]),
            NOW,
        )
        .unwrap();
    let head = a.descriptor(group).control.head.0;
    a.engine
        .close(&mut a.owner, request(3), group, head, NOW)
        .unwrap();
    let invites = a.invitations(NOW);
    assert_eq!(invites[0].state, InvitationState::Cancelled);
    assert!(!invites[0].can_cancel);
    assert!(invites[0].can_dismiss);
    assert!(a
        .work_matching(|frame| matches!(frame, Frame::Offer(_)))
        .is_none());
    a.engine
        .dismiss_invitation(&mut a.owner, request(4), id, NOW)
        .unwrap();
    assert!(a.invitations(NOW).is_empty());
}

#[cfg(feature = "text")]
fn managed_command(
    database: &Database,
    identity: IdentityHash,
    group: GroupId,
    control: Option<prns_groups_core::ControlReference>,
    command: crate::client::Command,
) -> Result<crate::client::Reply, crate::Error> {
    let mut manager = crate::GroupManager::open_managed(database, identity).unwrap();
    let (reply, mut receiver) = tokio::sync::oneshot::channel();
    manager.execute(crate::Job {
        _input: None,
        command,
        group: Some(group),
        control,
        reply,
        wake: std::sync::Arc::new(tokio::sync::Notify::new()),
    });
    receiver.try_recv().unwrap()
}

#[cfg(feature = "text")]
#[test]
fn committed_send_replays_from_terminal_and_archived_membership_without_republishing() {
    use crate::client::{Command, Reply};
    let directory = tempfile::tempdir().unwrap();
    let adb = Database::create(directory.path().join("a")).unwrap();
    let bdb = Database::create(directory.path().join("b")).unwrap();
    let (mut a, mut b) = (Device::new(&adb, 1), Device::new(&bdb, 2));
    let group = a
        .engine
        .create(&mut a.owner, request(1), "history retry", NOW)
        .unwrap();
    let invitation = enroll(&mut a, &mut b, group, 2);
    let prior = b.descriptor(group);
    let clock = prns_groups_core::ClockReading::Trusted(
        prns_groups_core::ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
    );
    let request = crate::test_support::text(
        group,
        crate::OriginalScope::Control {
            control: prior.control,
            incarnation: prior.incarnation,
        },
        request(30),
        "Retained result".into(),
        clock,
    );
    let Reply::Outcome(crate::Outcome::Published(original)) = managed_command(
        &bdb,
        b.identity,
        group,
        Some(prior.control),
        crate::test_support::command(request.clone()),
    )
    .unwrap() else {
        panic!("missing publication result")
    };
    remove(&mut a, &mut b, group, 40);
    assert!(
        matches!(managed_command(&bdb, b.identity, group, Some(prior.control),
        crate::test_support::command(request.clone())).unwrap(), Reply::Outcome(crate::Outcome::Published(id)) if id == original)
    );
    b.engine
        .dismiss_invitation(&mut b.owner, super::request(41), invitation, NOW)
        .unwrap();
    assert_eq!(
        crate::GroupManager::open_managed(&bdb, b.identity)
            .unwrap()
            .invitation_group(invitation)
            .unwrap(),
        Some(group)
    );
    enroll(&mut a, &mut b, group, 42);
    let current = b.descriptor(group);
    assert_ne!(prior.identity.binding(), current.identity.binding());
    assert!(matches!(managed_command(&bdb, b.identity, group, None,
        crate::test_support::command(request.clone())).unwrap(), Reply::Outcome(crate::Outcome::Published(id)) if id == original));
    let mut changed = request.clone();
    changed.0.intent = crate::Intent::Publish(
        crate::PublicationIntent::text(group, "Changed immutable intent").unwrap(),
    );
    assert!(matches!(
        managed_command(
            &bdb,
            b.identity,
            group,
            None,
            crate::test_support::command(changed)
        ),
        Err(crate::Error::IdempotencyConflict)
    ));
    let mut unsent = request;
    unsent.0.id = crate::RequestId(super::request(31));
    assert!(matches!(
        managed_command(
            &bdb,
            b.identity,
            group,
            Some(prior.control),
            crate::test_support::command(unsent.clone())
        ),
        Err(crate::Error::Enrollment(EnrollmentError::StaleMembership))
    ));
    let Reply::Messages(history) = managed_command(
        &bdb,
        b.identity,
        group,
        None,
        Command::Messages {
            group,
            before: None,
            limit: 100,
            clock,
        },
    )
    .unwrap() else {
        panic!("missing history")
    };
    assert_eq!(history.messages.len(), 1);
    assert_eq!(history.messages[0].id, original);
    let Reply::DeliveryTargets(Some(targets)) = managed_command(
        &bdb,
        b.identity,
        group,
        None,
        Command::DeliveryTargets {
            group,
            incarnation: prior.incarnation,
            record: original,
            after: None,
            limit: crate::PageLimits {
                max_rows: 2,
                max_bytes: 4096,
            },
        },
    )
    .unwrap() else {
        panic!("archived incarnation targets");
    };
    assert_eq!(targets.targets.len(), 1);
    assert_eq!(targets.targets[0].identity, Some(a.identity));
    assert_eq!(
        targets.targets[0].receipt,
        prns_groups_core::ReceiptStatus::Awaiting
    );
    assert_eq!(targets.next, None);

    unsent.0.original_scope = Some(crate::OriginalScope::Control {
        control: current.control,
        incarnation: current.incarnation,
    });
    assert!(
        matches!(managed_command(&bdb, b.identity, group, Some(current.control),
        crate::test_support::command(unsent)).unwrap(), Reply::Outcome(crate::Outcome::Published(id)) if id != original)
    );
    b.engine
        .delete_local(&mut b.owner, super::request(49), group, NOW)
        .unwrap();
    assert_eq!(
        crate::GroupManager::open_managed(&bdb, b.identity)
            .unwrap()
            .invitation_group(invitation)
            .unwrap(),
        Some(group)
    );
}

#[cfg(feature = "text")]
#[test]
fn text_filters_bounded_generic_history_without_losing_older_incarnations() {
    use crate::client::{Command, Reply};
    let directory = tempfile::tempdir().unwrap();
    let adb = Database::create(directory.path().join("a")).unwrap();
    let bdb = Database::create(directory.path().join("b")).unwrap();
    let (mut a, mut b) = (Device::new(&adb, 1), Device::new(&bdb, 2));
    let group = a
        .engine
        .create(&mut a.owner, request(1), "mixed retained history", NOW)
        .unwrap();
    enroll(&mut a, &mut b, group, 2);
    let prior = b.descriptor(group);
    let clock = prns_groups_core::ClockReading::Trusted(
        prns_groups_core::ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
    );
    let Reply::Outcome(crate::Outcome::Published(original)) = managed_command(
        &bdb,
        b.identity,
        group,
        None,
        crate::test_support::command(crate::test_support::text(
            group,
            crate::OriginalScope::Control {
                control: prior.control,
                incarnation: prior.incarnation,
            },
            request(100),
            "original text".into(),
            clock,
        )),
    )
    .unwrap() else {
        panic!("publication")
    };
    for round in 0..5 {
        remove(&mut a, &mut b, group, 20 + round * 4);
        enroll(&mut a, &mut b, group, 21 + round * 4);
        let cfg = b.descriptor(group);
        managed_command(
            &bdb,
            b.identity,
            group,
            None,
            crate::test_support::command((
                crate::Request {
                    id: crate::RequestId(request(110 + round)),
                    original_scope: Some(crate::OriginalScope::Control {
                        control: cfg.control,
                        incarnation: cfg.incarnation,
                    }),
                    intent: crate::Intent::Publish(crate::PublicationIntent {
                        group,
                        namespace: prns_groups_core::NamespaceId(9),
                        schema: prns_groups_core::SchemaId(2),
                        body: vec![0, 255],
                        targets: crate::Targets::Explicit(vec![cfg.peers[0].endpoint]),
                    }),
                },
                crate::ExecutionContext { clock },
            )),
        )
        .unwrap();
    }
    let Reply::Messages(first) = managed_command(
        &bdb,
        b.identity,
        group,
        None,
        Command::Messages {
            group,
            before: None,
            limit: 10,
            clock,
        },
    )
    .unwrap() else {
        panic!("messages")
    };
    assert!(first.messages.is_empty());
    assert_eq!(
        first.visited_heads.len(),
        crate::MAX_HISTORY_NAMESPACES_PER_PAGE
    );
    let next = first.next_before.unwrap();
    let Reply::Messages(tail) = managed_command(
        &bdb,
        b.identity,
        group,
        None,
        Command::Messages {
            group,
            before: Some(next),
            limit: 10,
            clock,
        },
    )
    .unwrap() else {
        panic!("messages")
    };
    assert_eq!(tail.messages.len(), 1);
    assert_eq!(tail.messages[0].id, original);
    assert_eq!(tail.messages[0].position.incarnation, prior.incarnation);
    assert_eq!(tail.messages[0].author, b.identity);
    assert!(tail.messages[0].membership_changed);
    assert!(tail.next_before.is_none());
    let Reply::Records(generic) = managed_command(
        &bdb,
        b.identity,
        group,
        None,
        Command::Records {
            group,
            before: None,
            limit: 10,
            clock,
            selection: crate::RecordSelection::All,
        },
    )
    .unwrap() else {
        panic!("records")
    };
    assert_eq!(
        generic.records.len(),
        crate::MAX_HISTORY_NAMESPACES_PER_PAGE
    );
    assert_eq!(generic.visited_heads, first.visited_heads);
    assert_eq!(generic.next_before, Some(next));
}

#[test]
fn canonical_requests_recover_without_clock_and_bind_the_original_scope() {
    use crate::{ExecutionContext, Intent, OriginalScope, Outcome, Request, RequestId};
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("canonical.redb")).unwrap();
    let identity = IdentityHash::new([1; 16]);
    let mut manager = crate::GroupManager::create_managed(&db, identity).unwrap();
    let trusted = ExecutionContext {
        clock: prns_groups_core::ClockReading::Trusted(
            prns_groups_core::ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
        ),
    };
    let unknown = ExecutionContext {
        clock: prns_groups_core::ClockReading::Unknown,
    };
    let create = Request {
        id: RequestId(request(1)),
        intent: Intent::Create {
            profile: Default::default(),
            title: "canonical".into(),
        },
        original_scope: None,
    };
    let Outcome::Created(group) = manager.execute_request(create.clone(), trusted).unwrap() else {
        panic!("create")
    };
    assert_eq!(
        manager.execute_request(create.clone(), unknown).unwrap(),
        Outcome::Created(group)
    );
    let mut fresh = create.clone();
    fresh.id = RequestId(request(2));
    assert!(matches!(
        manager.execute_request(fresh, unknown),
        Err(crate::Error::Enrollment(EnrollmentError::Clock))
    ));
    let mut conflict = create;
    conflict.intent = Intent::Create {
        profile: Default::default(),
        title: "changed".into(),
    };
    assert!(matches!(
        manager.execute_request(conflict, unknown),
        Err(crate::Error::Enrollment(EnrollmentError::RequestConflict))
    ));
    let scope = manager.group_scope(group).unwrap();
    let invite = Request {
        id: RequestId(request(3)),
        intent: Intent::Invite {
            group,
            recipient: IdentityHash::new([2; 16]),
        },
        original_scope: Some(scope),
    };
    let expected = manager.execute_request(invite.clone(), trusted).unwrap();
    assert_eq!(
        manager.execute_request(invite.clone(), unknown).unwrap(),
        expected
    );
    let OriginalScope::Control {
        control: mut changed,
        incarnation,
    } = scope
    else {
        panic!("joined scope")
    };
    changed.era.0 .0[0] ^= 1;
    let mut conflict = invite.clone();
    conflict.original_scope = Some(OriginalScope::Control {
        control: changed,
        incarnation,
    });
    assert!(matches!(
        manager.execute_request(conflict.clone(), unknown),
        Err(crate::Error::Enrollment(EnrollmentError::RequestConflict))
    ));
    conflict.id = RequestId(request(4));
    assert!(matches!(
        manager.execute_request(conflict, trusted),
        Err(crate::Error::Enrollment(EnrollmentError::StaleMembership))
    ));
}

#[test]
fn unjoined_scope_read_is_pure_and_old_delete_cannot_remove_readmission() {
    use crate::{ExecutionContext, Intent, OriginalScope, Outcome, Request, RequestId};
    let dir = tempfile::tempdir().unwrap();
    let adb = Database::create(dir.path().join("owner")).unwrap();
    let bdb = Database::create(dir.path().join("member")).unwrap();
    let (mut a, mut b) = (Device::new(&adb, 1), Device::new(&bdb, 2));
    let group = a
        .engine
        .create(&mut a.owner, request(1), "pending", NOW)
        .unwrap();
    let invitation = offer(&mut a, &mut b, group, 2);
    b.engine
        .respond(&mut b.owner, request(3), invitation, true, NOW)
        .unwrap();
    let before = b.engine.read().unwrap().header.fingerprint().unwrap();
    let original = b.engine.group_scope(&mut b.owner, group).unwrap();
    assert!(matches!(original, OriginalScope::Unjoined(_)));
    assert_eq!(
        before,
        b.engine.read().unwrap().header.fingerprint().unwrap()
    );
    let trusted = ExecutionContext {
        clock: prns_groups_core::ClockReading::Trusted(
            prns_groups_core::ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
        ),
    };
    let unknown = ExecutionContext {
        clock: prns_groups_core::ClockReading::Unknown,
    };
    let delete = Request {
        id: RequestId(request(40)),
        intent: Intent::DeleteLocal { group },
        original_scope: Some(original),
    };
    b.engine
        .execute_request(&mut b.owner, delete.clone(), trusted)
        .unwrap();
    assert!(b
        .engine
        .invitation(crate::InvitationId(invitation), NOW)
        .unwrap()
        .is_none());
    let mut unsent = delete.clone();
    unsent.id = RequestId(request(41));
    a.engine
        .cancel_invitation(&mut a.owner, request(42), invitation, NOW)
        .unwrap();
    let second = offer(&mut a, &mut b, group, 43);
    b.engine
        .respond(&mut b.owner, request(44), second, true, NOW)
        .unwrap();
    let current = b.engine.group_scope(&mut b.owner, group).unwrap();
    assert_ne!(original, current);
    assert_eq!(
        b.engine
            .execute_request(&mut b.owner, delete, unknown)
            .unwrap(),
        Outcome::Saved
    );
    assert_eq!(b.engine.group_scope(&mut b.owner, group).unwrap(), current);
    assert!(matches!(
        b.engine.execute_request(&mut b.owner, unsent, trusted),
        Err(EnrollmentError::StaleMembership)
    ));
    let fresh = Request {
        id: RequestId(request(45)),
        intent: Intent::DeleteLocal { group },
        original_scope: Some(current),
    };
    assert!(matches!(
        b.engine
            .execute_request(&mut b.owner, fresh.clone(), unknown),
        Err(EnrollmentError::Clock)
    ));
    assert_eq!(
        b.engine
            .execute_request(&mut b.owner, fresh.clone(), trusted)
            .unwrap(),
        Outcome::Saved
    );
    assert_eq!(
        b.engine
            .execute_request(&mut b.owner, fresh, unknown)
            .unwrap(),
        Outcome::Saved
    );
}

#[test]
fn invitation_acceptance_uses_the_entire_trusted_execution_interval() {
    use crate::{ExecutionContext, Intent, InvitationId, Request, RequestId};
    let dir = tempfile::tempdir().unwrap();
    let adb = Database::create(dir.path().join("owner")).unwrap();
    let bdb = Database::create(dir.path().join("member")).unwrap();
    let (mut a, mut b) = (Device::new(&adb, 1), Device::new(&bdb, 2));
    let group = a
        .engine
        .create(&mut a.owner, request(1), "interval", NOW)
        .unwrap();
    let invitation = offer(&mut a, &mut b, group, 2);
    let request = Request {
        id: RequestId(request(3)),
        intent: Intent::Respond {
            invitation: InvitationId(invitation),
            accept: true,
        },
        original_scope: None,
    };
    let uncertain = ExecutionContext {
        clock: prns_groups_core::ClockReading::Trusted(
            prns_groups_core::ClockInterval::new(
                Timestamp(NOW),
                Timestamp(NOW + INVITATION_LIFETIME_SECS),
            )
            .unwrap(),
        ),
    };
    assert!(matches!(
        b.engine
            .execute_request(&mut b.owner, request.clone(), uncertain),
        Err(EnrollmentError::Expired)
    ));
    assert!(b.engine.read().unwrap().entry(group.0).unwrap().is_none());
    let trusted = ExecutionContext {
        clock: prns_groups_core::ClockReading::Trusted(
            prns_groups_core::ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
        ),
    };
    b.engine
        .execute_request(&mut b.owner, request.clone(), trusted)
        .unwrap();
    b.engine
        .execute_request(
            &mut b.owner,
            request,
            ExecutionContext {
                clock: prns_groups_core::ClockReading::Unknown,
            },
        )
        .unwrap();
}

#[test]
fn exact_receipt_inspection_is_pure_even_when_admitted_followup_cannot_finish() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("binding");
    let db = Database::create(&path).unwrap();
    let mut device = Device::new(&db, 1);
    let input = crate::Request {
        id: crate::RequestId(request(1)),
        intent: crate::Intent::Create {
            profile: Default::default(),
            title: "accepted before namespace".into(),
        },
        original_scope: None,
    };
    assert!(!device
        .engine
        .has_retained_request(&mut device.owner, &input)
        .unwrap());
    let group = GroupId([87; 32]);
    let mut entry = device
        .engine
        .entry(
            group.0,
            "accepted before namespace",
            Default::default(),
            None,
            NOW,
        )
        .unwrap();
    let namespace = StoreNamespace(entry.namespace);
    let incarnation = StoreIncarnation::new(entry.incarnation).unwrap();
    // A durable admitted intent remains evidence even if its later namespace
    // initialization cannot reconstruct the damaged private-key material.
    entry.secret.clear();
    let catalog = device.engine.read().unwrap();
    let edit = Edit::new(&catalog)
        .unwrap()
        .put_current(&catalog, group.0, &indexed::rows::Current::pending(&entry))
        .unwrap()
        .put_incarnation(
            &catalog,
            indexed::keys::incarnation(group.0, entry.generation),
            &indexed::rows::Incarnation::Live(entry),
        )
        .unwrap();
    let edit = Enrollment::remember(
        &catalog,
        edit,
        input.id.0,
        device.engine.request_fingerprint(&input).unwrap(),
        group.0,
    )
    .unwrap();
    device.engine.write(edit.finish().unwrap()).unwrap();
    let before = hash(&std::fs::read(&path).unwrap());
    assert!(device
        .engine
        .has_retained_request(&mut device.owner, &input)
        .unwrap());
    assert_eq!(hash(&std::fs::read(&path).unwrap()), before);
    assert!(matches!(
        device
            .owner
            .open(namespace, incarnation, profile::store_limits()),
        Err(RedbError::MissingNamespace)
    ));
    assert!(device
        .engine
        .recover_request(&mut device.owner, input.clone())
        .is_err());
    assert!(device
        .engine
        .has_retained_request(&mut device.owner, &input)
        .unwrap());
    let mut changed = input;
    changed.intent = crate::Intent::Create {
        profile: Default::default(),
        title: "different".into(),
    };
    assert!(matches!(
        device
            .engine
            .has_retained_request(&mut device.owner, &changed),
        Err(EnrollmentError::RequestConflict)
    ));
}

#[test]
fn exact_receipt_inspection_uses_atomically_projected_namespace_result() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("binding");
    let db = Database::create(&path).unwrap();
    let mut device = Device::new(&db, 1);
    let group = device
        .engine
        .create(&mut device.owner, request(1), "namespace receipt", NOW)
        .unwrap();
    let descriptor = device.descriptor(group);
    let control = descriptor.control;
    let incarnation = descriptor.incarnation;
    let input = crate::Request {
        id: crate::RequestId(request(2)),
        intent: crate::Intent::Close { group },
        original_scope: Some(crate::OriginalScope::Control {
            control,
            incarnation,
        }),
    };
    device
        .engine
        .execute_request(
            &mut device.owner,
            input.clone(),
            crate::ExecutionContext {
                clock: prns_groups_core::ClockReading::Trusted(
                    prns_groups_core::ClockInterval::new(
                        prns_groups_core::Timestamp(NOW),
                        prns_groups_core::Timestamp(NOW),
                    )
                    .unwrap(),
                ),
            },
        )
        .unwrap();
    let catalog = device.engine.read().unwrap();
    let receipt = catalog
        .receipt(
            input.id.0,
            device.engine.request_fingerprint(&input).unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.namespace.unwrap().incarnation,
        *incarnation.as_bytes()
    );
    let before = hash(&std::fs::read(&path).unwrap());
    assert!(device
        .engine
        .has_retained_request(&mut device.owner, &input)
        .unwrap());
    assert_eq!(hash(&std::fs::read(&path).unwrap()), before);
    assert!(device
        .engine
        .read()
        .unwrap()
        .receipt(
            input.id.0,
            device.engine.request_fingerprint(&input).unwrap()
        )
        .unwrap()
        .is_some());
    let mut changed = input;
    changed.original_scope = Some(crate::OriginalScope::Control {
        control: prns_groups_core::ControlReference {
            head: prns_groups_core::ControlId([91; 32]),
            era: control.era,
        },
        incarnation,
    });
    assert!(matches!(
        device
            .engine
            .has_retained_request(&mut device.owner, &changed),
        Err(EnrollmentError::RequestConflict)
    ));
}

#[cfg(feature = "archive")]
#[test]
fn full_archive_rejects_fresh_retained_publication_without_blocking_close_or_delete() {
    use crate::{
        admission::{Access, Authorization, Settlement},
        retained::State,
        ExecutionContext, GroupManager, Intent, Outcome, PublicationIntent, RequestId, Targets,
    };
    struct Gate;
    impl crate::admission::CarrierRecovery for Gate {}
    impl Authorization for Gate {
        type Permit = Access;
        fn acquire(&mut self, access: Access) -> Result<Access, crate::Error> {
            Ok(access)
        }
        fn settle(&mut self, _: Access, _: Settlement) -> Result<(), crate::Error> {
            Ok(())
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let dbs = ["archive-owner", "archive-member"]
        .map(|name| Database::create(directory.path().join(name)).unwrap());
    let (mut a, mut b) = (Device::new(&dbs[0], 1), Device::new(&dbs[1], 2));
    let group = a
        .engine
        .create(&mut a.owner, request(1), "Archive quota", NOW)
        .unwrap();
    enroll(&mut a, &mut b, group, 2);
    let identity = a.identity;
    drop(a);
    drop(b);
    let tx = dbs[0].begin_write().unwrap();
    crate::archive::initialize_in(
        &tx,
        identity,
        crate::archive::Limits {
            max_records: 1,
            max_bytes: 1024 * 1024,
        },
    )
    .unwrap();
    crate::retained::initialize_in(
        &tx,
        identity,
        crate::retained::Limits {
            max_operations: 8,
            max_bytes: 1024 * 1024,
            ..Default::default()
        },
    )
    .unwrap();
    tx.commit().unwrap();
    let mut manager = GroupManager::open_managed(&dbs[0], identity).unwrap();
    let context = ExecutionContext {
        clock: prns_groups_core::ClockReading::Trusted(
            prns_groups_core::ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
        ),
    };
    let intent = Intent::Publish(PublicationIntent {
        group,
        namespace: prns_groups_core::NamespaceId(42),
        schema: prns_groups_core::SchemaId(7),
        targets: Targets::OtherMembers,
        body: b"archive budget".to_vec(),
    });
    let id = |n| RequestId::from(prns_groups_core::retry::RequestToken::new(NOW, [n; 20]).unwrap());
    let mut gate = Gate;
    assert!(matches!(
        manager
            .submit_retained(id(40), intent.clone(), context, &mut gate)
            .unwrap()
            .state,
        State::Completed(Outcome::Published(_))
    ));
    let rejected = manager
        .submit_retained(id(41), intent, context, &mut gate)
        .unwrap();
    assert_eq!(
        rejected.state,
        State::Rejected(crate::RejectionReason::StorageCapacity)
    );
    assert!(matches!(
        manager
            .submit_retained(id(42), Intent::Close { group }, context, &mut gate)
            .unwrap()
            .state,
        State::Completed(Outcome::Saved)
    ));
    assert!(matches!(
        manager
            .submit_retained(id(43), Intent::DeleteLocal { group }, context, &mut gate)
            .unwrap()
            .state,
        State::Completed(Outcome::Saved)
    ));
    assert_eq!(
        crate::archive::usage_in(&dbs[0].begin_read().unwrap(), identity)
            .unwrap()
            .unwrap()
            .records,
        0
    );
    assert_eq!(
        manager.retained_operation(id(41)).unwrap().unwrap(),
        rejected
    );
}

#[test]
fn managed_service_rejects_stale_roster_reserve_and_missing_selection() {
    let directory = tempfile::tempdir().unwrap();
    let adb = Database::create(directory.path().join("a")).unwrap();
    let bdb = Database::create(directory.path().join("b")).unwrap();
    let mut a = Device::new(&adb, 1);
    let mut b = Device::new(&bdb, 2);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "reserve fence", NOW)
        .unwrap();
    let stale = a.descriptor(group);
    enroll(&mut a, &mut b, group, 2);
    assert!(matches!(
        crate::session::execute_with_owner(
            &adb,
            &mut a.owner,
            &stale,
            crate::client::Command::Snapshot
        ),
        Err(crate::Error::Enrollment(EnrollmentError::StaleMembership))
    ));
    let mut current = a.descriptor(group);
    assert!(current.cleanup_reserve.unwrap().bytes > stale.cleanup_reserve.unwrap().bytes);
    assert!(crate::session::execute_with_owner(
        &adb,
        &mut a.owner,
        &current,
        crate::client::Command::Snapshot
    )
    .is_ok());
    current.cleanup_reserve = None;
    assert!(matches!(
        crate::session::execute_with_owner(
            &adb,
            &mut a.owner,
            &current,
            crate::client::Command::Snapshot
        ),
        Err(crate::Error::Configuration)
    ));
}

#[test]
fn exact_invitation_query_is_pure_independent_of_pages_and_hides_dismissed_rows() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("exact-invitation")).unwrap();
    let mut owner = Device::new(&db, 1);
    let group = owner
        .engine
        .create(&mut owner.owner, request(1), "query", NOW)
        .unwrap();
    let mut ids = [2u8, 3].map(|n| {
        owner
            .engine
            .invite(
                &mut owner.owner,
                request(n),
                group,
                IdentityHash::new([n; 16]),
                NOW,
            )
            .unwrap()
    });
    ids.sort();
    let limits = crate::PageLimits {
        max_rows: 1,
        max_bytes: 65_536,
    };
    let page = owner.engine.invitations_page(NOW, None, limits).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_ne!(page.items[0].id.0, ids[1]);
    let revision = owner.engine.read().unwrap().header.revision;
    let selected = owner
        .engine
        .invitation(crate::InvitationId(ids[1]), NOW)
        .unwrap()
        .unwrap();
    assert_eq!(selected.id.0, ids[1]);
    assert_eq!(selected.state, InvitationState::Pending);
    assert_eq!(
        owner
            .engine
            .invitation(selected.id, selected.expires_at_secs)
            .unwrap()
            .unwrap()
            .state,
        InvitationState::Expired
    );
    // The expiry projection neither performs cleanup nor advances the time floor.
    assert_eq!(
        owner
            .engine
            .invitation(selected.id, NOW)
            .unwrap()
            .unwrap()
            .state,
        InvitationState::Pending
    );
    assert!(owner
        .engine
        .invitation(crate::InvitationId([255; 32]), NOW)
        .unwrap()
        .is_none());
    assert_eq!(owner.engine.read().unwrap().header.revision, revision);
    owner
        .engine
        .cancel_invitation(&mut owner.owner, request(4), ids[1], NOW)
        .unwrap();
    assert_eq!(
        owner
            .engine
            .invitation(selected.id, NOW)
            .unwrap()
            .unwrap()
            .state,
        InvitationState::Cancelled
    );
    owner
        .engine
        .dismiss_invitation(&mut owner.owner, request(5), ids[1], NOW)
        .unwrap();
    assert!(owner.engine.invitation(selected.id, NOW).unwrap().is_none());
}
