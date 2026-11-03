#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[allow(dead_code)]
mod support;

use prns_groups_mls::{Admission, Command, CompositionError, Error, Event, LocalIdentity, Policy};
use prns_groups_store::{OperationId, RecoveryOutcome};
use support::{durable::Cut, limits, Endpoint, TestResult, GROUP, NOW};

fn operation(sequence: u8) -> OperationId {
    OperationId([sequence; 32])
}

fn managed_pair() -> TestResult<(Endpoint, Endpoint)> {
    let alice = LocalIdentity::generate([1; 32], 1)?;
    let bob = LocalIdentity::generate([2; 32], 1)?;
    let policy = Policy::owner_managed(alice.binding(), vec![alice.binding(), bob.binding()])?;
    let owner = Endpoint::new(alice, policy.clone(), limits())?;
    let member = Endpoint::new(bob, policy, limits())?;
    owner.adapter().create(operation(1), GROUP)?;
    let package = member.adapter().key_package(operation(1), NOW)?;
    let roster = [owner.identity.binding(), member.identity.binding()];
    let added = owner
        .adapter()
        .compose_with_roster(
            operation(2),
            Command::Add {
                group: GROUP,
                packages: &[&package.output.messages[0]],
                now_secs: NOW,
            },
            Some(&roster),
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Ok(()),
        )
        .unwrap();
    member
        .adapter()
        .compose_with_roster(
            operation(2),
            Command::Join {
                group: GROUP,
                welcome: &added.output.messages[1],
                now_secs: NOW,
            },
            Some(&roster),
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Ok(()),
        )
        .unwrap();
    Ok((owner, member))
}

#[test]
fn managed_removal_checks_exact_binding_and_removed_recipient_roster() -> TestResult {
    let (mut owner, mut member) = managed_pair()?;
    let before = owner.bytes()?;
    for binding in [
        owner.identity.binding(),
        LocalIdentity::generate([2; 32], 2)?.binding(),
    ] {
        assert!(owner
            .adapter()
            .compose(
                operation(3),
                Command::RemoveMember {
                    group: GROUP,
                    member: binding
                },
                |_, _, _| Ok::<_, ()>(Admission::Accept),
                |_, _| Ok(()),
            )
            .is_err());
        assert_eq!(owner.bytes()?, before);
    }
    let remaining = [owner.identity.binding()];
    let removal = owner
        .adapter()
        .compose_with_roster(
            operation(3),
            Command::RemoveMember {
                group: GROUP,
                member: member.identity.binding(),
            },
            Some(&remaining),
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Ok(()),
        )
        .unwrap();
    let before = member.bytes()?;
    let wrong = [owner.identity.binding(), member.identity.binding()];
    assert!(member
        .adapter()
        .compose_with_roster(
            operation(3),
            Command::Receive {
                group: GROUP,
                bytes: &removal.output.messages[0]
            },
            Some(&wrong),
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Ok(()),
        )
        .is_err());
    assert_eq!(member.bytes()?, before);
    let removed = member
        .adapter()
        .compose_with_roster(
            operation(3),
            Command::Receive {
                group: GROUP,
                bytes: &removal.output.messages[0],
            },
            Some(&remaining),
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |tx, output| {
                assert_eq!(output.event, Event::Removed);
                tx.put(b"terminal", b"removed").map_err(|_| ())
            },
        )
        .unwrap();
    assert_eq!(removed.output.event, Event::Removed);
    member.reopen()?;
    assert!(member.adapter().is_retired(GROUP)?);
    assert!(member
        .adapter()
        .send(operation(4), GROUP, b"forbidden")
        .is_err());
    assert_eq!(
        member
            .adapter()
            .application_read(b"terminal")?
            .unwrap()
            .as_slice(),
        b"removed"
    );
    owner.policy = Policy::owner_managed(owner.identity.binding(), remaining.to_vec())?;
    owner.reopen()?;
    owner.adapter().send(operation(4), GROUP, b"still active")?;
    Ok(())
}

