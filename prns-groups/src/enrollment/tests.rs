#![allow(clippy::unwrap_used, clippy::panic)]
// Explicitly end borrowed lane lifetimes before closing/reopening the database.
#![allow(clippy::drop_non_drop)]
use super::*;
#[cfg(feature = "text")]
#[path = "capacity_tests.rs"]
mod capacity_tests;
#[path = "carrier_tests.rs"]
mod carrier_tests;
#[path = "dependency_tests.rs"]
mod dependency_tests;
#[path = "maintenance_tests.rs"]
mod maintenance_tests;
#[path = "management_tests.rs"]
mod management_tests;
#[path = "membership_tests.rs"]
mod membership_tests;
#[path = "profile_tests.rs"]
mod profile_tests;
const NOW: u64 = 1_800_000_000;
fn request(id: u8) -> [u8; 32] {
    request_at(id, NOW)
}
fn request_at(id: u8, now: u64) -> [u8; 32] {
    prns_groups_core::retry::RequestToken::new(now, [id; 20])
        .unwrap()
        .into_bytes()
}
struct Device<'db> {
    engine: Enrollment<'db>,
    owner: RedbStoreOwner<'db>,
    identity: IdentityHash,
}
impl<'db> Device<'db> {
    fn new(db: &'db Database, id: u8) -> Self {
        let identity = IdentityHash::new([id; 16]);
        Self {
            engine: match Enrollment::open_existing(db, identity) {
                Ok(engine) => engine,
                Err(EnrollmentError::MissingCatalog) => {
                    Enrollment::create_new(db, identity).unwrap()
                }
                Err(error) => panic!("fixture catalog failed: {error}"),
            },
            owner: RedbStoreOwner::new(db),
            identity,
        }
    }
    /// Small-fixture assertions traverse canonical pages; no production
    /// inventory or implicit recovery is hidden behind this helper.
    fn ready_groups(&self) -> Vec<GroupId> {
        let mut after = None;
        let mut groups = Vec::new();
        loop {
            let page = self
                .engine
                .conversation_summaries(
                    after,
                    crate::PageLimits {
                        max_rows: 3,
                        max_bytes: 64 * 1024,
                    },
                )
                .unwrap();
            groups.extend(
                page.items
                    .into_iter()
                    .filter(|row| {
                        row.control.is_some()
                            && (row.lifecycle != GroupLifecycle::Active || row.ready)
                    })
                    .map(|row| row.group),
            );
            match page.next {
                Some(next) => {
                    assert!(after.is_none_or(|after| next > after));
                    after = Some(next);
                }
                None => return groups,
            }
        }
    }
    fn invitations(&self, now: u64) -> Vec<Invitation> {
        let mut after = None;
        let mut invitations = Vec::new();
        loop {
            let page = self
                .engine
                .invitations_page(
                    now,
                    after,
                    crate::PageLimits {
                        max_rows: 3,
                        max_bytes: 64 * 1024,
                    },
                )
                .unwrap();
            invitations.extend(page.items);
            match page.next {
                Some(next) => {
                    assert!(after.is_none_or(|after| next > after));
                    after = Some(next);
                }
                None => return invitations,
            }
        }
    }
    fn work_matching(&mut self, matches: impl Fn(&Frame) -> bool) -> Option<Work> {
        let mut after = None;
        loop {
            let page = self
                .engine
                .pending(
                    &mut self.owner,
                    NOW,
                    after,
                    crate::PageLimits {
                        max_rows: 2,
                        max_bytes: 2 * 1024 * 1024,
                    },
                )
                .unwrap();
            if let Some(work) = page
                .items
                .into_iter()
                .find(|work| matches(&decode::<Frame>(&work.frame).unwrap()))
            {
                return Some(work);
            }
            let next = page.next?;
            assert!(after.is_none_or(|after| next > after));
            after = Some(next);
        }
    }
    fn work(&mut self, kind: &str) -> Work {
        self.work_matching(|frame| match frame {
            Frame::Leave(_) => kind == "leave",
            Frame::Terminal(_) => kind == "terminal",
            Frame::Cancel { .. } => kind == "cancel",
            Frame::Offer(_) => kind == "offer",
            Frame::Join { .. } => kind == "join",
            Frame::Sync { .. } => kind == "sync",
            Frame::Decline { .. } => kind == "decline",
            Frame::Joined { .. } => kind == "joined",
        })
        .unwrap()
    }
    fn receive(&mut self, from: IdentityHash, work: &Work, now: u64) -> Vec<u8> {
        assert_eq!(work.recipient, self.identity);
        self.engine
            .handle(&mut self.owner, from, &work.frame, now)
            .unwrap()
    }
    fn complete(&mut self, work: &Work, reply: &[u8], now: u64) -> CompletionActivity {
        self.engine
            .complete(&mut self.owner, work.id, reply, now)
            .unwrap()
    }
    fn descriptor(&mut self, group: GroupId) -> Provisioned {
        self.engine.provisioned(&mut self.owner, group).unwrap()
    }
}
fn offer(owner: &mut Device<'_>, recipient: &mut Device<'_>, group: GroupId, id: u8) -> [u8; 32] {
    let invitation = owner
        .engine
        .invite(
            &mut owner.owner,
            request(id),
            group,
            recipient.identity,
            NOW,
        )
        .unwrap();
    let work = owner.work("offer");
    let reply = recipient.receive(owner.identity, &work, NOW);
    owner.complete(&work, &reply, NOW);
    invitation
}

