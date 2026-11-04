#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use prns_identities_core as catalog;
use prns_identities_runtime::*;

const SERVICE: catalog::ServiceId = catalog::ServiceId(31);
type Snapshot = catalog::Catalog<3, 32, 2>;
type Core<'a> = catalog::Coordinator<'a, 3, 32, 2>;
type State = RuntimeState<3, 2, 3>;

struct Disk {
    snapshot: Snapshot,
    fail: Option<bool>,
}
impl catalog::Storage<3, 32, 2> for Disk {
    type Error = ();
    fn load(&mut self, target: &mut Snapshot) -> Result<(), ()> {
        target.copy_from(&self.snapshot);
        Ok(())
    }
    fn compare_and_store(&mut self, expected: u64, next: &Snapshot) -> Result<(), ()> {
        assert_eq!(self.snapshot.revision(), expected);
        let fail = self.fail.take();
        if fail == Some(false) {
            return Err(());
        }
        self.snapshot.copy_from(next);
        if fail == Some(true) {
            Err(())
        } else {
            Ok(())
        }
    }
}

fn fixtures() -> (Disk, Snapshot, Snapshot, State, Driver) {
    (
        Disk {
            snapshot: Snapshot::new([1; 16]),
            fail: None,
        },
        Snapshot::new([1; 16]),
        Snapshot::new([1; 16]),
        State::new([7; 16], budget()).unwrap(),
        Driver::default(),
    )
}

fn budget() -> Budget {
    Budget {
        capacity: Resources {
            held_keys: 5,
            destinations: 5,
            request_handlers: 4,
            links: 6,
            queue_slots: 6,
        },
        infrastructure_reserve: Resources {
            held_keys: 3,
            destinations: 3,
            request_handlers: 2,
            links: 2,
            queue_slots: 2,
        },
    }
}

fn plan() -> ActivationPlan<2> {
    ActivationPlan::new(
        Resources {
            held_keys: 1,
            ..Resources::ZERO
        },
        &[ServicePlan {
            service: SERVICE,
            resources: Resources {
                destinations: 1,
                request_handlers: 1,
                links: 2,
                queue_slots: 2,
                ..Resources::ZERO
            },
            requires_receive_state: true,
            capabilities: Capabilities::QUALIFIED,
        }],
    )
    .unwrap()
}

fn report() -> catalog::DependencyReport<2> {
    catalog::DependencyReport::new(&[catalog::ServiceDependency::clear(SERVICE, 1)], 1).unwrap()
}

fn create(core: &mut Core<'_>, disk: &mut Disk, number: u8) -> catalog::LocalIdentityId {
    let command = catalog::Command::Create(catalog::CreateRequest {
        source: catalog::KeySource::Generate,
        metadata: catalog::Metadata::new("Sensor", None).unwrap(),
        services: catalog::ServiceSet::new(&[SERVICE]).unwrap(),
    });
    let token = core
        .reserve(disk, command, catalog::CommandDigest([number; 32]))
        .unwrap();
    core.begin(disk, token, &command).unwrap();
    let action = core.action(token).unwrap().unwrap();
    let catalog::Action::PublishKey {
        key_reference,
        ticket,
        ..
    } = action
    else {
        panic!()
    };
    let result = core
        .ack_key(
            disk,
            ticket,
            catalog::VerifiedKey {
                key_reference,
                public_hash: catalog::PublicIdentityHash([number; 16]),
            },
        )
        .unwrap();
    let catalog::OperationResult::Created { identity } = result else {
        panic!()
    };
    identity
}

fn transition(
    core: &mut Core<'_>,
    disk: &mut Disk,
    id: catalog::LocalIdentityId,
    target: catalog::TransitionTarget,
) -> catalog::Action<2> {
    let prepared = core.prepare(id, target, report()).unwrap();
    let token = if target == catalog::TransitionTarget::Enable {
        let command = catalog::Command::Transition(prepared);
        let token = core
            .reserve(disk, command, catalog::CommandDigest([4; 32]))
            .unwrap();
        core.begin(disk, token, &command).unwrap();
        token
    } else {
        core.apply_stop(disk, prepared, catalog::CommandDigest([5; 32]))
            .unwrap()
    };
    core.action(token).unwrap().unwrap()
}

fn guard(
    core: &Core<'_>,
    id: catalog::LocalIdentityId,
    class: catalog::AdmissionClass,
) -> catalog::AdmissionGuard {
    core.admit(id, core.identity(id).unwrap().public_hash, class)
        .unwrap()
}

fn activate_to_receipt<const I: usize>(
    runtime: &mut Controller<'_, I, 2, 3>,
    core: &Core<'_>,
    driver: &mut Driver,
    action: catalog::Action<2>,
) -> ActivationReceipt {
    let owner = runtime.begin_activation(core, action, plan()).unwrap();
    finish_activation(runtime, core, driver, owner)
}

fn finish_activation<const I: usize>(
    runtime: &mut Controller<'_, I, 2, 3>,
    core: &Core<'_>,
    driver: &mut Driver,
    owner: OwnerToken,
) -> ActivationReceipt {
    assert_eq!(
        runtime.fence_previous_workers(core, driver, owner).unwrap(),
        Progress::Complete(())
    );
    assert_eq!(
        runtime.preflight(core, driver, owner).unwrap(),
        Progress::Complete(())
    );
    assert_eq!(
        runtime
            .prepare_service(core, driver, owner, SERVICE)
            .unwrap(),
        Progress::Complete(())
    );
    assert_eq!(
        runtime
            .activate_service(core, driver, owner, SERVICE)
            .unwrap(),
        Progress::Complete(())
    );
    runtime.finish_activation(core, owner).unwrap()
}

