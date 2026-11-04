use super::*;
use prns_groups_service::{
    ClockInterval, ClockReading, Descriptor, GroupService, Incoming, NamespaceId, Publication,
    PublicationValidity, RecordId, SchemaId,
};

fn clock(now: u64) -> ClockReading {
    ClockReading::Trusted(ClockInterval::new(Timestamp(now), Timestamp(now)).unwrap())
}

fn with_service<T>(
    device: &mut Device<'_>,
    group: GroupId,
    action: impl FnOnce(&mut GroupService<RedbBackend<'_>>) -> T,
) -> T {
    let catalog = device.engine.read().unwrap();
    let entry = Enrollment::find(&catalog, group.0).unwrap();
    device
        .engine
        .owned_adapter(&mut device.owner, &entry, None, |adapter, snapshot| {
            let current = snapshot.current.clone().unwrap();
            let head = prns_groups_core::ControlId(current.head().unwrap());
            let readers = current
                .manifest
                .members
                .iter()
                .map(|member| member.endpoint().unwrap())
                .collect::<Vec<_>>();
            let mut service = GroupService::new_managed(
                adapter,
                Descriptor {
                    group,
                    control: prns_groups_service::ControlReference {
                        head,
                        era: prns_groups_core::AuthorizationEra(head),
                    },
                    readers: &readers,
                },
                entry.profile.service_limits().unwrap(),
            )
            .unwrap();
            Ok(action(&mut service))
        })
        .unwrap()
}

fn next(service: &GroupService<RedbBackend<'_>>, incarnation: StoreIncarnation) -> OperationId {
    OperationTicket::new(incarnation, service.coordinator().head().unwrap().0 + 1)
        .unwrap()
        .operation_id()
}

fn publish(
    device: &mut Device<'_>,
    group: GroupId,
    nonce: u8,
    deadline: u64,
) -> (RecordId, Vec<u8>) {
    let cfg = device.descriptor(group);
    let local = cfg.identity.binding().endpoint;
    let targets = cfg
        .readers
        .iter()
        .copied()
        .filter(|target| target.id.0 != local)
        .collect::<Vec<_>>();
    with_service(device, group, |service| {
        let operation = next(service, cfg.incarnation);
        let result = service
            .publish(
                operation,
                &Publication {
                    nonce: [nonce; 32],
                    namespace: NamespaceId(17),
                    schema: SchemaId(1),
                    validity: PublicationValidity::new(
                        Timestamp(NOW),
                        Timestamp(deadline),
                        profile::service_limits().maximum_window,
                    )
                    .unwrap(),
                    targets: &targets,
                    body: b"generic record before joining",
                },
                clock(NOW),
            )
            .unwrap();
        (
            result.record,
            service.outgoing(operation).unwrap().unwrap().to_vec(),
        )
    })
}

fn receive(device: &mut Device<'_>, group: GroupId, wire: &[u8]) -> Incoming {
    let cfg = device.descriptor(group);
    with_service(device, group, |service| {
        service
            .receive(next(service, cfg.incarnation), wire, clock(NOW))
            .unwrap()
    })
}

fn join(owner: &mut Device<'_>, member: &mut Device<'_>, group: GroupId, nonce: u8) {
    let invitation = offer(owner, member, group, nonce);
    member
        .engine
        .respond(&mut member.owner, request(nonce + 1), invitation, true, NOW)
        .unwrap();
    let work = member.work("join");
    let response = owner.receive(member.identity, &work, NOW);
    member.complete(&work, &response, NOW);
    let work = member.work("joined");
    let response = owner.receive(member.identity, &work, NOW);
    member.complete(&work, &response, NOW);
}

#[test]
fn join_waits_for_post_invitation_delivery_across_restart_and_replays_committed_add() {
    let directory = tempfile::tempdir().unwrap();
    let paths = ["owner", "member", "joining"].map(|name| directory.path().join(name));
    let (group, invitation, work, record, wire, prior_head) = {
        let dbs = paths.each_ref().map(|path| Database::create(path).unwrap());
        let (mut a, mut b, mut c) = (
            Device::new(&dbs[0], 1),
            Device::new(&dbs[1], 2),
            Device::new(&dbs[2], 3),
        );
        let group = a
            .engine
            .create(&mut a.owner, request(1), "safe add", NOW)
            .unwrap();
        join(&mut a, &mut b, group, 2);
        let invitation = offer(&mut a, &mut c, group, 4);
        // The invitation itself cannot reserve a permanently drained send queue.
        let (record, wire) = publish(&mut a, group, 31, NOW + 600);
        c.engine
            .respond(&mut c.owner, request(5), invitation, true, NOW)
            .unwrap();
        let work = c.work("join");
        let prior_head = a.descriptor(group).control.head;
        let reply = a.receive(c.identity, &work, NOW);
        assert!(matches!(
            decode::<Reply>(&reply).unwrap(),
            Reply::AwaitingDelivery
        ));
        assert_eq!(a.descriptor(group).control.head, prior_head);
        assert_eq!(a.descriptor(group).readers.len(), 2);
        assert!(
            a.invitations(NOW)
                .iter()
                .find(|invite| invite.id.0 == invitation)
                .unwrap()
                .waiting_for_delivery
        );
        // Lose the reply and close every store before retrying the same Join.
        (group, invitation, work, record, wire, prior_head)
    };
    let welcome = {
        let dbs = paths.each_ref().map(|path| Database::open(path).unwrap());
        let (mut a, mut b, mut c) = (
            Device::new(&dbs[0], 1),
            Device::new(&dbs[1], 2),
            Device::new(&dbs[2], 3),
        );
        assert_eq!(c.work("join").frame, work.frame);
        let reply = a.receive(c.identity, &work, NOW);
        assert_eq!(c.complete(&work, &reply, NOW), CompletionActivity::Idle);
        assert_eq!(c.complete(&work, &reply, NOW), CompletionActivity::Idle);
        assert!(c.invitations(NOW)[0].waiting_for_delivery);
        assert_eq!(a.descriptor(group).control.head, prior_head);
        assert_eq!(c.work("join").frame, work.frame);
        let mut changed: Frame = decode(&work.frame).unwrap();
        if let Frame::Join { package, .. } = &mut changed {
            package.push(0);
        }
        assert!(matches!(
            a.engine
                .handle(&mut a.owner, c.identity, &frame(&changed).unwrap(), NOW),
            Err(EnrollmentError::Conflict)
        ));
        assert_eq!(receive(&mut b, group, &wire), Incoming::Record(record));
        // Receipt loss continues blocking Add although the destination has it.
        assert!(matches!(
            decode::<Reply>(&a.receive(c.identity, &work, NOW)).unwrap(),
            Reply::AwaitingDelivery
        ));
        let cfg = b.descriptor(group);
        let receipt = with_service(&mut b, group, |service| {
            let operation = next(service, cfg.incarnation);
            service.emit_receipt(operation, record).unwrap();
            service.outgoing(operation).unwrap().unwrap().to_vec()
        });
        receive(&mut a, group, &receipt);
        let welcome = a.receive(c.identity, &work, NOW);
        assert!(matches!(
            decode::<Reply>(&welcome).unwrap(),
            Reply::Welcome { .. }
        ));
        assert_eq!(a.descriptor(group).readers.len(), 3);
        assert!(
            !a.invitations(NOW)
                .iter()
                .find(|invite| invite.id.0 == invitation)
                .unwrap()
                .waiting_for_delivery
        );
        // Fresh pending work in the new era cannot block an already committed
        // Welcome; another crash loses that Welcome but not its exact result.
        publish(&mut a, group, 32, NOW + 600);
        welcome
    };
    let dbs = paths.each_ref().map(|path| Database::open(path).unwrap());
    let (mut a, mut c) = (Device::new(&dbs[0], 1), Device::new(&dbs[2], 3));
    assert_eq!(a.receive(c.identity, &work, NOW), welcome);
    assert_eq!(
        c.complete(&work, &welcome, NOW),
        CompletionActivity::Progress
    );
    assert_eq!(c.descriptor(group).readers.len(), 3);
    assert!(!c.invitations(NOW)[0].waiting_for_delivery);
    with_service(&mut c, group, |service| {
        assert!(service.record(record).unwrap().is_none())
    });
}

#[test]
fn definite_expiry_allows_waiting_add_without_fabricating_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let paths = ["owner", "member", "joining"].map(|name| directory.path().join(name));
    let dbs = paths.each_ref().map(|path| Database::create(path).unwrap());
    let (mut a, mut b, mut c) = (
        Device::new(&dbs[0], 1),
        Device::new(&dbs[1], 2),
        Device::new(&dbs[2], 3),
    );
    let group = a
        .engine
        .create(&mut a.owner, request(1), "expiry", NOW)
        .unwrap();
    join(&mut a, &mut b, group, 2);
    let invitation = offer(&mut a, &mut c, group, 4);
    let (record, _) = publish(&mut a, group, 41, NOW + 60);
    c.engine
        .respond(&mut c.owner, request(5), invitation, true, NOW)
        .unwrap();
    let work = c.work("join");
    assert!(matches!(
        decode::<Reply>(&a.receive(c.identity, &work, NOW + 59)).unwrap(),
        Reply::AwaitingDelivery
    ));
    let welcome = a.receive(c.identity, &work, NOW + 60);
    assert!(matches!(
        decode::<Reply>(&welcome).unwrap(),
        Reply::Welcome { .. }
    ));
    with_service(&mut a, group, |service| {
        assert_eq!(
            service.progress(record, clock(NOW + 60)).unwrap().received,
            0
        );
        assert!(!service.in_current_era(record).unwrap());
    });
}