#[test]
fn unchanged_control_is_idle_without_writes_but_valid_new_control_is_progress() {
    let directory = tempfile::tempdir().unwrap();
    let paths = ["a", "b", "c"].map(|name| directory.path().join(name));
    let dbs = paths.each_ref().map(|path| Database::create(path).unwrap());
    let (mut a, mut b, mut c) = (
        Device::new(&dbs[0], 1),
        Device::new(&dbs[1], 2),
        Device::new(&dbs[2], 3),
    );
    let group = a
        .engine
        .create(&mut a.owner, request(1), "idle control", NOW)
        .unwrap();
    let invitation = offer(&mut a, &mut b, group, 2);
    b.engine
        .respond(&mut b.owner, request(3), invitation, true, NOW)
        .unwrap();
    let join = b.work("join");
    let response = a.receive(b.identity, &join, NOW);
    assert_eq!(
        b.complete(&join, &response, NOW),
        CompletionActivity::Progress
    );
    let joined = b.work("joined");
    let response = a.receive(b.identity, &joined, NOW);
    assert_eq!(
        b.complete(&joined, &response, NOW),
        CompletionActivity::Progress
    );

    // Drain completed-operation cleanup before observing a steady-state poll.
    let sync = b.work("sync");
    let unchanged = a.receive(b.identity, &sync, NOW);
    assert!(matches!(
        decode::<Reply>(&unchanged).unwrap(),
        Reply::Control(None)
    ));
    let before = hash(&std::fs::read(&paths[1]).unwrap());
    for _ in 0..3 {
        assert_eq!(b.complete(&sync, &unchanged, NOW), CompletionActivity::Idle);
    }
    assert_eq!(hash(&std::fs::read(&paths[1]).unwrap()), before);
    assert!(matches!(
        b.engine
            .complete(&mut b.owner, sync.id, &frame(&Reply::Ack).unwrap(), NOW),
        Err(EnrollmentError::Invalid)
    ));

    let invitation = offer(&mut a, &mut c, group, 4);
    c.engine
        .respond(&mut c.owner, request(5), invitation, true, NOW)
        .unwrap();
    let join = c.work("join");
    let response = a.receive(c.identity, &join, NOW);
    assert_eq!(
        c.complete(&join, &response, NOW),
        CompletionActivity::Progress
    );
    let advanced = a.receive(b.identity, &sync, NOW);
    assert!(matches!(
        decode::<Reply>(&advanced).unwrap(),
        Reply::Control(Some(_))
    ));
    assert_eq!(
        b.complete(&sync, &advanced, NOW),
        CompletionActivity::Progress
    );
    assert_ne!(hash(&std::fs::read(&paths[1]).unwrap()), before);
    assert_eq!(b.descriptor(group).readers.len(), 3);
    // An old completion cannot create fresh work after the head has advanced.
    assert_eq!(b.complete(&sync, &advanced, NOW), CompletionActivity::Idle);
}