fn running<const I: usize>(
    runtime: &mut Controller<'_, I, 2, 3>,
    core: &mut Core<'_>,
    disk: &mut Disk,
    driver: &mut Driver,
    id: catalog::LocalIdentityId,
) -> OwnerToken {
    let action = transition(core, disk, id, catalog::TransitionTarget::Enable);
    let receipt = activate_to_receipt(runtime, core, driver, action);
    core.ack_activated(disk, receipt.ticket()).unwrap();
    runtime
        .confirm_activation(
            core,
            receipt,
            guard(core, id, catalog::AdmissionClass::Network),
        )
        .unwrap();
    receipt.owner()
}

fn finish_stop<const I: usize>(
    runtime: &mut Controller<'_, I, 2, 3>,
    core: &Core<'_>,
    driver: &mut Driver,
    owner: OwnerToken,
) -> QuiescenceReceipt<2> {
    if runtime.phase(owner).unwrap() == Phase::FencingPreviousWorkers {
        assert_eq!(
            runtime.fence_previous_workers(core, driver, owner).unwrap(),
            Progress::Complete(())
        );
    }
    assert_eq!(
        runtime
            .quiesce_service(core, driver, owner, SERVICE)
            .unwrap(),
        Progress::Complete(())
    );
    let Progress::Complete(receipt) = runtime.finish_quiescence(core, driver, owner).unwrap()
    else {
        panic!()
    };
    receipt
}

struct Driver {
    calls: Vec<Scope>,
    pending: Option<Step>,
    error: Option<Step>,
    wrong_scope: bool,
    prepared: PreparedService,
    quiescence: QuiescenceEvidence,
    report: catalog::DependencyReport<2>,
    shared_preserved: bool,
    management_key: [u8; 16],
    management_destinations: u32,
}
impl Default for Driver {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            pending: None,
            error: None,
            wrong_scope: false,
            prepared: PreparedService {
                storage_ready: true,
                receive_state_restored: true,
            },
            quiescence: QuiescenceEvidence::COMPLETE,
            report: report(),
            shared_preserved: true,
            management_key: [1; 16],
            management_destinations: 3,
        }
    }
}
impl Driver {
    fn reply<T>(&mut self, mut scope: Scope, value: T) -> Result<Progress<Evidence<T>>, ()> {
        self.calls.push(scope);
        if self.error == Some(scope.step) {
            return Err(());
        }
        if self.pending == Some(scope.step) {
            return Ok(Progress::Pending);
        }
        if self.wrong_scope {
            scope.generation += 1;
        }
        Ok(Progress::Complete(Evidence { scope, value }))
    }
}
impl ScopedDriver<2> for Driver {
    type Error = ();
    fn fence_previous_workers(&mut self, scope: Scope) -> Result<Progress<Evidence<bool>>, ()> {
        self.reply(scope, true)
    }
    fn preflight(
        &mut self,
        scope: Scope,
        plan: &ActivationPlan<2>,
    ) -> Result<Progress<Evidence<Resources>>, ()> {
        self.reply(scope, plan.total().unwrap())
    }
    fn prepare_service(
        &mut self,
        scope: Scope,
        _key: catalog::KeyReference,
        _plan: ServicePlan,
    ) -> Result<Progress<Evidence<PreparedService>>, ()> {
        self.reply(scope, self.prepared)
    }
    fn activate_service(
        &mut self,
        scope: Scope,
        _plan: ServicePlan,
    ) -> Result<Progress<Evidence<()>>, ()> {
        self.reply(scope, ())
    }
    fn quiesce_service(
        &mut self,
        scope: Scope,
        _plan: ServicePlan,
    ) -> Result<Progress<Evidence<QuiescedService>>, ()> {
        self.reply(
            scope,
            QuiescedService {
                evidence: self.quiescence,
                dependencies: self.report.entries[0].unwrap(),
            },
        )
    }
    fn release_identity(
        &mut self,
        scope: Scope,
    ) -> Result<Progress<Evidence<ReleasedIdentity<2>>>, ()> {
        self.reply(
            scope,
            ReleasedIdentity {
                owned_reservations_released: true,
                shared_owners_preserved: self.shared_preserved,
                report: self.report,
            },
        )
    }
}

#[test]
fn preflight_reserves_peak_management_headroom_and_two_distinct_owners() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let first = create(&mut core, &mut disk, 1);
    let second = create(&mut core, &mut disk, 2);
    let third = create(&mut core, &mut disk, 3);
    let mut runtime = Controller::new(&mut state);
    let a = running(&mut runtime, &mut core, &mut disk, &mut driver, first);
    let b = running(&mut runtime, &mut core, &mut disk, &mut driver, second);
    let before = driver.calls.len();
    let action = transition(
        &mut core,
        &mut disk,
        third,
        catalog::TransitionTarget::Enable,
    );
    assert_eq!(
        runtime.begin_activation(&core, action, plan()),
        Err(Fault::CapacityExceeded)
    );
    assert_eq!(
        runtime.state().reserved_resources(),
        budget().application_capacity().unwrap()
    );
    assert_eq!(runtime.state().owned_identities(), 2);
    assert_eq!(driver.calls.len(), before);
    assert_ne!(a, b);
    assert_eq!(driver.management_key, [1; 16]); // Shares hash with persona 1, not ownership.
    assert_eq!(driver.management_destinations, 3);
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
    let recovery = runtime.recover_quiescence(&core, cleanup, plan()).unwrap();
    assert!(runtime.state().cleanup_pending());
    assert_eq!(
        runtime.state().reserved_resources(),
        budget().application_capacity().unwrap()
    );
    let receipt = finish_stop(&mut runtime, &core, &mut driver, recovery);
    core.ack_quiesced(&mut disk, receipt.ticket(), receipt.report())
        .unwrap();
    runtime.confirm_quiescence(&core, receipt).unwrap();
    assert!(!runtime.state().cleanup_pending());
    assert_eq!(runtime.phase(a).unwrap(), Phase::Running);
    assert_eq!(runtime.phase(b).unwrap(), Phase::Running);
}

