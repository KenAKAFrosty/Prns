use super::*;

#[test]
fn termination_preserves_catalog_enablement_and_drains_exact_owners_and_leases() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let first = create(&mut core, &mut disk, 1);
    let second = create(&mut core, &mut disk, 2);
    let third = create(&mut core, &mut disk, 3);
    let mut runtime = Controller::new(&mut state);
    let a = running(&mut runtime, &mut core, &mut disk, &mut driver, first);
    let b = running(&mut runtime, &mut core, &mut disk, &mut driver, second);
    let admission = guard(&core, first, catalog::AdmissionClass::Network);
    let permit = runtime.network_permit(&core, admission, SERVICE).unwrap();
    let lease = runtime
        .admit_mutation(&core, admission, SERVICE, MutationId([8; 16]))
        .unwrap();
    let activation_receipt = runtime.activation_receipt(a).unwrap();
    let pending = transition(
        &mut core,
        &mut disk,
        third,
        catalog::TransitionTarget::Enable,
    );
    let catalog_before = disk.snapshot.clone();
    let reserved = runtime.state().reserved_resources();
    let calls = driver.calls.len();

    let termination = runtime.begin_termination();
    // Losing/recreating the borrowed controller does not reopen this owner or
    // lose its retained mutation, resources, or termination capability.
    let mut runtime = Controller::new(&mut state);
    assert_eq!(runtime.begin_termination(), termination);
    assert!(runtime.state().is_terminating());
    assert_eq!(
        runtime.state().owner_tokens().collect::<Vec<_>>(),
        vec![a, b]
    );
    assert_eq!(runtime.state().reserved_resources(), reserved);
    assert_eq!(runtime.state().in_flight_mutations(first), 1);
    assert_eq!(driver.calls.len(), calls);
    assert_eq!(
        runtime.begin_activation(&core, pending, plan()),
        Err(Fault::Terminating)
    );
    assert_eq!(
        runtime.reconcile_activation(&core, pending, plan()),
        Err(Fault::Terminating)
    );
    assert_eq!(
        runtime.recover_quiescence(&core, pending, plan()),
        Err(Fault::Terminating)
    );
    assert_eq!(
        runtime.begin_quiescence(&core, pending),
        Err(Fault::Terminating)
    );
    assert_eq!(
        runtime.preflight(&core, &mut driver, a),
        Err(Error::Fault(Fault::Terminating))
    );
    assert_eq!(
        runtime.prepare_service(&core, &mut driver, a, SERVICE),
        Err(Error::Fault(Fault::Terminating))
    );
    assert_eq!(
        runtime.activate_service(&core, &mut driver, a, SERVICE),
        Err(Error::Fault(Fault::Terminating))
    );
    assert_eq!(runtime.finish_activation(&core, a), Err(Fault::Terminating));
    assert_eq!(
        runtime.confirm_activation(&core, activation_receipt, admission),
        Err(Fault::Terminating)
    );
    assert_eq!(
        runtime.admit_mutation(&core, admission, SERVICE, MutationId([9; 16])),
        Err(Fault::Terminating)
    );
    assert_eq!(
        runtime.network_permit(&core, admission, SERVICE),
        Err(Fault::Terminating)
    );
    assert_eq!(
        runtime.validate_network(&core, permit),
        Err(Fault::Terminating)
    );
    assert_eq!(driver.calls.len(), calls);

    runtime
        .fence_previous_workers(&core, &mut driver, a)
        .unwrap();
    assert_eq!(
        runtime.quiesce_service(&core, &mut driver, a, SERVICE),
        Err(Error::Fault(Fault::MutationsPending))
    );
    assert!(!runtime.termination_complete(termination).unwrap());
    runtime
        .complete_mutation(lease, Settlement::Committed)
        .unwrap();
    let receipt = finish_stop(&mut runtime, &core, &mut driver, a);
    assert_eq!(receipt.termination(), Some(termination));
    assert_eq!(
        runtime.confirm_quiescence(&core, receipt),
        Err(Fault::Terminating)
    );
    runtime.confirm_termination(termination, receipt).unwrap();
    assert_eq!(runtime.state().owned_identities(), 1);
    assert_eq!(
        runtime.state().reserved_resources(),
        plan().total().unwrap()
    );
    assert_eq!(runtime.phase(b).unwrap(), Phase::FencingPreviousWorkers);
    assert!(!runtime.termination_complete(termination).unwrap());
    assert_eq!(
        runtime.confirm_termination(termination, receipt),
        Err(Fault::StaleOwner)
    );
    let receipt = finish_stop(&mut runtime, &core, &mut driver, b);
    runtime.confirm_termination(termination, receipt).unwrap();
    assert!(runtime.termination_complete(termination).unwrap());
    assert_eq!(runtime.begin_termination(), termination);
    assert_eq!(
        runtime.network_permit(&core, admission, SERVICE),
        Err(Fault::Terminating)
    );
    assert_eq!(core.snapshot(), &catalog_before);
    assert_eq!(disk.snapshot, catalog_before);
    assert_eq!(driver.management_key, [1; 16]);
    assert_eq!(driver.management_destinations, 3);

    core.reload(&mut disk).unwrap();
    assert_eq!(
        core.identity(first).unwrap().desired,
        catalog::DesiredState::Enabled
    );
    assert!(matches!(
        core.recovery_action(first).unwrap(),
        Some(catalog::Action::Activate { .. })
    ));
    assert!(matches!(
        core.action(pending.ticket().operation.unwrap()).unwrap(),
        Some(catalog::Action::Activate { .. })
    ));
}

