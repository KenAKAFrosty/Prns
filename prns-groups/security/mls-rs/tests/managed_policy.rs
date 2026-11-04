#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[allow(dead_code)]
mod support;
use prns_groups_mls::{Admission, Command, CompositionError, Error, LocalIdentity, Policy};
use prns_groups_store::OperationId;
use support::{limits, Endpoint, TestResult, GROUP, NOW};

#[test]
fn managed_roster_change_preserves_configuration_and_wrong_tree_rolls_back() -> TestResult {
    let alice = LocalIdentity::generate([1; 32], 1)?;
    let bob = LocalIdentity::generate([2; 32], 1)?;
    let charlie = LocalIdentity::generate([3; 32], 1)?;
    let owner_binding = alice.binding();
    let member_binding = bob.binding();
    let mut owner = Endpoint::new(
        alice,
        Policy::owner_managed(owner_binding.clone(), vec![owner_binding.clone()])?,
        limits(),
    )?;
    owner.adapter().create(OperationId([1; 32]), GROUP)?;
    // Only a validated application control may widen this policy. Configuration
    // binds the immutable owner and local key, not the transient allowlist.
    owner.policy = Policy::owner_managed(
        owner_binding.clone(),
        vec![
            owner_binding.clone(),
            member_binding.clone(),
            charlie.binding(),
        ],
    )?;
    owner.reopen()?;
    let member = Endpoint::new(bob, owner.policy.clone(), limits())?;
    let package = member.adapter().key_package(OperationId([1; 32]), NOW)?;
    assert!(owner
        .adapter()
        .key_package_matches(&package.output.messages[0], &member_binding)?);
    assert!(!owner
        .adapter()
        .key_package_matches(&package.output.messages[0], &charlie.binding())?);
    let before = owner.bytes()?;
    // The allowlist is a superset: merely validating every member against it
    // would miss this advertised roster mismatch.
    let wrong = [owner_binding.clone(), charlie.binding()];
    let failed = owner.adapter().compose_with_roster(
        OperationId([2; 32]),
        Command::Add {
            group: GROUP,
            packages: &[&package.output.messages[0]],
            now_secs: NOW,
        },
        Some(&wrong),
        |_, _, _| Ok::<_, ()>(Admission::Reject(1)),
        |_, _| Ok(()),
    );
    assert!(matches!(
        failed,
        Err(CompositionError::Mls(Error::Policy(_)))
    ));
    assert_eq!(before, owner.bytes()?);
    assert!(owner.adapter().retry(OperationId([2; 32]))?.is_none());
    let exact = [owner_binding.clone(), member_binding.clone()];
    let added = owner
        .adapter()
        .compose_with_roster(
            OperationId([2; 32]),
            Command::Add {
                group: GROUP,
                packages: &[&package.output.messages[0]],
                now_secs: NOW,
            },
            Some(&exact),
            |_, _, _| Ok::<_, ()>(Admission::Reject(1)),
            |_, _| Ok(()),
        )
        .unwrap();
    let before_join = member.bytes()?;
    let wrong = [
        owner_binding.clone(),
        member_binding.clone(),
        charlie.binding(),
    ];
    let failed = member.adapter().compose_with_roster(
        OperationId([2; 32]),
        Command::Join {
            group: GROUP,
            welcome: &added.output.messages[1],
            now_secs: NOW,
        },
        Some(&wrong),
        |_, _, _| Ok::<_, ()>(Admission::Reject(1)),
        |_, _| Ok(()),
    );
    assert!(matches!(
        failed,
        Err(CompositionError::Mls(Error::Policy(_)))
    ));
    assert_eq!(before_join, member.bytes()?);
    member
        .adapter()
        .compose_with_roster(
            OperationId([2; 32]),
            Command::Join {
                group: GROUP,
                welcome: &added.output.messages[1],
                now_secs: NOW,
            },
            Some(&exact),
            |_, _, _| Ok::<_, ()>(Admission::Reject(1)),
            |_, _| Ok(()),
        )
        .unwrap();
    let before_rekey = owner.bytes()?;
    assert!(owner.adapter().rekey(OperationId([3; 32]), GROUP).is_err());
    assert_eq!(before_rekey, owner.bytes()?);
    owner.policy = Policy::owner_managed(
        charlie.binding(),
        vec![owner_binding, member_binding, charlie.binding()],
    )?;
    assert!(owner.reopen().is_err());
    Ok(())
}

#[test]
fn fixed_policy_configuration_stays_immutable_and_signatures_are_domain_separated() -> TestResult {
    let alice = LocalIdentity::generate([1; 32], 1)?;
    let bob = LocalIdentity::generate([2; 32], 1)?;
    let signed = alice.sign_enrollment(b"canonical manifest")?;
    alice
        .binding()
        .verify_enrollment(b"canonical manifest", &signed)?;
    assert!(alice
        .binding()
        .verify_enrollment(b"different manifest", &signed)
        .is_err());
    assert!(bob
        .binding()
        .verify_enrollment(b"canonical manifest", &signed)
        .is_err());
    let mut owner = Endpoint::new(
        alice.clone(),
        Policy::new(alice.binding(), vec![alice.binding()])?,
        limits(),
    )?;
    owner.adapter().create(OperationId([1; 32]), GROUP)?;
    owner.policy = Policy::new(alice.binding(), vec![alice.binding(), bob.binding()])?;
    assert!(owner.reopen().is_err());
    Ok(())
}