#[test]
fn forged_action_and_unqualified_plan_never_reserve_or_dispatch() {
    let (mut disk, mut work, mut stage, mut state, driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let id = create(&mut core, &mut disk, 1);
    let action = transition(&mut core, &mut disk, id, catalog::TransitionTarget::Enable);
    let mut runtime = Controller::new(&mut state);
    for kind in 0..3 {
        let mut forged = action;
        let catalog::Action::Activate {
            identity,
            key_reference,
            services,
            ..
        } = &mut forged
        else {
            panic!()
        };
        match kind {
            0 => identity.public_hash = catalog::PublicIdentityHash([9; 16]),
            1 => *key_reference = catalog::KeyReference([9; 32]),
            _ => *services = catalog::ServiceSet::new(&[catalog::ServiceId(99)]).unwrap(),
        }
        assert_eq!(
            runtime.begin_activation(&core, forged, plan()),
            Err(Fault::StaleEffect)
        );
    }
    let mut unqualified = plan();
    unqualified.services[0]
        .as_mut()
        .unwrap()
        .capabilities
        .durable_receive_state = false;
    assert_eq!(
        runtime.begin_activation(&core, action, unqualified),
        Err(Fault::UnsupportedService)
    );
    assert_eq!(runtime.state().reserved_resources(), Resources::ZERO);
    assert!(driver.calls.is_empty());
}

#[test]
fn offline_durable_mutation_requires_prepared_storage_but_not_network_readiness() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let id = create(&mut core, &mut disk, 1);
    let mut runtime = Controller::new(&mut state);
    let owner = running(&mut runtime, &mut core, &mut disk, &mut driver, id);
    core.reload(&mut disk).unwrap();
    let action = core.recovery_action(id).unwrap().unwrap();
    assert_eq!(
        runtime.begin_activation(&core, action, plan()),
        Err(Fault::OwnerStillActive)
    );
    assert_eq!(
        runtime.reconcile_activation(&core, action, plan()).unwrap(),
        owner
    );
    runtime
        .fence_previous_workers(&core, &mut driver, owner)
        .unwrap();
    runtime.preflight(&core, &mut driver, owner).unwrap();
    let admission = guard(&core, id, catalog::AdmissionClass::ServiceMutation);
    assert_eq!(
        runtime.admit_mutation(&core, admission, SERVICE, MutationId([1; 16])),
        Err(Fault::StorageNotReady)
    );
    driver.prepared.receive_state_restored = false;
    assert_eq!(
        runtime.prepare_service(&core, &mut driver, owner, SERVICE),
        Err(Error::Fault(Fault::InvalidEvidence))
    );
    driver.prepared.receive_state_restored = true;
    runtime
        .prepare_service(&core, &mut driver, owner, SERVICE)
        .unwrap();
    let lease = runtime
        .admit_mutation(&core, admission, SERVICE, MutationId([1; 16]))
        .unwrap();
    assert_eq!(
        runtime.admit_mutation(&core, admission, SERVICE, MutationId([1; 16])),
        Err(Fault::DuplicateMutation)
    );
    assert!(core
        .admit(
            id,
            core.identity(id).unwrap().public_hash,
            catalog::AdmissionClass::Network
        )
        .is_err());
    assert_eq!(runtime.state().in_flight_mutations(id), 1);
    runtime
        .complete_mutation(lease, Settlement::Committed)
        .unwrap();
    assert_eq!(
        runtime.complete_mutation(lease, Settlement::Committed),
        Err(Fault::StaleLease)
    );
}

#[test]
fn stopping_preserves_accepted_mutations_and_other_owners_across_controller_recreation() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let first = create(&mut core, &mut disk, 1);
    let second = create(&mut core, &mut disk, 2);
    let (a, b, lease, permit) = {
        let mut runtime = Controller::new(&mut state);
        let a = running(&mut runtime, &mut core, &mut disk, &mut driver, first);
        let b = running(&mut runtime, &mut core, &mut disk, &mut driver, second);
        let admission = guard(&core, first, catalog::AdmissionClass::ServiceMutation);
        let lease = runtime
            .admit_mutation(&core, admission, SERVICE, MutationId([7; 16]))
            .unwrap();
        let permit = runtime
            .network_permit(
                &core,
                guard(&core, first, catalog::AdmissionClass::Network),
                SERVICE,
            )
            .unwrap();
        (a, b, lease, permit)
    };
    let mut runtime = Controller::new(&mut state);
    assert_eq!(runtime.state().in_flight_mutations(first), 1);
    let action = transition(
        &mut core,
        &mut disk,
        first,
        catalog::TransitionTarget::Retire,
    );
    assert_eq!(runtime.begin_quiescence(&core, action).unwrap(), a);
    assert!(runtime.validate_network(&core, permit).is_err());
    let before = driver.calls.len();
    assert_eq!(
        runtime.quiesce_service(&core, &mut driver, a, SERVICE),
        Err(Error::Fault(Fault::MutationsPending))
    );
    assert_eq!(driver.calls.len(), before);
    runtime.validate_settlement(lease).unwrap();
    runtime
        .complete_mutation(lease, Settlement::Committed)
        .unwrap();
    assert_eq!(runtime.validate_settlement(lease), Err(Fault::StaleLease));
    // An admitted write changed the authoritative retirement obligations.
    driver.report.entries[0].as_mut().unwrap().revision = 2;
    driver.report.entries[0].as_mut().unwrap().pending_messages = 1;
    let receipt = finish_stop(&mut runtime, &core, &mut driver, a);
    assert!(matches!(
        core.ack_quiesced(&mut disk, receipt.ticket(), receipt.report())
            .unwrap(),
        catalog::OperationResult::RetirementBlocked { .. }
    ));
    runtime.confirm_quiescence(&core, receipt).unwrap();
    assert_eq!(runtime.phase(b).unwrap(), Phase::Running);
    assert_eq!(
        runtime.state().reserved_resources(),
        plan().total().unwrap()
    );
    assert_eq!(
        core.identity(first).unwrap().desired,
        catalog::DesiredState::Paused
    );
    assert_eq!(driver.management_destinations, 3);
}