#[test]
fn partial_activation_pending_and_unavailable_cleanup_never_release_ownership() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let identity = create(&mut core, &mut disk, 1);
    let action = transition(
        &mut core,
        &mut disk,
        identity,
        catalog::TransitionTarget::Enable,
    );
    let mut runtime = Controller::new(&mut state);
    let owner = runtime.begin_activation(&core, action, plan()).unwrap();
    runtime
        .fence_previous_workers(&core, &mut driver, owner)
        .unwrap();
    runtime.preflight(&core, &mut driver, owner).unwrap();
    driver.pending = Some(Step::PrepareService);
    assert_eq!(
        runtime
            .prepare_service(&core, &mut driver, owner, SERVICE)
            .unwrap(),
        Progress::Pending
    );
    let old_scope = *driver.calls.last().unwrap();
    runtime.validate_scope(&core, old_scope).unwrap();
    let before = disk.snapshot.clone();
    let termination = runtime.begin_termination();
    assert_eq!(
        runtime.validate_scope(&core, old_scope),
        Err(Fault::Terminating)
    );
    let mut foreign = State::new([99; 16], budget()).unwrap();
    let foreign_termination = Controller::new(&mut foreign).begin_termination();
    assert_eq!(
        runtime.termination_complete(foreign_termination),
        Err(Fault::StaleEvidence)
    );

    driver.pending = Some(Step::FencePreviousWorkers);
    assert_eq!(
        runtime
            .fence_previous_workers(&core, &mut driver, owner)
            .unwrap(),
        Progress::Pending
    );
    let scope = *driver.calls.last().unwrap();
    runtime.validate_scope(&core, scope).unwrap();
    assert_eq!(scope.termination, Some(termination));
    let mut wrong = scope;
    wrong.termination = Some(foreign_termination);
    assert_eq!(
        runtime.validate_scope(&core, wrong),
        Err(Fault::StaleEvidence)
    );
    wrong = scope;
    wrong.generation += 1;
    assert_eq!(
        runtime.validate_scope(&core, wrong),
        Err(Fault::StaleEvidence)
    );
    driver.pending = None;
    driver.wrong_scope = true;
    assert_eq!(
        runtime.fence_previous_workers(&core, &mut driver, owner),
        Err(Error::Fault(Fault::StaleEvidence))
    );
    driver.wrong_scope = false;
    runtime
        .fence_previous_workers(&core, &mut driver, owner)
        .unwrap();
    assert_eq!(runtime.validate_scope(&core, scope), Err(Fault::WrongPhase));

    driver.error = Some(Step::QuiesceService);
    assert_eq!(
        runtime.quiesce_service(&core, &mut driver, owner, SERVICE),
        Err(Error::Driver(()))
    );
    driver.error = None;
    driver.pending = Some(Step::QuiesceService);
    assert_eq!(
        runtime
            .quiesce_service(&core, &mut driver, owner, SERVICE)
            .unwrap(),
        Progress::Pending
    );
    driver.pending = None;
    driver.quiescence.incoming_links = 1;
    assert_eq!(
        runtime.quiesce_service(&core, &mut driver, owner, SERVICE),
        Err(Error::Fault(Fault::InvalidEvidence))
    );
    assert_eq!(
        runtime.state().reserved_resources(),
        plan().total().unwrap()
    );
    assert!(!runtime.termination_complete(termination).unwrap());
    driver.quiescence = QuiescenceEvidence::COMPLETE;
    runtime
        .quiesce_service(&core, &mut driver, owner, SERVICE)
        .unwrap();
    driver.report = catalog::DependencyReport::empty();
    assert_eq!(
        runtime.finish_quiescence(&core, &mut driver, owner),
        Err(Error::Fault(Fault::InvalidEvidence))
    );
    driver.report = report();
    driver.pending = Some(Step::ReleaseIdentity);
    assert_eq!(
        runtime
            .finish_quiescence(&core, &mut driver, owner)
            .unwrap(),
        Progress::Pending
    );
    assert_eq!(
        runtime.state().reserved_resources(),
        plan().total().unwrap()
    );
    driver.pending = None;
    let Progress::Complete(receipt) = runtime
        .finish_quiescence(&core, &mut driver, owner)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        runtime.confirm_termination(foreign_termination, receipt),
        Err(Fault::StaleEvidence)
    );
    runtime.confirm_termination(termination, receipt).unwrap();
    assert!(runtime.termination_complete(termination).unwrap());
    assert_eq!(core.snapshot(), &before);
    assert_eq!(disk.snapshot, before);
}