#[test]
fn deferral_does_not_extend_invitation_expiry_or_leave_a_waiting_projection() {
    let directory = tempfile::tempdir().unwrap();
    let paths = ["owner", "member", "joining"].map(|name| directory.path().join(name));
    let dbs = paths.each_ref().map(|path| Database::create(path).unwrap());
    let (mut a, mut b, mut c) = (
        Device::new(&dbs[0], 1),
        Device::new(&dbs[1], 2),
        Device::new(&dbs[2], 3),
    );
    let group = a
        .engine
        .create(&mut a.owner, request(1), "deadline", NOW)
        .unwrap();
    join(&mut a, &mut b, group, 2);
    let invitation = offer(&mut a, &mut c, group, 4);
    publish(&mut a, group, 51, NOW + INVITATION_LIFETIME_SECS + 600);
    c.engine
        .respond(&mut c.owner, request(5), invitation, true, NOW)
        .unwrap();
    let work = c.work("join");
    let prior = a.descriptor(group).control.head;
    let reply = a.receive(c.identity, &work, NOW);
    assert_eq!(c.complete(&work, &reply, NOW), CompletionActivity::Idle);
    let deadline = NOW + INVITATION_LIFETIME_SECS;
    let expired = a.receive(c.identity, &work, deadline);
    assert!(matches!(decode::<Reply>(&expired).unwrap(), Reply::Expired));
    assert_eq!(
        c.complete(&work, &expired, deadline),
        CompletionActivity::Progress
    );
    let incoming = c.invitations(deadline);
    assert_eq!(incoming[0].state, InvitationState::Expired);
    assert!(!incoming[0].waiting_for_delivery);
    assert!(
        !c.invitations(NOW)[0].waiting_for_delivery,
        "a rolled-back clock cannot restore waiting after terminal expiry"
    );
    // Returning Expired does not run unrelated catalog retirement. Explicit
    // bounded maintenance eventually refunds the owner's unused reservation.
    for _ in 0..8 {
        a.engine.maintain(&mut a.owner, deadline).unwrap();
    }
    assert!(a
        .invitations(deadline)
        .iter()
        .all(|invite| invite.id.0 != invitation));
    assert_eq!(a.descriptor(group).control.head, prior);
    with_service(&mut a, group, |service| {
        assert!(service
            .has_pending_authored_deliveries(clock(deadline))
            .unwrap())
    });
}