#[test]
fn quiescence_needs_actual_link_queue_ratchet_and_key_completion() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let id = create(&mut core, &mut disk, 1);
    let mut runtime = Controller::new(&mut state);
    let owner = running(&mut runtime, &mut core, &mut disk, &mut driver, id);
    let action = transition(&mut core, &mut disk, id, catalog::TransitionTarget::Pause);
    runtime.begin_quiescence(&core, action).unwrap();
    for variant in 0..10 {
        driver.quiescence = QuiescenceEvidence::COMPLETE;
        match variant {
            0 => driver.quiescence.in_flight_mutations = 1,
            1 => driver.quiescence.incoming_links = 1,
            2 => driver.quiescence.outgoing_links = 1,
            3 => driver.quiescence.queued_egress = 1,
            4 => driver.quiescence.scheduled_announces = 1,
            5 => driver.quiescence.registered_destinations = 1,
            6 => driver.quiescence.request_handlers = 1,
            7 => driver.quiescence.receive_state_durable = false,
            8 => driver.quiescence.key_handles_reconciled = false,
            _ => driver.quiescence.admissions_closed = false,
        }
        assert_eq!(
            runtime.quiesce_service(&core, &mut driver, owner, SERVICE),
            Err(Error::Fault(Fault::InvalidEvidence))
        );
        assert_eq!(
            runtime.state().reserved_resources(),
            plan().total().unwrap()
        );
    }
    driver.quiescence = QuiescenceEvidence::COMPLETE;
    driver.wrong_scope = true;
    assert_eq!(
        runtime.quiesce_service(&core, &mut driver, owner, SERVICE),
        Err(Error::Fault(Fault::StaleEvidence))
    );
    driver.wrong_scope = false;
    runtime
        .quiesce_service(&core, &mut driver, owner, SERVICE)
        .unwrap();
    driver.shared_preserved = false;
    assert_eq!(
        runtime.finish_quiescence(&core, &mut driver, owner),
        Err(Error::Fault(Fault::InvalidEvidence))
    );
    driver.shared_preserved = true;
    let Progress::Complete(receipt) = runtime
        .finish_quiescence(&core, &mut driver, owner)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(runtime.state().reserved_resources(), Resources::ZERO);
    assert!(runtime.confirm_quiescence(&core, receipt).is_err());
    core.ack_quiesced(&mut disk, receipt.ticket(), receipt.report())
        .unwrap();
    runtime.confirm_quiescence(&core, receipt).unwrap();
}

#[test]
fn incomplete_previous_worker_fence_cannot_be_skipped_by_activation_cleanup() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let id = create(&mut core, &mut disk, 1);
    let action = transition(&mut core, &mut disk, id, catalog::TransitionTarget::Enable);
    let mut runtime = Controller::new(&mut state);
    let owner = runtime.begin_activation(&core, action, plan()).unwrap();
    driver.pending = Some(Step::FencePreviousWorkers);
    assert_eq!(
        runtime
            .fence_previous_workers(&core, &mut driver, owner)
            .unwrap(),
        Progress::Pending
    );
    core.activation_failed(
        &mut disk,
        action.ticket(),
        catalog::ServiceFailure::Unavailable(9),
    )
    .unwrap();
    let cleanup = core
        .action(action.ticket().operation.unwrap())
        .unwrap()
        .unwrap();
    runtime.begin_quiescence(&core, cleanup).unwrap();
    assert_eq!(runtime.phase(owner).unwrap(), Phase::FencingPreviousWorkers);
    assert_eq!(
        runtime.quiesce_service(&core, &mut driver, owner, SERVICE),
        Err(Error::Fault(Fault::WrongPhase))
    );
    driver.pending = None;
    let receipt = finish_stop(&mut runtime, &core, &mut driver, owner);
    assert!(matches!(
        core.ack_quiesced(&mut disk, receipt.ticket(), receipt.report())
            .unwrap(),
        catalog::OperationResult::ActivationFailed { .. }
    ));
    runtime.confirm_quiescence(&core, receipt).unwrap();
}

