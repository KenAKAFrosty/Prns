//! Contract checks for immutable shared profiles. These small real-MLS cases
//! are distinct from the later 256-member capacity qualification.
use super::*;

fn create(device: &mut Device<'_>, id: u8, profile: crate::GroupProfile) -> GroupId {
    match device
        .engine
        .test_execute(
            &mut device.owner,
            request(id),
            crate::Intent::Create {
                title: "selected profile".into(),
                profile,
            },
            None,
            NOW,
        )
        .unwrap()
    {
        crate::Outcome::Created(group) => group,
        _ => panic!("expected created group"),
    }
}

fn joined(owner: &mut Device<'_>, member: &mut Device<'_>, group: GroupId, nonce: u8) {
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
fn selected_profile_admits_real_members_beyond_the_old_eight_member_policy() {
    let directory = tempfile::tempdir().unwrap();
    let dbs = (0..10)
        .map(|id| Database::create(directory.path().join(format!("{id}.redb"))).unwrap())
        .collect::<Vec<_>>();
    let mut devices = dbs
        .iter()
        .enumerate()
        .map(|(id, db)| Device::new(db, u8::try_from(id + 1).unwrap()))
        .collect::<Vec<_>>();
    let profile = crate::GroupProfile {
        max_members: 12,
        ..Default::default()
    };
    let (owner, members) = devices.split_first_mut().unwrap();
    let group = create(owner, 1, profile);
    for (id, member) in members.iter_mut().enumerate() {
        joined(owner, member, group, u8::try_from(3 * id + 2).unwrap());
    }
    let expected = owner.descriptor(group);
    assert_eq!(expected.readers.len(), 10);
    assert_eq!(expected.mls_limits.max_members, 12);
    assert_eq!(expected.service_limits.max_targets, 12);
    // Each participant consumes the owner's real control chain to the same
    // head, including participants admitted before the ninth member.
    for member in members {
        for _ in 0..9 {
            if member.descriptor(group).control == expected.control {
                break;
            }
            let work = member.work("sync");
            let response = owner.receive(member.identity, &work, NOW);
            member.complete(&work, &response, NOW);
        }
        let actual = member.descriptor(group);
        assert_eq!(actual.control, expected.control);
        assert_eq!(actual.readers, expected.readers);
        assert_eq!(actual.mls_limits.max_members, 12);
    }
}

#[test]
fn profile_is_exact_create_input_and_survives_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owner.redb");
    let profile = crate::GroupProfile {
        max_members: 12,
        ..Default::default()
    };
    let group = {
        let db = Database::create(&path).unwrap();
        let mut owner = Device::new(&db, 1);
        let group = create(&mut owner, 1, profile);
        assert_eq!(create(&mut owner, 1, profile), group);
        assert!(matches!(
            owner.engine.test_execute(
                &mut owner.owner,
                request(1),
                crate::Intent::Create {
                    title: "selected profile".into(),
                    profile: crate::GroupProfile::default(),
                },
                None,
                NOW,
            ),
            Err(EnrollmentError::RequestConflict)
        ));
        assert_eq!(owner.ready_groups(), vec![group]);
        group
    };
    let db = Database::open(path).unwrap();
    let mut owner = Device::new(&db, 1);
    assert_eq!(create(&mut owner, 1, profile), group);
    assert_eq!(owner.descriptor(group).mls_limits.max_members, 12);
}