#[test]
fn delivery_target_read_preserves_received_and_removed_with_historical_identity() {
    use crate::client::{Command, Reply as ClientReply};
    use prns_groups_core::{ReceiptStatus, RemovalStatus};
    let directory = tempfile::tempdir().unwrap();
    let adb = Database::create(directory.path().join("owner")).unwrap();
    let bdb = Database::create(directory.path().join("member")).unwrap();
    let (mut a, mut b) = (Device::new(&adb, 1), Device::new(&bdb, 2));
    let group = a
        .engine
        .create(&mut a.owner, request(1), "Target facts", NOW)
        .unwrap();
    join(&mut a, &mut b, group, 2);
    let cfg = a.descriptor(group);
    let peer = cfg.peers[0];
    let (record, wire) = publish(&mut a, group, 31, NOW + 600);
    assert_eq!(receive(&mut b, group, &wire), Incoming::Record(record));
    let bcfg = b.descriptor(group);
    let receipt = with_service(&mut b, group, |service| {
        let operation = next(service, bcfg.incarnation);
        service.emit_receipt(operation, record).unwrap();
        service.outgoing(operation).unwrap().unwrap().to_vec()
    });
    receive(&mut a, group, &receipt);
    super::management_tests::remove(&mut a, &mut b, group, 40);
    assert!(a.descriptor(group).peers.is_empty());
    let mut manager = crate::GroupManager::open_managed(&adb, a.identity).unwrap();
    let (reply, mut receiver) = tokio::sync::oneshot::channel();
    manager.execute(crate::Job {
        command: Command::DeliveryTargets {
            group,
            incarnation: cfg.incarnation,
            record,
            after: None,
            limit: crate::PageLimits {
                max_rows: 1,
                max_bytes: 4096,
            },
        },
        group: Some(group),
        control: None,
        reply,
        _input: None,
        wake: std::sync::Arc::new(tokio::sync::Notify::new()),
    });
    let ClientReply::DeliveryTargets(Some(page)) = receiver.try_recv().unwrap().unwrap() else {
        panic!("target page");
    };
    assert_eq!(
        page,
        crate::DeliveryTargetPage {
            targets: vec![crate::DeliveryTarget {
                endpoint: peer.endpoint,
                identity: Some(b.identity),
                receipt: ReceiptStatus::Received,
                removal: RemovalStatus::Removed,
            }],
            next: None
        }
    );
}