#[test]
fn uncertain_activation_ack_reconciles_retained_owner_without_losing_other_resources() {
    for committed in [false, true] {
        let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
        let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
        let first = create(&mut core, &mut disk, 1);
        let second = create(&mut core, &mut disk, 2);
        let mut runtime = Controller::new(&mut state);
        let other = running(&mut runtime, &mut core, &mut disk, &mut driver, second);
        let action = transition(
            &mut core,
            &mut disk,
            first,
            catalog::TransitionTarget::Enable,
        );
        let old = activate_to_receipt(&mut runtime, &core, &mut driver, action);
        disk.fail = Some(committed);
        assert!(core.ack_activated(&mut disk, old.ticket()).is_err());
        core.reload(&mut disk).unwrap();
        let action = core.recovery_action(first).unwrap().unwrap();
        assert_eq!(
            runtime.begin_activation(&core, action, plan()),
            Err(Fault::OwnerStillActive)
        );
        let owner = runtime.reconcile_activation(&core, action, plan()).unwrap();
        assert_eq!(owner, old.owner());
        assert_eq!(runtime.phase(other).unwrap(), Phase::Running);
        assert_eq!(
            runtime.state().reserved_resources(),
            budget().application_capacity().unwrap()
        );
        let receipt = finish_activation(&mut runtime, &core, &mut driver, owner);
        core.ack_activated(&mut disk, receipt.ticket()).unwrap();
        let current = guard(&core, first, catalog::AdmissionClass::Network);
        assert_eq!(
            runtime.confirm_activation(&core, old, current),
            Err(Fault::StaleEvidence)
        );
        runtime.confirm_activation(&core, receipt, current).unwrap();
    }
}

#[test]
fn uncertain_stop_ack_reconciles_both_disk_outcomes_without_resetting_state() {
    for committed in [false, true] {
        let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
        let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
        let first = create(&mut core, &mut disk, 1);
        let second = create(&mut core, &mut disk, 2);
        let mut runtime = Controller::new(&mut state);
        let owner = running(&mut runtime, &mut core, &mut disk, &mut driver, first);
        let other = running(&mut runtime, &mut core, &mut disk, &mut driver, second);
        let action = transition(
            &mut core,
            &mut disk,
            first,
            catalog::TransitionTarget::Pause,
        );
        runtime.begin_quiescence(&core, action).unwrap();
        let mut receipt = finish_stop(&mut runtime, &core, &mut driver, owner);
        disk.fail = Some(committed);
        assert!(core
            .ack_quiesced(&mut disk, receipt.ticket(), receipt.report())
            .is_err());
        core.reload(&mut disk).unwrap();
        if !committed {
            let fresh = core.recovery_action(first).unwrap().unwrap();
            runtime.begin_quiescence(&core, fresh).unwrap();
            assert_eq!(runtime.phase(owner).unwrap(), Phase::FencingPreviousWorkers);
            receipt = finish_stop(&mut runtime, &core, &mut driver, owner);
            core.ack_quiesced(&mut disk, receipt.ticket(), receipt.report())
                .unwrap();
        }
        runtime.confirm_quiescence(&core, receipt).unwrap();
        assert_eq!(runtime.phase(other).unwrap(), Phase::Running);
        assert_eq!(
            runtime.state().reserved_resources(),
            plan().total().unwrap()
        );
        let new_owner = running(&mut runtime, &mut core, &mut disk, &mut driver, first);
        assert_ne!(new_owner, owner);
        assert_eq!(runtime.state().owned_identities(), 2);
    }
}

#[test]
fn new_session_reconciliation_keeps_old_leases_and_requires_drain_before_restore() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let id = create(&mut core, &mut disk, 1);
    let mut runtime = Controller::new(&mut state);
    let owner = running(&mut runtime, &mut core, &mut disk, &mut driver, id);
    let lease = runtime
        .admit_mutation(
            &core,
            guard(&core, id, catalog::AdmissionClass::ServiceMutation),
            SERVICE,
            MutationId([1; 16]),
        )
        .unwrap();
    let permit = runtime
        .network_permit(
            &core,
            guard(&core, id, catalog::AdmissionClass::Network),
            SERVICE,
        )
        .unwrap();
    core.reload(&mut disk).unwrap();
    let action = core.recovery_action(id).unwrap().unwrap();
    runtime.reconcile_activation(&core, action, plan()).unwrap();
    assert!(runtime.validate_network(&core, permit).is_err());
    runtime
        .fence_previous_workers(&core, &mut driver, owner)
        .unwrap();
    assert_eq!(
        runtime.preflight(&core, &mut driver, owner),
        Err(Error::Fault(Fault::MutationsPending))
    );
    runtime.validate_settlement(lease).unwrap();
    runtime
        .complete_mutation(lease, Settlement::Rejected)
        .unwrap();
    assert_eq!(
        runtime.preflight(&core, &mut driver, owner).unwrap(),
        Progress::Complete(())
    );
}

#[test]
fn fresh_runtime_recovery_requires_driver_worker_reconciliation_and_complete_barriers() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let id = create(&mut core, &mut disk, 1);
    let action = transition(&mut core, &mut disk, id, catalog::TransitionTarget::Enable);
    core.activation_failed(
        &mut disk,
        action.ticket(),
        catalog::ServiceFailure::StorageUnavailable,
    )
    .unwrap();
    core.reload(&mut disk).unwrap();
    let cleanup = core.recovery_action(id).unwrap().unwrap();
    let mut runtime = Controller::new(&mut state);
    let owner = runtime.recover_quiescence(&core, cleanup, plan()).unwrap();
    assert_eq!(
        runtime.quiesce_service(&core, &mut driver, owner, SERVICE),
        Err(Error::Fault(Fault::WrongPhase))
    );
    driver.error = Some(Step::FencePreviousWorkers);
    assert_eq!(
        runtime.fence_previous_workers(&core, &mut driver, owner),
        Err(Error::Driver(()))
    );
    assert_eq!(runtime.state().reserved_resources(), Resources::ZERO);
    assert!(runtime.state().cleanup_pending());
    driver.error = None;
    let receipt = finish_stop(&mut runtime, &core, &mut driver, owner);
    core.ack_quiesced(&mut disk, receipt.ticket(), receipt.report())
        .unwrap();
    runtime.confirm_quiescence(&core, receipt).unwrap();
    assert_eq!(runtime.state().owned_identities(), 0);
}