#[test]
fn unsupported_offer_is_explicit_and_creates_no_recipient_membership() {
    let directory = tempfile::tempdir().unwrap();
    let a = Database::create(directory.path().join("a.redb")).unwrap();
    let b = Database::create(directory.path().join("b.redb")).unwrap();
    let local = IdentityHash::new([1; 16]);
    let tx = a.begin_write().unwrap();
    Enrollment::initialize_with_limits_in(
        &tx,
        local,
        CatalogLimits {
            max_enrollment_frame_bytes: 524_288,
            ..Default::default()
        },
    )
    .unwrap();
    tx.commit().unwrap();
    let (mut owner, mut member) = (Device::new(&a, 1), Device::new(&b, 2));
    let group = create(
        &mut owner,
        1,
        crate::GroupProfile {
            max_members: 256,
            max_enrollment_frame_bytes: 524_288,
            ..Default::default()
        },
    );
    owner
        .engine
        .invite(&mut owner.owner, request(2), group, member.identity, NOW)
        .unwrap();
    let work = owner.work("offer");
    let response = member.receive(owner.identity, &work, NOW);
    assert!(matches!(
        decode::<Reply>(&response).unwrap(),
        Reply::UnsupportedProfile
    ));
    owner.complete(&work, &response, NOW);
    let invitation = owner.invitations(NOW).remove(0);
    assert_eq!(invitation.state, InvitationState::UnsupportedProfile);
    assert!(invitation.can_dismiss);
    assert!(member.engine.read().unwrap().header.counts.live == 0);
    assert!(member.invitations(NOW).is_empty());
    assert!(owner.work_matching(|_| true).is_none());
}

#[test]
fn carrier_support_rejects_offer_before_recipient_membership_or_key_package() {
    let directory = tempfile::tempdir().unwrap();
    let a = Database::create(directory.path().join("a.redb")).unwrap();
    let b = Database::create(directory.path().join("b.redb")).unwrap();
    let (mut owner, mut member) = (Device::new(&a, 1), Device::new(&b, 2));
    let profile = crate::GroupProfile::default();
    let group = create(&mut owner, 1, profile);
    member
        .engine
        .set_profile_support(
            crate::ProfileSupport::new(
                profile.max_wire_bytes as usize - 1,
                profile.max_enrollment_frame_bytes as usize,
            )
            .unwrap(),
        )
        .unwrap();
    owner
        .engine
        .invite(&mut owner.owner, request(2), group, member.identity, NOW)
        .unwrap();
    let work = owner.work("offer");
    let response = member.receive(owner.identity, &work, NOW);
    assert!(matches!(
        decode::<Reply>(&response).unwrap(),
        Reply::UnsupportedProfile
    ));
    let catalog = member.engine.read().unwrap();
    assert!(catalog.header.counts.live == 0);
    assert!(catalog.header.counts.invitations == 0);
}

#[test]
fn restored_pending_invitation_must_fit_before_network_readiness() {
    let directory = tempfile::tempdir().unwrap();
    let a = Database::create(directory.path().join("a.redb")).unwrap();
    let b = Database::create(directory.path().join("b.redb")).unwrap();
    let (mut owner, mut member) = (Device::new(&a, 1), Device::new(&b, 2));
    let profile = crate::GroupProfile::default();
    let group = create(&mut owner, 1, profile);
    let invitation = offer(&mut owner, &mut member, group, 2);
    drop(member);
    let mut restored = Device::new(&b, 2);
    assert!(matches!(
        restored.engine.set_profile_support(
            crate::ProfileSupport::new(
                profile.max_wire_bytes as usize,
                profile.max_enrollment_frame_bytes as usize - 1,
            )
            .unwrap()
        ),
        Err(EnrollmentError::UnsupportedProfile)
    ));
    assert!(restored.engine.profile_support.is_none());
    assert!(restored.engine.read().unwrap().header.counts.live == 0);
    restored
        .engine
        .set_profile_support(
            crate::ProfileSupport::new(
                profile.max_wire_bytes as usize,
                profile.max_enrollment_frame_bytes as usize,
            )
            .unwrap(),
        )
        .unwrap();
    restored
        .engine
        .respond(&mut restored.owner, request(3), invitation, true, NOW)
        .unwrap();
    assert_eq!(restored.work("join").recipient, owner.identity);
}