#[test]
fn three_peers_lost_welcome_later_add_restart_and_exact_replay() {
    let dirs = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let paths = [
        dirs.0.path().join("a.redb"),
        dirs.1.path().join("b.redb"),
        dirs.2.path().join("c.redb"),
    ];
    let dbs = [
        Database::create(&paths[0]).unwrap(),
        Database::create(&paths[1]).unwrap(),
        Database::create(&paths[2]).unwrap(),
    ];
    let (mut a, mut b, mut c) = (
        Device::new(&dbs[0], 1),
        Device::new(&dbs[1], 2),
        Device::new(&dbs[2], 3),
    );
    let group = a
        .engine
        .create(&mut a.owner, request(1), "Private group", NOW)
        .unwrap();
    assert_eq!(
        a.engine
            .create(&mut a.owner, request(1), "Private group", NOW + 1)
            .unwrap(),
        group
    );
    assert!(matches!(
        a.engine.create(&mut a.owner, request(1), "Changed", NOW),
        Err(EnrollmentError::RequestConflict)
    ));
    let bi = offer(&mut a, &mut b, group, 2);
    b.engine
        .respond(&mut b.owner, request(3), bi, true, NOW)
        .unwrap();
    let bw = b.work("join");
    // A previously deferred response must stop showing "waiting for delivery"
    // after Welcome, even while later controls still hold back publication.
    b.complete(&bw, &frame(&Reply::AwaitingDelivery).unwrap(), NOW);
    let lost_welcome = a.receive(b.identity, &bw, NOW);
    assert_eq!(a.descriptor(group).readers.len(), 2);
    let ci = offer(&mut a, &mut c, group, 4);
    c.engine
        .respond(&mut c.owner, request(5), ci, true, NOW)
        .unwrap();
    let cw = c.work("join");
    let cr = a.receive(c.identity, &cw, NOW);
    c.complete(&cw, &cr, NOW);
    let post_expiry = NOW + INVITATION_LIFETIME_SECS + 1;
    let late = a.receive(b.identity, &bw, post_expiry);
    if let (Reply::Welcome { added: early, .. }, Reply::Welcome { added: later, .. }) = (
        decode::<Reply>(&lost_welcome).unwrap(),
        decode::<Reply>(&late).unwrap(),
    ) {
        assert_eq!(early.welcome, later.welcome);
        assert_eq!(early.signed.manifest.epoch, 1);
    } else {
        panic!("expected welcome")
    }
    assert_eq!(
        a.invitations(NOW)
            .iter()
            .find(|row| row.id.0 == bi)
            .unwrap()
            .state,
        InvitationState::JoinRequested
    );
    drop(a);
    drop(b);
    drop(c);
    drop(dbs);
    let dbs = [
        Database::open(&paths[0]).unwrap(),
        Database::open(&paths[1]).unwrap(),
        Database::open(&paths[2]).unwrap(),
    ];
    let (mut a, mut b, mut c) = (
        Device::new(&dbs[0], 1),
        Device::new(&dbs[1], 2),
        Device::new(&dbs[2], 3),
    );
    assert_eq!(b.work("join").frame, bw.frame);
    b.complete(&bw, &a.receive(b.identity, &bw, post_expiry), post_expiry);
    assert!(b.ready_groups().is_empty());
    assert!(!b.invitations(NOW)[0].waiting_for_delivery);
    assert!(matches!(
        b.engine.provisioned(&mut b.owner, group),
        Err(EnrollmentError::NotJoined)
    ));
    let sync = b.work("sync");
    let result = a.receive(b.identity, &sync, post_expiry);
    b.complete(&sync, &result, post_expiry);
    let ack = b.work("joined");
    let ack_response = a.receive(b.identity, &ack, post_expiry);
    b.complete(&ack, &ack_response, post_expiry);
    assert_eq!(
        a.invitations(post_expiry)
            .iter()
            .find(|row| row.id.0 == bi)
            .unwrap()
            .state,
        InvitationState::Joined
    );
    let ad = a.descriptor(group);
    let bd = b.descriptor(group);
    let cd = c.descriptor(group);
    assert_eq!(ad.readers.len(), 3);
    assert_eq!(ad.readers, bd.readers);
    assert_eq!(bd.readers, cd.readers);
    assert_eq!(ad.control, bd.control);
    assert_eq!(bd.control, cd.control);
    assert_eq!(ad.peers.len(), 2);
    assert!(ad.policy.is_owner_managed());
    assert_eq!(b.invitations(post_expiry)[0].state, InvitationState::Joined);
    // Re-completing old replies after newer state cannot rewind the tree.
    b.complete(&bw, &lost_welcome, post_expiry);
    assert_eq!(b.descriptor(group).control, ad.control);
    assert!(Enrollment::open_existing(&dbs[0], IdentityHash::new([99; 16])).is_err());
}
#[test]
fn authenticated_recipient_key_package_and_signed_roster_are_required() {
    let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let dbs = [
        Database::create(dirs[0].path().join("a")).unwrap(),
        Database::create(dirs[1].path().join("b")).unwrap(),
    ];
    let (mut a, mut b) = (Device::new(&dbs[0], 1), Device::new(&dbs[1], 2));
    let group = a
        .engine
        .create(&mut a.owner, request(1), "auth", NOW)
        .unwrap();
    let id = a
        .engine
        .invite(&mut a.owner, request(2), group, b.identity, NOW)
        .unwrap();
    let ow = a.work("offer");
    assert!(matches!(
        b.engine
            .handle(&mut b.owner, IdentityHash::new([9; 16]), &ow.frame, NOW),
        Err(EnrollmentError::Authentication)
    ));
    assert!(b.invitations(NOW).is_empty());
    let reply = b.receive(a.identity, &ow, NOW);
    a.complete(&ow, &reply, NOW);
    let mut forged_offer: Frame = decode(&ow.frame).unwrap();
    if let Frame::Offer(o) = &mut forged_offer {
        o.owner.key = [9; 32];
    }
    assert!(matches!(
        b.engine.handle(
            &mut b.owner,
            a.identity,
            &frame(&forged_offer).unwrap(),
            NOW
        ),
        Err(EnrollmentError::Conflict)
    ));
    b.engine
        .respond(&mut b.owner, request(3), id, true, NOW)
        .unwrap();
    let join = b.work("join");
    assert!(a
        .engine
        .handle(&mut a.owner, IdentityHash::new([9; 16]), &join.frame, NOW)
        .is_err());
    let mut wrong_claim: Frame = decode(&join.frame).unwrap();
    if let Frame::Join { member, .. } = &mut wrong_claim {
        member.key = [9; 32];
    }
    assert!(a
        .engine
        .handle(&mut a.owner, b.identity, &frame(&wrong_claim).unwrap(), NOW)
        .is_err());
    assert_eq!(a.descriptor(group).readers.len(), 1);
    let good = a.receive(b.identity, &join, NOW);
    let mut forged_tip: Reply = decode(&good).unwrap();
    if let Reply::Welcome { tip, .. } = &mut forged_tip {
        tip.signature[0] ^= 1;
    }
    assert!(matches!(
        b.engine
            .complete(&mut b.owner, join.id, &frame(&forged_tip).unwrap(), NOW),
        Err(EnrollmentError::Authentication)
    ));
    assert!(b.ready_groups().is_empty());
    // A valid owner's signature on a fabricated roster still cannot replace the
    // membership authenticated by the actual MLS Welcome.
    let mut wrong_tree: Reply = decode(&good).unwrap();
    if let Reply::Welcome { added, .. } = &mut wrong_tree {
        let m = added
            .signed
            .manifest
            .members
            .iter_mut()
            .find(|m| m.rns == a.identity.into_bytes())
            .unwrap();
        m.key = [9; 32];
        let entry = a.engine.read().unwrap().entry(group.0).unwrap().unwrap();
        added.signed.signature = entry
            .identity()
            .unwrap()
            .sign_enrollment(&encode(&added.signed.manifest).unwrap())
            .unwrap();
    }
    assert!(b
        .engine
        .complete(&mut b.owner, join.id, &frame(&wrong_tree).unwrap(), NOW)
        .is_err());
    assert!(b.ready_groups().is_empty());
    b.complete(&join, &good, NOW);
    assert_eq!(b.descriptor(group).readers.len(), 2);
    // A changed KeyPackage on a consumed invitation is never a second Add.
    let mut conflict: Frame = decode(&join.frame).unwrap();
    if let Frame::Join { package, .. } = &mut conflict {
        package.push(0);
    }
    assert!(matches!(
        a.engine
            .handle(&mut a.owner, b.identity, &frame(&conflict).unwrap(), NOW),
        Err(EnrollmentError::Conflict)
    ));
    assert_eq!(a.descriptor(group).readers.len(), 2);
}
#[test]
fn accepted_create_and_accept_intents_recover_without_caller_request_id() {
    let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let paths = [dirs[0].path().join("a"), dirs[1].path().join("b")];
    let dbs = [
        Database::create(&paths[0]).unwrap(),
        Database::create(&paths[1]).unwrap(),
    ];
    let (mut a, mut b) = (Device::new(&dbs[0], 1), Device::new(&dbs[1], 2));
    // Simulate death just after the catalog admission, before namespace creation.
    let group = GroupId([19; 32]);
    let entry = a
        .engine
        .entry(group.0, "recover", Default::default(), None, NOW)
        .unwrap();
    let local_key = entry.local.key;
    {
        let catalog = a.engine.read().unwrap();
        let edit = indexed::Edit::new(&catalog)
            .unwrap()
            .put_current(&catalog, group.0, &indexed::rows::Current::pending(&entry))
            .unwrap()
            .put_incarnation(
                &catalog,
                indexed::keys::incarnation(group.0, entry.generation),
                &indexed::rows::Incarnation::Live(entry),
            )
            .unwrap();
        a.engine
            .write(
                Enrollment::remember(
                    &catalog,
                    edit,
                    request(1),
                    hash(&encode(&("create", "recover")).unwrap()),
                    group.0,
                )
                .unwrap()
                .finish()
                .unwrap(),
            )
            .unwrap();
    }
    assert!(
        a.ready_groups().is_empty(),
        "summary reads cannot initialize MLS state"
    );
    for _ in 0..8 {
        a.engine.maintain(&mut a.owner, NOW).unwrap();
        if !a.ready_groups().is_empty() {
            break;
        }
    }
    assert_eq!(a.ready_groups(), vec![group]);
    assert_eq!(
        a.descriptor(group).identity.binding().signing_key,
        local_key
    );
    let invitation = offer(&mut a, &mut b, group, 2);
    // Simulate death after acceptance/identity admission but before KeyPackage.
    let joining_key = {
        let catalog = b.engine.read().unwrap();
        let mut invite = catalog.invitation(invitation).unwrap().unwrap();
        let mut entry = b
            .engine
            .entry(
                invite.offer.group,
                &invite.offer.title,
                invite.offer.profile,
                Some(invite.offer.owner.clone()),
                NOW,
            )
            .unwrap();
        entry.invitation = Some(invitation);
        let joining_key = entry.local.key;
        invite.state = InvitationState::JoinRequested;
        let edit = indexed::Edit::new(&catalog)
            .unwrap()
            .put_current(
                &catalog,
                entry.group,
                &indexed::rows::Current::pending(&entry),
            )
            .unwrap()
            .put_incarnation(
                &catalog,
                indexed::keys::incarnation(entry.group, entry.generation),
                &indexed::rows::Incarnation::Live(entry.clone()),
            )
            .unwrap()
            .bind_invitation(
                &catalog,
                invitation,
                &indexed::rows::InvitationBinding {
                    namespace: indexed::rows::NamespaceReceipt {
                        group: entry.group,
                        generation: entry.generation,
                        incarnation: entry.incarnation,
                    },
                    member: entry.local.clone(),
                },
            )
            .unwrap()
            .put_invitation(&catalog, &invite)
            .unwrap();
        b.engine
            .write(
                Enrollment::remember(
                    &catalog,
                    edit,
                    request(3),
                    hash(&encode(&("respond", invitation, true)).unwrap()),
                    invitation,
                )
                .unwrap()
                .finish()
                .unwrap(),
            )
            .unwrap();
        joining_key
    };
    drop(a);
    drop(b);
    drop(dbs);
    let dbs = [
        Database::open(&paths[0]).unwrap(),
        Database::open(&paths[1]).unwrap(),
    ];
    let (mut a, mut b) = (Device::new(&dbs[0], 1), Device::new(&dbs[1], 2));
    // Recovery is explicit bounded maintenance, independent of the lost caller
    // request ID. Work selection itself does not scan every namespace.
    for _ in 0..8 {
        b.engine.maintain(&mut b.owner, NOW).unwrap();
    }
    let work = b.work("join");
    if let Frame::Join { member, .. } = decode(&work.frame).unwrap() {
        assert_eq!(member.key, joining_key)
    } else {
        panic!("expected join")
    }
    let response = a.receive(b.identity, &work, NOW);
    b.complete(&work, &response, NOW);
    assert_eq!(b.descriptor(group).readers.len(), 2);
}
#[test]
fn expired_and_declined_invitations_do_not_add_members() {
    let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let dbs = [
        Database::create(dirs[0].path().join("a")).unwrap(),
        Database::create(dirs[1].path().join("b")).unwrap(),
    ];
    let (mut a, mut b) = (Device::new(&dbs[0], 1), Device::new(&dbs[1], 2));
    let group = a
        .engine
        .create(&mut a.owner, request(1), "decline", NOW)
        .unwrap();
    let invitation = offer(&mut a, &mut b, group, 2);
    b.engine
        .respond(&mut b.owner, request(4), invitation, false, NOW)
        .unwrap();
    let work = b.work("decline");
    let response = a.receive(b.identity, &work, NOW);
    b.complete(&work, &response, NOW);
    assert_eq!(a.invitations(NOW)[0].state, InvitationState::Declined);
    assert_eq!(a.descriptor(group).readers.len(), 1);
    let expired = offer(&mut a, &mut b, group, 5);
    assert!(matches!(
        b.engine.respond(
            &mut b.owner,
            request(3),
            expired,
            true,
            NOW + INVITATION_LIFETIME_SECS
        ),
        Err(EnrollmentError::Missing | EnrollmentError::Expired)
    ));
    assert!(matches!(
        b.engine
            .respond(&mut b.owner, request(3), expired, true, NOW),
        Err(EnrollmentError::Missing | EnrollmentError::Expired)
    ));
    assert!(!EnrollmentError::Capacity.is_invalid_input());
}