#[test]
fn bounded_leases_do_not_evict_accepted_work() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let id = create(&mut core, &mut disk, 1);
    let mut runtime = Controller::new(&mut state);
    running(&mut runtime, &mut core, &mut disk, &mut driver, id);
    let admission = guard(&core, id, catalog::AdmissionClass::ServiceMutation);
    let mut leases = [None; 3];
    for (index, lease) in leases.iter_mut().enumerate() {
        *lease = Some(
            runtime
                .admit_mutation(
                    &core,
                    admission,
                    SERVICE,
                    MutationId([u8::try_from(index).unwrap(); 16]),
                )
                .unwrap(),
        );
    }
    assert_eq!(
        runtime.admit_mutation(&core, admission, SERVICE, MutationId([9; 16])),
        Err(Fault::CapacityExceeded)
    );
    for lease in leases.into_iter().flatten() {
        runtime.validate_settlement(lease).unwrap();
    }
}

#[test]
fn repeated_enable_reconciles_current_owner_without_replaying_prior_generation() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let id = create(&mut core, &mut disk, 1);
    let mut runtime = Controller::new(&mut state);
    let owner = running(&mut runtime, &mut core, &mut disk, &mut driver, id);
    let old = runtime
        .network_permit(
            &core,
            guard(&core, id, catalog::AdmissionClass::Network),
            SERVICE,
        )
        .unwrap();
    let action = transition(&mut core, &mut disk, id, catalog::TransitionTarget::Enable);
    assert_eq!(
        runtime.reconcile_activation(&core, action, plan()).unwrap(),
        owner
    );
    assert!(runtime.validate_network(&core, old).is_err());
    let receipt = finish_activation(&mut runtime, &core, &mut driver, owner);
    core.ack_activated(&mut disk, receipt.ticket()).unwrap();
    runtime
        .confirm_activation(
            &core,
            receipt,
            guard(&core, id, catalog::AdmissionClass::Network),
        )
        .unwrap();
    let new = runtime
        .network_permit(
            &core,
            guard(&core, id, catalog::AdmissionClass::Network),
            SERVICE,
        )
        .unwrap();
    assert_eq!(old.owner(), new.owner());
    assert!(old.generation() < new.generation());
    assert_eq!(
        runtime.state().reserved_resources(),
        plan().total().unwrap()
    );
}

#[test]
fn uncertain_restore_cleanup_ack_reconciles_both_disk_outcomes() {
    for committed in [false, true] {
        let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
        let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
        let id = create(&mut core, &mut disk, 1);
        let mut runtime = Controller::new(&mut state);
        let owner = running(&mut runtime, &mut core, &mut disk, &mut driver, id);
        core.reload(&mut disk).unwrap();
        let action = core.recovery_action(id).unwrap().unwrap();
        runtime.reconcile_activation(&core, action, plan()).unwrap();
        runtime
            .fence_previous_workers(&core, &mut driver, owner)
            .unwrap();
        runtime.preflight(&core, &mut driver, owner).unwrap();
        core.activation_failed(
            &mut disk,
            action.ticket(),
            catalog::ServiceFailure::StorageUnavailable,
        )
        .unwrap();
        let cleanup = core.recovery_action(id).unwrap().unwrap();
        runtime.begin_quiescence(&core, cleanup).unwrap();
        let mut receipt = finish_stop(&mut runtime, &core, &mut driver, owner);
        disk.fail = Some(committed);
        assert!(core
            .ack_recovery_quiesced(&mut disk, receipt.ticket(), receipt.report())
            .is_err());
        core.reload(&mut disk).unwrap();
        if !committed {
            runtime
                .begin_quiescence(&core, core.recovery_action(id).unwrap().unwrap())
                .unwrap();
            receipt = finish_stop(&mut runtime, &core, &mut driver, owner);
            core.ack_recovery_quiesced(&mut disk, receipt.ticket(), receipt.report())
                .unwrap();
        }
        runtime.confirm_quiescence(&core, receipt).unwrap();
        assert_eq!(runtime.state().owned_identities(), 0);
        assert_eq!(
            core.identity(id).unwrap().desired,
            catalog::DesiredState::Enabled
        );
        let receipt = activate_to_receipt(
            &mut runtime,
            &core,
            &mut driver,
            core.recovery_action(id).unwrap().unwrap(),
        );
        core.ack_activated(&mut disk, receipt.ticket()).unwrap();
        runtime
            .confirm_activation(
                &core,
                receipt,
                guard(&core, id, catalog::AdmissionClass::Network),
            )
            .unwrap();
    }
}

#[test]
fn driver_errors_and_pending_steps_retain_all_reserved_capacity() {
    for step in [
        Step::FencePreviousWorkers,
        Step::Preflight,
        Step::PrepareService,
        Step::ActivateService,
    ] {
        let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
        let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
        let id = create(&mut core, &mut disk, 1);
        let action = transition(&mut core, &mut disk, id, catalog::TransitionTarget::Enable);
        let mut runtime = Controller::new(&mut state);
        let owner = runtime.begin_activation(&core, action, plan()).unwrap();
        if step != Step::FencePreviousWorkers {
            runtime
                .fence_previous_workers(&core, &mut driver, owner)
                .unwrap();
        }
        if matches!(step, Step::PrepareService | Step::ActivateService) {
            runtime.preflight(&core, &mut driver, owner).unwrap();
        }
        if step == Step::ActivateService {
            runtime
                .prepare_service(&core, &mut driver, owner, SERVICE)
                .unwrap();
        }
        for pending in [true, false] {
            driver.pending = pending.then_some(step);
            driver.error = (!pending).then_some(step);
            let result = match step {
                Step::FencePreviousWorkers => {
                    runtime.fence_previous_workers(&core, &mut driver, owner)
                }
                Step::Preflight => runtime.preflight(&core, &mut driver, owner),
                Step::PrepareService => runtime.prepare_service(&core, &mut driver, owner, SERVICE),
                Step::ActivateService => {
                    runtime.activate_service(&core, &mut driver, owner, SERVICE)
                }
                _ => panic!(),
            };
            if pending {
                assert_eq!(result, Ok(Progress::Pending));
            } else {
                assert_eq!(result, Err(Error::Driver(())));
            }
            assert_eq!(
                runtime.state().reserved_resources(),
                plan().total().unwrap()
            );
            assert!(runtime.finish_activation(&core, owner).is_err());
        }
    }
}