#[test]
fn join_digest_and_even_authentic_successor_cannot_change_selected_profile() {
    let directory = tempfile::tempdir().unwrap();
    let a = Database::create(directory.path().join("a.redb")).unwrap();
    let b = Database::create(directory.path().join("b.redb")).unwrap();
    let (mut owner, mut member) = (Device::new(&a, 1), Device::new(&b, 2));
    let profile = crate::GroupProfile {
        max_members: 12,
        ..Default::default()
    };
    let group = create(&mut owner, 1, profile);
    let invitation = offer(&mut owner, &mut member, group, 2);
    member
        .engine
        .respond(&mut member.owner, request(3), invitation, true, NOW)
        .unwrap();
    let work = member.work("join");
    let mut changed: Frame = decode(&work.frame).unwrap();
    let Frame::Join { profile, .. } = &mut changed else {
        panic!("expected Join")
    };
    *profile = [0; 32];
    assert!(matches!(
        owner.engine.handle(
            &mut owner.owner,
            member.identity,
            &frame(&changed).unwrap(),
            NOW
        ),
        Err(EnrollmentError::Authentication)
    ));
    assert_eq!(owner.descriptor(group).readers.len(), 1);
    let response = owner.receive(member.identity, &work, NOW);
    let mut changed: Reply = decode(&response).unwrap();
    let Reply::Welcome { tip, .. } = &mut changed else {
        panic!("expected Welcome")
    };
    tip.manifest.profile.max_members = 16;
    let catalog = owner.engine.read().unwrap();
    let entry = Enrollment::find(&catalog, group.0).unwrap();
    tip.signature = entry
        .identity()
        .unwrap()
        .sign_enrollment(&encode(&tip.manifest).unwrap())
        .unwrap()
        .to_vec();
    assert!(matches!(
        member
            .engine
            .complete(&mut member.owner, work.id, &frame(&changed).unwrap(), NOW),
        Err(EnrollmentError::Authentication)
    ));
    assert!(member.ready_groups().is_empty());
    member.complete(&work, &response, NOW);
    assert_eq!(member.descriptor(group).readers.len(), 2);
}

#[test]
fn pinned_frame_bound_is_enforced_even_when_local_ingress_allows_more() {
    let directory = tempfile::tempdir().unwrap();
    let dbs = ["a", "b"].map(|name| Database::create(directory.path().join(name)).unwrap());
    for (index, db) in dbs.iter().enumerate() {
        let tx = db.begin_write().unwrap();
        Enrollment::initialize_with_limits_in(
            &tx,
            IdentityHash::new([u8::try_from(index + 1).unwrap(); 16]),
            CatalogLimits {
                max_enrollment_frame_bytes: 524_288,
                ..Default::default()
            },
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let (mut owner, mut member) = (Device::new(&dbs[0], 1), Device::new(&dbs[1], 2));
    let group = create(&mut owner, 1, crate::GroupProfile::default());
    let invitation = offer(&mut owner, &mut member, group, 2);
    member
        .engine
        .respond(&mut member.owner, request(3), invitation, true, NOW)
        .unwrap();
    let work = member.work("join");
    let response = owner.receive(member.identity, &work, NOW);
    let mut padded = response.clone();
    padded.resize(MAX_FRAME_BYTES + 1, b' ');
    assert!(matches!(
        member
            .engine
            .complete(&mut member.owner, work.id, &padded, NOW),
        Err(EnrollmentError::Capacity)
    ));
    assert!(member.ready_groups().is_empty());
    member.complete(&work, &response, NOW);
    assert_eq!(member.descriptor(group).readers.len(), 2);
}

#[test]
fn old_wire_version_is_rejected_before_catalog_maintenance() {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::create(directory.path().join("member")).unwrap();
    let mut member = Device::new(&db, 2);
    let before = member.engine.read().unwrap().header.fingerprint().unwrap();
    let mut wire: serde_json::Value = serde_json::from_slice(
        &frame(&Frame::Decline {
            invitation: [1; 32],
        })
        .unwrap(),
    )
    .unwrap();
    wire["version"] = serde_json::json!(WIRE_VERSION - 1);
    let bytes = serde_json::to_vec(&wire).unwrap();
    assert!(matches!(
        member
            .engine
            .handle(&mut member.owner, IdentityHash::new([1; 16]), &bytes, NOW),
        Err(EnrollmentError::UnsupportedVersion)
    ));
    assert_eq!(
        member.engine.read().unwrap().header.fingerprint().unwrap(),
        before
    );
    wire["version"] = serde_json::json!(WIRE_VERSION);
    assert!(decode::<Frame>(&serde_json::to_vec(&wire).unwrap()).is_ok());
}