#[test]
fn catalog_failure_cannot_prevent_own_cleanup_or_enable_fresh_dispatch() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let identity = create(&mut core, &mut disk, 1);
    let mut runtime = Controller::new(&mut state);
    let owner = running(&mut runtime, &mut core, &mut disk, &mut driver, identity);
    let admission = guard(&core, identity, catalog::AdmissionClass::Network);
    let before = disk.snapshot.clone();
    disk.fail = Some(false);
    assert!(core.checkpoint(&mut disk).is_err());
    assert!(core.is_poisoned());
    assert_eq!(
        runtime.network_permit(&core, admission, SERVICE),
        Err(Fault::Authority(catalog::Fault::Poisoned))
    );
    let termination = runtime.begin_termination();
    assert_eq!(
        runtime.network_permit(&core, admission, SERVICE),
        Err(Fault::Terminating)
    );
    let receipt = finish_stop(&mut runtime, &core, &mut driver, owner);
    runtime.confirm_termination(termination, receipt).unwrap();
    assert!(runtime.termination_complete(termination).unwrap());
    assert!(core.is_poisoned());
    assert_eq!(core.snapshot(), &before);
    assert_eq!(disk.snapshot, before);
}

#[test]
fn termination_retains_cleanup_only_debt_and_rejects_ordinary_receipts() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let first = create(&mut core, &mut disk, 1);
    let second = create(&mut core, &mut disk, 2);
    let third = create(&mut core, &mut disk, 3);
    let mut runtime = Controller::new(&mut state);
    let a = running(&mut runtime, &mut core, &mut disk, &mut driver, first);
    running(&mut runtime, &mut core, &mut disk, &mut driver, second);
    let action = transition(
        &mut core,
        &mut disk,
        third,
        catalog::TransitionTarget::Enable,
    );
    core.activation_failed(
        &mut disk,
        action.ticket(),
        catalog::ServiceFailure::CapacityExceeded,
    )
    .unwrap();
    let cleanup = core
        .action(action.ticket().operation.unwrap())
        .unwrap()
        .unwrap();
    let debt = runtime.recover_quiescence(&core, cleanup, plan()).unwrap();
    let ordinary = finish_stop(&mut runtime, &core, &mut driver, debt);
    assert_eq!(ordinary.termination(), None);
    let before = disk.snapshot.clone();
    let termination = runtime.begin_termination();
    assert_eq!(runtime.state().owner_tokens().count(), 3);
    assert!(runtime.state().cleanup_pending());
    assert_eq!(
        runtime.confirm_termination(termination, ordinary),
        Err(Fault::StaleEvidence)
    );
    let receipt = finish_stop(&mut runtime, &core, &mut driver, debt);
    runtime.confirm_termination(termination, receipt).unwrap();
    assert!(!runtime.state().cleanup_pending());
    assert_eq!(runtime.state().owned_identities(), 2);
    assert_eq!(runtime.phase(a).unwrap(), Phase::FencingPreviousWorkers);
    for owner in runtime.state().owner_tokens().collect::<Vec<_>>() {
        let receipt = finish_stop(&mut runtime, &core, &mut driver, owner);
        runtime.confirm_termination(termination, receipt).unwrap();
    }
    assert!(runtime.termination_complete(termination).unwrap());
    assert_eq!(core.snapshot(), &before);
    assert_eq!(disk.snapshot, before);
}

#[test]
fn empty_runtime_termination_is_repeatable_and_not_transferable() {
    let mut first = State::new([80; 16], budget()).unwrap();
    let mut second = State::new([81; 16], budget()).unwrap();
    let mut first = Controller::new(&mut first);
    let second = Controller::new(&mut second);
    let token = first.begin_termination();
    assert!(first.termination_complete(token).unwrap());
    assert_eq!(first.begin_termination(), token);
    assert_eq!(
        second.termination_complete(token),
        Err(Fault::StaleEvidence)
    );
}