#[test]
fn outgoing_invitations_reserve_slots_and_reject_duplicate_recipients() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("a")).unwrap();
    let mut a = Device::new(&db, 1);
    let group = a
        .engine
        .create(&mut a.owner, request(1), "bounded", NOW)
        .unwrap();
    let mut first = None;
    for id in 2..9 {
        let invitation = a
            .engine
            .invite(
                &mut a.owner,
                request(id),
                group,
                IdentityHash::new([id; 16]),
                NOW,
            )
            .unwrap();
        if first.is_none() {
            first = Some(invitation)
        }
    }
    assert!(matches!(
        a.engine.invite(
            &mut a.owner,
            request(20),
            group,
            IdentityHash::new([2; 16]),
            NOW
        ),
        Err(EnrollmentError::Conflict)
    ));
    assert!(matches!(
        a.engine.invite(
            &mut a.owner,
            request(21),
            group,
            IdentityHash::new([9; 16]),
            NOW
        ),
        Err(EnrollmentError::Capacity)
    ));
    a.engine
        .handle(
            &mut a.owner,
            IdentityHash::new([2; 16]),
            &frame(&Frame::Decline {
                invitation: first.unwrap(),
            })
            .unwrap(),
            NOW,
        )
        .unwrap();
    a.engine
        .invite(
            &mut a.owner,
            request(21),
            group,
            IdentityHash::new([9; 16]),
            NOW,
        )
        .unwrap();
    assert_eq!(a.invitations(NOW).len(), 8);
    // Unconsumed expired reservations are released without authorizing late Add.
    // Retirement is bounded work, so explicitly finish a maintenance rotation
    // before expecting all seven earlier reservations to have been refunded.
    for _ in 0..8 {
        a.engine
            .maintain(&mut a.owner, NOW + INVITATION_LIFETIME_SECS)
            .unwrap();
    }
    a.engine
        .invite(
            &mut a.owner,
            request(22),
            group,
            IdentityHash::new([10; 16]),
            NOW + INVITATION_LIFETIME_SECS,
        )
        .unwrap();
}

#[test]
fn default_mls_wire_fits_and_selected_complete_frame_budget_is_enforced() {
    let request = Frame::Join {
        invitation: [1; 32],
        profile: profile_digest(crate::GroupProfile::default()).unwrap(),
        member: Member {
            rns: [2; 16],
            endpoint: [3; 32],
            incarnation: 1,
            key: [4; 32],
        },
        package: vec![255; 65_536],
    };
    let bytes = frame(&request).unwrap();
    assert!(bytes.len() < MAX_FRAME_BYTES);
    let Frame::Join { package, .. } = decode(&bytes).unwrap() else {
        panic!("expected join")
    };
    assert_eq!(package, vec![255; 65_536]);
    let oversized = Frame::Join {
        invitation: [1; 32],
        profile: profile_digest(crate::GroupProfile::default()).unwrap(),
        member: Member {
            rns: [2; 16],
            endpoint: [3; 32],
            incarnation: 1,
            key: [4; 32],
        },
        package: vec![255; MAX_FRAME_BYTES],
    };
    assert!(frame(&oversized).is_err());
    assert!(decode::<Frame>(&vec![b' '; MAX_FRAME_BYTES + 1]).is_err());
}