#[test]
fn managed_remaining_member_advances_while_removed_member_loses_access() -> TestResult {
    let (mut owner, mut member) = managed_pair()?;
    let charlie = LocalIdentity::generate([3; 32], 1)?;
    let all = [
        owner.identity.binding(),
        member.identity.binding(),
        charlie.binding(),
    ];
    let policy = Policy::owner_managed(owner.identity.binding(), all.to_vec())?;
    owner.policy = policy.clone();
    member.policy = policy.clone();
    owner.reopen()?;
    member.reopen()?;
    let removed = Endpoint::new(charlie, policy, limits())?;
    let package = removed.adapter().key_package(operation(1), NOW)?;
    let add = owner
        .adapter()
        .compose_with_roster(
            operation(3),
            Command::Add {
                group: GROUP,
                packages: &[&package.output.messages[0]],
                now_secs: NOW,
            },
            Some(&all),
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Ok(()),
        )
        .unwrap();
    member
        .adapter()
        .compose_with_roster(
            operation(3),
            Command::Receive {
                group: GROUP,
                bytes: &add.output.messages[0],
            },
            Some(&all),
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Ok(()),
        )
        .unwrap();
    removed
        .adapter()
        .compose_with_roster(
            operation(2),
            Command::Join {
                group: GROUP,
                welcome: &add.output.messages[1],
                now_secs: NOW,
            },
            Some(&all),
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Ok(()),
        )
        .unwrap();
    assert!(member
        .adapter()
        .compose(
            operation(4),
            Command::RemoveMember {
                group: GROUP,
                member: removed.identity.binding()
            },
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Ok(()),
        )
        .is_err());
    let old = owner
        .adapter()
        .send(operation(4), GROUP, b"old authority")?;
    let remaining = [owner.identity.binding(), member.identity.binding()];
    let removal = owner
        .adapter()
        .compose_with_roster(
            operation(5),
            Command::RemoveMember {
                group: GROUP,
                member: removed.identity.binding(),
            },
            Some(&remaining),
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Ok(()),
        )
        .unwrap();
    let accepted = member
        .adapter()
        .compose_with_roster(
            operation(4),
            Command::Receive {
                group: GROUP,
                bytes: &removal.output.messages[0],
            },
            Some(&remaining),
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Ok(()),
        )
        .unwrap();
    assert_eq!(accepted.output.event, Event::Control);
    let terminal = removed
        .adapter()
        .compose_with_roster(
            operation(3),
            Command::Receive {
                group: GROUP,
                bytes: &removal.output.messages[0],
            },
            Some(&remaining),
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Ok(()),
        )
        .unwrap();
    assert_eq!(terminal.output.event, Event::Removed);
    let policy = Policy::owner_managed(owner.identity.binding(), remaining.to_vec())?;
    owner.policy = policy.clone();
    member.policy = policy;
    owner.reopen()?;
    member.reopen()?;
    let fresh = owner
        .adapter()
        .send(operation(6), GROUP, b"new authority")?;
    let accepted =
        member
            .adapter()
            .receive(operation(5), GROUP, &fresh.output.messages[0], |_| {
                Admission::Accept
            })?;
    assert!(matches!(&accepted.output.event, Event::Application(body) if body == b"new authority"));
    assert!(removed
        .adapter()
        .receive(operation(4), GROUP, &fresh.output.messages[0], |_| panic!(
            "removed recipient received new data"
        ))
        .is_err());
    assert!(member
        .adapter()
        .receive(operation(6), GROUP, &old.output.messages[0], |_| panic!(
            "old era reached admission"
        ))
        .is_err());
    Ok(())
}

#[test]
fn retirement_and_application_terminal_fact_recover_together() -> TestResult {
    for cut in [
        Cut::BeforeSnapshot,
        Cut::DuringSnapshot,
        Cut::AfterSnapshot,
        Cut::AfterPublish,
    ] {
        let (mut owner, _) = managed_pair()?;
        owner.adapter().send(operation(3), GROUP, b"pending")?;
        let before = owner.bytes()?;
        let rejected = owner.adapter().compose(
            operation(4),
            Command::Retire { group: GROUP },
            |_, _, _| Ok::<_, ()>(Admission::Accept),
            |_, _| Err(()),
        );
        assert!(matches!(rejected, Err(CompositionError::Application(()))));
        assert_eq!(owner.bytes()?, before);
        owner.faults.arm(cut);
        let retire = |owner: &Endpoint| {
            owner.adapter().compose(
                operation(4),
                Command::Retire { group: GROUP },
                |_, _, _| Ok::<_, ()>(Admission::Accept),
                |tx, output| {
                    assert_eq!(output.event, Event::Retired);
                    tx.put(b"closure", b"exact signed closure").map_err(|_| ())
                },
            )
        };
        assert!(retire(&owner).is_err());
        owner.reopen()?;
        let recovered = owner.adapter().coordinator().recover(operation(4))?;
        if matches!(cut, Cut::AfterPublish) {
            assert!(matches!(recovered, RecoveryOutcome::Committed(_)));
        } else {
            assert!(matches!(recovered, RecoveryOutcome::NotCommitted { .. }));
            assert_eq!(owner.bytes()?, before);
            retire(&owner).unwrap();
        }
        owner.reopen()?;
        assert!(owner.adapter().is_retired(GROUP)?);
        assert_eq!(
            owner.adapter().retry(operation(4))?.unwrap().event,
            Event::Retired
        );
        assert!(matches!(
            owner.adapter().retry(operation(3)),
            Err(Error::PriorAuthorizationEra)
        ));
        assert!(owner
            .adapter()
            .send(operation(5), GROUP, b"forbidden")
            .is_err());
        assert!(owner.adapter().create(operation(5), GROUP).is_err());
        assert_eq!(
            owner
                .adapter()
                .application_read(b"closure")?
                .unwrap()
                .as_slice(),
            b"exact signed closure"
        );
    }
    Ok(())
}
