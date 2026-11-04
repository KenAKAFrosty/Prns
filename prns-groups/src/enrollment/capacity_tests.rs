use super::*;
use prns_groups_store::{AtomicBackend, CommitOutcome, Mutation};

#[test]
fn ordinary_object_exhaustion_preserves_all_member_removals_and_closure() {
    assert_management_survives(Exhaustion::Objects);
}

#[test]
fn pinned_service_receipts_preserve_all_member_removals_and_closure() {
    assert_management_survives(Exhaustion::Receipts);
}

enum Exhaustion {
    Objects,
    Receipts,
}

fn assert_management_survives(exhaustion: Exhaustion) {
    let directory = tempfile::tempdir().unwrap();
    let databases = (0..profile::MAX_MEMBERS)
        .map(|index| Database::create(directory.path().join(index.to_string())).unwrap())
        .collect::<Vec<_>>();
    let mut owner = Device::new(&databases[0], 1);
    let mut members = databases[1..]
        .iter()
        .enumerate()
        .map(|(index, database)| Device::new(database, index as u8 + 2))
        .collect::<Vec<_>>();
    let group = owner
        .engine
        .create(&mut owner.owner, request(1), "Reserved management", NOW)
        .unwrap();
    let mut endpoints = Vec::new();
    for (index, member) in members.iter_mut().enumerate() {
        let token = index as u8 * 2 + 2;
        let invitation = offer(&mut owner, member, group, token);
        member
            .engine
            .respond(&mut member.owner, request(token + 1), invitation, true, NOW)
            .unwrap();
        let join = member.work("join");
        let reply = owner.receive(member.identity, &join, NOW);
        member.complete(&join, &reply, NOW);
        let joined = member.work("joined");
        let reply = owner.receive(member.identity, &joined, NOW);
        member.complete(&joined, &reply, NOW);
        endpoints.push(
            Member::new(member.identity, member.descriptor(group).identity.binding())
                .endpoint()
                .unwrap(),
        );
    }
    let config = owner.descriptor(group);
    let clock = prns_groups_service::ClockReading::Trusted(
        prns_groups_service::ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap(),
    );
    assert!(matches!(
        crate::session::execute_with_owner(
            &databases[0],
            &mut owner.owner,
            &config,
            crate::test_support::command(crate::test_support::text(
                group,
                crate::OriginalScope::Control {
                    control: config.control,
                    incarnation: config.incarnation
                },
                request(35),
                "Pending service output pins the receipt prefix".into(),
                clock
            )),
        )
        .unwrap(),
        crate::client::Reply::Outcome(crate::Outcome::Published(_))
    ));
    owner.engine.maintain(&mut owner.owner, NOW).unwrap();
    let limits = profile::store_limits();
    let reserve = config.cleanup_reserve.unwrap();
    let ordinary_objects = usize::try_from(limits.max_live_objects).unwrap() - reserve.objects;
    let ordinary_receipts = usize::try_from(limits.max_receipts).unwrap() - reserve.receipts;
    let full_revision = {
        let mut backend = owner
            .owner
            .open(config.namespace, config.incarnation, limits)
            .unwrap()
            .reserve_capacity(reserve)
            .unwrap();
        let mut next_key = 0usize;
        loop {
            let usage = backend.usage().unwrap();
            let changes = match exhaustion {
                Exhaustion::Objects => {
                    if usage.live_objects == ordinary_objects {
                        break;
                    }
                    let remaining = ordinary_objects - usage.live_objects;
                    (0..remaining.min(usize::try_from(limits.max_changes).unwrap()))
                        .map(|_| {
                            let key = format!("capacity-fixture/{next_key}").into_bytes();
                            next_key += 1;
                            Mutation::Put {
                                key,
                                value: vec![0],
                            }
                        })
                        .collect::<Vec<_>>()
                }
                Exhaustion::Receipts => {
                    if usage.receipts == ordinary_receipts {
                        break;
                    }
                    Vec::new()
                }
            };
            let revision = backend.head().unwrap();
            let operation = OperationTicket::new(config.incarnation, revision.0 + 1).unwrap();
            assert!(matches!(
                backend
                    .commit(revision, operation.operation_id(), &changes)
                    .unwrap(),
                CommitOutcome::Committed(_)
            ));
        }
        backend.head().unwrap()
    };
    let ordinary = crate::session::execute_with_owner(
        &databases[0],
        &mut owner.owner,
        &config,
        crate::test_support::command(crate::test_support::text(
            group,
            crate::OriginalScope::Control {
                control: config.control,
                incarnation: config.incarnation,
            },
            request(40),
            "Cannot consume management reserve".into(),
            clock,
        )),
    );
    assert!(matches!(ordinary, Err(error) if error.is_storage_full()));
    {
        let mut backend = owner
            .owner
            .open(config.namespace, config.incarnation, limits)
            .unwrap();
        assert_eq!(backend.head().unwrap(), full_revision);
        let usage = backend.usage().unwrap();
        match exhaustion {
            Exhaustion::Objects => assert_eq!(usage.live_objects, ordinary_objects),
            Exhaustion::Receipts => assert_eq!(usage.receipts, ordinary_receipts),
        }
    }
    for (index, endpoint) in endpoints.into_iter().enumerate() {
        let head = owner.descriptor(group).control.head.0;
        owner
            .engine
            .remove_member(
                &mut owner.owner,
                request(index as u8 + 50),
                group,
                endpoint,
                head,
                NOW,
            )
            .unwrap();
    }
    let remaining = owner.descriptor(group);
    assert_eq!(remaining.readers.len(), 1);
    owner
        .engine
        .close(
            &mut owner.owner,
            request(60),
            group,
            remaining.control.head.0,
            NOW,
        )
        .unwrap();
    drop(owner);
    let mut reopened = Device::new(&databases[0], 1);
    assert_eq!(reopened.descriptor(group).lifecycle, GroupLifecycle::Closed);
}