#[test]
fn full_active_owner_table_still_has_one_cleanup_only_slot() {
    let (mut disk, mut work, mut stage, _, mut driver) = fixtures();
    let mut state = RuntimeState::<1, 2, 3>::new([3; 16], budget()).unwrap();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let first = create(&mut core, &mut disk, 1);
    let second = create(&mut core, &mut disk, 2);
    let third = create(&mut core, &mut disk, 3);
    let mut runtime = Controller::new(&mut state);
    let owner = running(&mut runtime, &mut core, &mut disk, &mut driver, first);
    let action = transition(
        &mut core,
        &mut disk,
        second,
        catalog::TransitionTarget::Enable,
    );
    assert_eq!(
        runtime.begin_activation(&core, action, plan()),
        Err(Fault::CapacityExceeded)
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
    let recovery = runtime.recover_quiescence(&core, cleanup, plan()).unwrap();
    assert_eq!(
        runtime.state().reserved_resources(),
        plan().total().unwrap()
    );
    assert_eq!(runtime.state().owned_identities(), 2);
    assert_eq!(runtime.state().in_flight_mutations(first), 0);
    let third_action = transition(
        &mut core,
        &mut disk,
        third,
        catalog::TransitionTarget::Enable,
    );
    assert_eq!(
        runtime.begin_activation(&core, third_action, plan()),
        Err(Fault::Busy)
    );
    // Existing scoped traffic remains admitted; cleanup cannot steal its budget.
    runtime
        .network_permit(
            &core,
            guard(&core, first, catalog::AdmissionClass::Network),
            SERVICE,
        )
        .unwrap();
    let receipt = finish_stop(&mut runtime, &core, &mut driver, recovery);
    core.ack_quiesced(&mut disk, receipt.ticket(), receipt.report())
        .unwrap();
    runtime.confirm_quiescence(&core, receipt).unwrap();
    assert_eq!(runtime.phase(owner).unwrap(), Phase::Running);
    assert!(!runtime.state().cleanup_pending());
}

#[derive(Default)]
struct Initializer {
    prepared: Option<(
        catalog::LocalIdentityId,
        catalog::ServiceId,
        catalog::OperationToken,
    )>,
    calls: usize,
    unavailable: bool,
    wrong_scope: bool,
}
impl configuration::ServiceInitializer for Initializer {
    type Error = ();
    fn initialize_service(
        &mut self,
        ticket: catalog::EffectTicket,
        identity: catalog::IdentityGuard,
        _key: catalog::KeyReference,
        service: catalog::ServiceId,
    ) -> Result<catalog::InitializedService, ()> {
        self.calls += 1;
        if self.unavailable {
            return Err(());
        }
        let binding = (identity.id, service, ticket.operation.unwrap());
        if self.prepared.is_some_and(|old| old != binding) {
            return Err(());
        }
        self.prepared = Some(binding);
        let mut ticket = ticket;
        if self.wrong_scope {
            ticket.session += 1;
        }
        Ok(catalog::InitializedService { ticket, service })
    }
}

#[test]
fn configuration_drain_confirms_before_explicit_preparation_and_reopen_never_initializes() {
    const ADDED: catalog::ServiceId = catalog::ServiceId(802);
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let id = create(&mut core, &mut disk, 1);
    let prepared = core
        .prepare(id, catalog::TransitionTarget::AddService(ADDED), report())
        .unwrap();
    let command = catalog::Command::Transition(prepared);
    let token = core
        .reserve(&mut disk, command, catalog::CommandDigest([19; 32]))
        .unwrap();
    core.begin(&mut disk, token, &command).unwrap();
    let action = core.action(token).unwrap().unwrap();
    let mut runtime = Controller::new(&mut state);
    let owner = runtime.recover_quiescence(&core, action, plan()).unwrap();
    let receipt = finish_stop(&mut runtime, &core, &mut driver, owner);
    core.ack_configuration_quiesced(&mut disk, receipt.ticket(), receipt.report())
        .unwrap();
    runtime.confirm_quiescence(&core, receipt).unwrap();
    let action = core.action(token).unwrap().unwrap();
    let mut initializer = Initializer::default();
    let initialized = configuration::initialize_service(&core, &mut initializer, action).unwrap();
    core.ack_service_initialized(&mut disk, initialized)
        .unwrap();
    assert_eq!(
        core.identity(id).unwrap().desired,
        catalog::DesiredState::Paused
    );
    assert_eq!(initializer.calls, 1);
    assert!(configuration::initialize_service(&core, &mut initializer, action).is_err());
    assert_eq!(initializer.calls, 1);

    let deps = catalog::DependencyReport::new(
        &[
            catalog::ServiceDependency::clear(SERVICE, 1),
            catalog::ServiceDependency::clear(ADDED, 1),
        ],
        0,
    )
    .unwrap();
    let enable = catalog::Command::Transition(
        core.prepare(id, catalog::TransitionTarget::Enable, deps)
            .unwrap(),
    );
    let enable_token = core
        .reserve(&mut disk, enable, catalog::CommandDigest([20; 32]))
        .unwrap();
    core.begin(&mut disk, enable_token, &enable).unwrap();
    let enable_action = core.action(enable_token).unwrap().unwrap();
    assert_eq!(
        runtime.begin_activation(&core, enable_action, plan()),
        Err(Fault::InvalidPlan)
    );
    let mut complete = plan();
    complete.services[1] = Some(ServicePlan {
        service: ADDED,
        resources: Resources::ZERO,
        requires_receive_state: false,
        capabilities: Capabilities::QUALIFIED,
    });
    let owner = runtime
        .begin_activation(&core, enable_action, complete)
        .unwrap();
    runtime
        .fence_previous_workers(&core, &mut driver, owner)
        .unwrap();
    runtime.preflight(&core, &mut driver, owner).unwrap();
    driver.prepared.storage_ready = false;
    assert_eq!(
        runtime.prepare_service(&core, &mut driver, owner, ADDED),
        Err(Error::Fault(Fault::InvalidEvidence))
    );
    assert_eq!(initializer.calls, 1);
    assert_eq!(core.identity(id).unwrap().services.iter().count(), 2);
}

#[test]
fn unavailable_or_stale_initializer_cannot_complete_pending_configuration() {
    let (mut disk, mut work, mut stage, _, _) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let id = create(&mut core, &mut disk, 1);
    let target = catalog::ServiceId(901);
    let prepared = core
        .prepare(id, catalog::TransitionTarget::AddService(target), report())
        .unwrap();
    let command = catalog::Command::Transition(prepared);
    let token = core
        .reserve(&mut disk, command, catalog::CommandDigest([31; 32]))
        .unwrap();
    core.begin(&mut disk, token, &command).unwrap();
    let ticket = core.action(token).unwrap().unwrap().ticket();
    core.ack_configuration_quiesced(&mut disk, ticket, report())
        .unwrap();
    let action = core.action(token).unwrap().unwrap();
    let mut initializer = Initializer {
        unavailable: true,
        ..Initializer::default()
    };
    assert_eq!(
        configuration::initialize_service(&core, &mut initializer, action),
        Err(Error::Driver(()))
    );
    initializer.unavailable = false;
    initializer.wrong_scope = true;
    assert_eq!(
        configuration::initialize_service(&core, &mut initializer, action),
        Err(Error::Fault(Fault::StaleEvidence))
    );
    assert_eq!(
        core.identity(id).unwrap().services,
        catalog::ServiceSet::new(&[SERVICE]).unwrap()
    );
    assert_eq!(
        core.identity(id).unwrap().pending.unwrap().phase,
        catalog::TransitionPhase::InitializingService
    );
    initializer.wrong_scope = false;
    let proof = configuration::initialize_service(&core, &mut initializer, action).unwrap();
    core.ack_service_initialized(&mut disk, proof).unwrap();
}

#[path = "controller/termination.rs"]
mod termination;

#[test]
fn recovery_finds_only_the_original_exact_lease_across_stop_and_termination() {
    let (mut disk, mut work, mut stage, mut state, mut driver) = fixtures();
    let mut core = Core::open(&mut disk, &mut work, &mut stage).unwrap();
    let first = create(&mut core, &mut disk, 1);
    let second = create(&mut core, &mut disk, 2);
    let mut runtime = Controller::new(&mut state);
    let owner = running(&mut runtime, &mut core, &mut disk, &mut driver, first);
    let other = running(&mut runtime, &mut core, &mut disk, &mut driver, second);
    let operation = MutationId([91; 16]);
    let lease = runtime
        .admit_mutation(
            &core,
            guard(&core, first, catalog::AdmissionClass::ServiceMutation),
            SERVICE,
            operation,
        )
        .unwrap();
    let action = transition(
        &mut core,
        &mut disk,
        first,
        catalog::TransitionTarget::Pause,
    );
    runtime.begin_quiescence(&core, action).unwrap();
    runtime.begin_termination();
    let recovery = guard(&core, first, catalog::AdmissionClass::Recovery);
    assert_ne!(recovery, lease.admission());
    assert_eq!(
        runtime
            .mutation_for_recovery(&core, recovery, owner, SERVICE, operation)
            .unwrap(),
        lease
    );
    assert_eq!(runtime.state().in_flight_mutations(first), 1);
    assert_eq!(
        runtime.mutation_for_recovery(&core, recovery, other, SERVICE, operation),
        Err(Fault::WrongIdentity)
    );
    assert_eq!(
        runtime.mutation_for_recovery(&core, recovery, owner, SERVICE, MutationId([92; 16])),
        Err(Fault::StaleLease)
    );
    assert_eq!(
        runtime.mutation_for_recovery(&core, recovery, owner, catalog::ServiceId(999), operation),
        Err(Fault::UnknownService)
    );
    assert_eq!(
        runtime.mutation_for_recovery(
            &core,
            guard(&core, first, catalog::AdmissionClass::History),
            owner,
            SERVICE,
            operation
        ),
        Err(Fault::InvalidAdmission)
    );
    runtime
        .complete_mutation(lease, Settlement::Committed)
        .unwrap();
    assert_eq!(
        runtime.mutation_for_recovery(&core, recovery, owner, SERVICE, operation),
        Err(Fault::StaleLease)
    );
    assert_eq!(runtime.state().in_flight_mutations(first), 0);
}
