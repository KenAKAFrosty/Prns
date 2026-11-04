use crate::model::*;

mod termination;
use prns_identities_core::{
    Action, AdmissionClass, AdmissionGuard, Command, Coordinator, DesiredState, EffectKind,
    EffectTicket, IdentityGuard, LocalIdentityId, OperationResult, OperationState, ServiceId,
    ServiceSet, TransitionPhase, TransitionTarget,
};

/// Checks the current durable/exclusive owner immediately before a dispatch.
/// A native adapter should additionally check its locked durable store here.
/// Implementations must not accept a stale in-memory Coordinator after another
/// owner has opened the same catalog. This trait does not acquire a worker lock;
/// hold the embedding host's exclusive dispatch gate across check and dispatch.
pub trait Authority {
    fn validate_effect(&self, ticket: EffectTicket) -> Result<(), Fault>;
    fn validate_action<const S: usize>(&self, action: &Action<S>) -> Result<(), Fault>;
    fn validate_admission(&self, guard: AdmissionGuard) -> Result<(), Fault>;
    fn validate_quiescence_commit(
        &self,
        ticket: EffectTicket,
        identity: IdentityGuard,
    ) -> Result<(), Fault>;
}

/// Sufficient only when the caller independently guarantees exclusive catalog
/// ownership. Filesystem/shared-store consumers should wrap this with a durable
/// session check rather than relying on an old Coordinator's local snapshot.
impl<const I: usize, const O: usize, const S: usize> Authority for Coordinator<'_, I, O, S> {
    fn validate_effect(&self, ticket: EffectTicket) -> Result<(), Fault> {
        Coordinator::validate_effect(self, ticket).map_err(authority_error)
    }

    fn validate_admission(&self, guard: AdmissionGuard) -> Result<(), Fault> {
        Coordinator::validate_admission(self, guard).map_err(authority_error)
    }

    fn validate_action<const N: usize>(&self, action: &Action<N>) -> Result<(), Fault> {
        Coordinator::validate_effect(self, action.ticket()).map_err(authority_error)?;
        if let Action::InitializeService {
            ticket,
            identity,
            key_reference,
            service,
        } = action
        {
            let current = self.identity(ticket.identity).map_err(authority_error)?;
            let token = ticket.operation.ok_or(Fault::StaleEffect)?;
            let operation = self.operation(token).map_err(authority_error)?;
            let Command::Transition(prepared) = operation.command else {
                return Err(Fault::StaleEffect);
            };
            if *identity != current.guard()
                || *key_reference != current.key_reference
                || prepared.target != TransitionTarget::AddService(*service)
                || ticket.kind != EffectKind::InitializeService
                || current.services.iter().any(|id| id == *service)
            {
                return Err(Fault::StaleEffect);
            }
            return Ok(());
        }
        let (ticket, guard, key, services) = match action {
            Action::Activate {
                ticket,
                identity,
                key_reference,
                services,
            } => (ticket, identity, Some(key_reference), services),
            Action::Quiesce {
                ticket,
                identity,
                services,
            } => (ticket, identity, None, services),
            Action::PublishKey { .. } | Action::InitializeService { .. } => {
                return Err(Fault::StaleEffect)
            }
        };
        let current = self.identity(ticket.identity).map_err(authority_error)?;
        if *guard != current.guard()
            || key.is_some_and(|key| *key != current.key_reference)
            || services.iter().count() != current.services.iter().count()
            || services
                .iter()
                .any(|id| current.services.iter().filter(|other| *other == id).count() != 1)
            || current
                .services
                .iter()
                .any(|id| services.iter().filter(|other| *other == id).count() != 1)
            || (key.is_some() && !matches!(ticket.kind, EffectKind::Activate | EffectKind::Restore))
            || (key.is_none()
                && !matches!(
                    ticket.kind,
                    EffectKind::Quiesce | EffectKind::RestoreCleanup
                ))
        {
            return Err(Fault::StaleEffect);
        }
        Ok(())
    }

    fn validate_quiescence_commit(
        &self,
        ticket: EffectTicket,
        identity: IdentityGuard,
    ) -> Result<(), Fault> {
        let current = self.identity(identity.id).map_err(authority_error)?;
        let initializing = current.pending.is_some_and(|pending| {
            Some(pending.operation) == ticket.operation
                && pending.phase == TransitionPhase::InitializingService
                && matches!(pending.target, TransitionTarget::AddService(_))
        });
        if self.snapshot().session < ticket.session
            || current.guard() != identity
            || ticket.identity != identity.id
            || ticket.lifecycle_revision != identity.lifecycle_revision
            || (current.pending.is_some() && !initializing)
            || current.recovery_cleanup.is_some()
        {
            return Err(Fault::StaleEffect);
        }
        match (ticket.kind, ticket.operation) {
            (EffectKind::RestoreCleanup, None) if current.desired == DesiredState::Enabled => {
                Ok(())
            }
            (EffectKind::Quiesce, Some(token)) => {
                let operation = self.operation(token).map_err(authority_error)?;
                let Command::Transition(prepared) = operation.command else {
                    return Err(Fault::StaleEffect);
                };
                if prepared.identity.id != identity.id
                    || prepared.identity.public_hash != identity.public_hash
                    || prepared.identity.lifecycle_revision.checked_add(1)
                        != Some(identity.lifecycle_revision)
                {
                    return Err(Fault::StaleEffect);
                }
                let agrees = match operation.state {
                    OperationState::Completed(OperationResult::Retired { identity: id }) => {
                        id == identity.id && current.desired == DesiredState::Retired
                    }
                    OperationState::Completed(
                        OperationResult::Paused { identity: id }
                        | OperationResult::RetirementBlocked { identity: id, .. }
                        | OperationResult::ActivationFailed { identity: id, .. }
                        | OperationResult::ServiceAdded { identity: id, .. }
                        | OperationResult::ServiceRemoved { identity: id, .. }
                        | OperationResult::ServiceRemovalBlocked { identity: id, .. },
                    ) => id == identity.id && current.desired == DesiredState::Paused,
                    OperationState::Transitioning => {
                        initializing && current.desired == DesiredState::Paused
                    }
                    _ => false,
                };
                if agrees {
                    Ok(())
                } else {
                    Err(Fault::StaleEffect)
                }
            }
            _ => Err(Fault::StaleEffect),
        }
    }
}

/// Exclusive, borrowed controller; constructing it changes no runtime state.
/// Keep the state alive until all external workers have been fenced. No method
/// stops shared interfaces or a host, deletes keys, or releases another owner.
pub struct Controller<'a, const I: usize, const S: usize, const L: usize> {
    state: &'a mut RuntimeState<I, S, L>,
}

impl<'a, const I: usize, const S: usize, const L: usize> Controller<'a, I, S, L> {
    pub fn new(state: &'a mut RuntimeState<I, S, L>) -> Self {
        Self { state }
    }

    pub fn state(&self) -> &RuntimeState<I, S, L> {
        self.state
    }

    pub fn owner(&self, identity: LocalIdentityId) -> Option<OwnerToken> {
        self.state
            .owners
            .iter()
            .flatten()
            .chain(self.state.cleanup.iter())
            .find(|owner| owner.identity.id == identity)
            .map(|owner| owner.token)
    }

    /// Declared service IDs of the retained owner, including during termination
    /// when the durable catalog may be unavailable or already changed.
    pub fn service_ids(
        &self,
        token: OwnerToken,
    ) -> Result<impl Iterator<Item = ServiceId> + '_, Fault> {
        Ok(self
            .get(token)?
            .services
            .iter()
            .flatten()
            .map(|service| service.plan.service))
    }

    pub fn phase(&self, token: OwnerToken) -> Result<Phase, Fault> {
        Ok(self.get(token)?.phase)
    }

    /// Atomically reserve this complete service plan before any registration.
    /// Even a new activation first fences/reconciles previous tagged workers;
    /// a fresh in-memory state is never evidence that old resources vanished.
    pub fn begin_activation<A: Authority>(
        &mut self,
        authority: &A,
        action: Action<S>,
        plan: ActivationPlan<S>,
    ) -> Result<OwnerToken, Fault> {
        self.ensure_open()?;
        authority.validate_action(&action)?;
        if self.state.cleanup.is_some() {
            return Err(Fault::Busy);
        }
        let Action::Activate {
            ticket,
            identity,
            key_reference,
            services,
        } = action
        else {
            return Err(Fault::StaleEffect);
        };
        authority.validate_effect(ticket)?;
        validate_action(ticket, identity)?;
        if !matches!(ticket.kind, EffectKind::Activate | EffectKind::Restore) {
            return Err(Fault::StaleEffect);
        }
        validate_plan(&plan, &services)?;
        if let Some(token) = self.owner(identity.id) {
            let prior = self.get(token)?;
            if prior.ticket == ticket
                && prior.identity == identity
                && prior.plan == plan
                && prior.key == Some(key_reference)
                && !matches!(
                    prior.phase,
                    Phase::FencingPreviousWorkers | Phase::Quiescing | Phase::Quiesced
                )
            {
                return Ok(token);
            }
            return Err(Fault::OwnerStillActive);
        }
        self.insert(
            identity,
            ticket,
            Some(key_reference),
            plan,
            Phase::FencingActivation,
        )
    }

    /// Reconcile a retained owner after a newer catalog session or lifecycle,
    /// including an ambiguous activation acknowledgement or repeated Enable.
    /// This never forgets
    /// reservations or accepted mutations. Old workers must be fenced and all
    /// existing leases settled before the driver can restore/register services.
    pub fn reconcile_activation<A: Authority>(
        &mut self,
        authority: &A,
        action: Action<S>,
        plan: ActivationPlan<S>,
    ) -> Result<OwnerToken, Fault> {
        self.ensure_open()?;
        authority.validate_action(&action)?;
        if self.state.cleanup.is_some() {
            return Err(Fault::Busy);
        }
        let Action::Activate {
            ticket,
            identity,
            key_reference,
            services,
        } = action
        else {
            return Err(Fault::StaleEffect);
        };
        authority.validate_effect(ticket)?;
        validate_action(ticket, identity)?;
        validate_plan(&plan, &services)?;
        if !matches!(ticket.kind, EffectKind::Activate | EffectKind::Restore) {
            return Err(Fault::StaleEffect);
        }
        let token = self.owner(identity.id).ok_or(Fault::UnknownIdentity)?;
        let owner = *self.get(token)?;
        if ticket == owner.ticket {
            return self.begin_activation(authority, action, plan);
        }
        if ticket.session < owner.ticket.session
            || (ticket.session == owner.ticket.session
                && identity.lifecycle_revision <= owner.identity.lifecycle_revision)
            || identity.public_hash != owner.identity.public_hash
            || identity.lifecycle_revision < owner.identity.lifecycle_revision
            || plan != owner.plan
            || owner.key.is_some_and(|key| key != key_reference)
        {
            return Err(Fault::StaleEffect);
        }
        let reservation = plan.total()?;
        let used = self
            .state
            .used
            .checked_sub(owner.reservation)
            .and_then(|used| used.checked_add(reservation))
            .ok_or(Fault::Exhausted)?;
        if !used.fits(self.state.budget.application_capacity()?) {
            return Err(Fault::CapacityExceeded);
        }
        let generation = owner.generation.checked_add(1).ok_or(Fault::Exhausted)?;
        let current = self.get_mut(token)?;
        current.identity = identity;
        current.ticket = ticket;
        current.key = Some(key_reference);
        current.generation = generation;
        current.reservation = reservation;
        current.phase = Phase::FencingActivation;
        current.report = None;
        for service in current.services.iter_mut().flatten() {
            service.phase = ServicePhase::Declared;
            service.dependencies = None;
        }
        self.state.used = used;
        Ok(token)
    }

    /// A process restart with no retained runtime table must explicitly recover
    /// a pending stop. Driver reconciliation fences old tagged workers first.
    /// Never use this to replace an owner/lease table that is still alive.
    pub fn recover_quiescence<A: Authority>(
        &mut self,
        authority: &A,
        action: Action<S>,
        plan: ActivationPlan<S>,
    ) -> Result<OwnerToken, Fault> {
        self.ensure_open()?;
        authority.validate_action(&action)?;
        let Action::Quiesce {
            ticket,
            identity,
            services,
        } = action
        else {
            return Err(Fault::StaleEffect);
        };
        authority.validate_effect(ticket)?;
        validate_action(ticket, identity)?;
        if !matches!(
            ticket.kind,
            EffectKind::Quiesce | EffectKind::RestoreCleanup
        ) {
            return Err(Fault::StaleEffect);
        }
        validate_plan(&plan, &services)?;
        if let Some(token) = self.owner(identity.id) {
            let prior = self.get(token)?;
            if prior.recovering
                && prior.ticket == ticket
                && prior.plan == plan
                && matches!(
                    prior.phase,
                    Phase::FencingPreviousWorkers | Phase::Quiescing | Phase::Quiesced
                )
            {
                return Ok(token);
            }
            return Err(Fault::OwnerStillActive);
        }
        if self.state.cleanup.is_some() {
            return Err(Fault::Busy);
        }
        let sequence = self
            .state
            .next_owner
            .checked_add(1)
            .ok_or(Fault::Exhausted)?;
        let token = OwnerToken {
            incarnation: self.state.incarnation,
            sequence,
        };
        self.state.cleanup = Some(Owner {
            token,
            identity,
            ticket,
            generation: 1,
            key: None,
            plan,
            services: service_states(&plan),
            phase: Phase::FencingPreviousWorkers,
            reservation: Resources::ZERO,
            recovering: true,
            report: None,
        });
        self.state.next_owner = sequence;
        Ok(token)
    }

    pub fn fence_previous_workers<A: Authority, D: ScopedDriver<S>>(
        &mut self,
        authority: &A,
        driver: &mut D,
        token: OwnerToken,
    ) -> Result<Progress<()>, Error<D::Error>> {
        let owner = *self.get(token)?;
        if !matches!(
            owner.phase,
            Phase::FencingActivation | Phase::FencingPreviousWorkers
        ) {
            return Err(Fault::WrongPhase.into());
        }
        let scope = self.scope(authority, token, None, Step::FencePreviousWorkers)?;
        match driver
            .fence_previous_workers(scope)
            .map_err(Error::Driver)?
        {
            Progress::Pending => Ok(Progress::Pending),
            Progress::Complete(evidence) => {
                check_evidence(scope, &evidence)?;
                if !evidence.value {
                    return Err(Fault::InvalidEvidence.into());
                }
                self.get_mut(token)?.phase = if owner.phase == Phase::FencingActivation {
                    Phase::Preflighting
                } else {
                    Phase::Quiescing
                };
                Ok(Progress::Complete(()))
            }
        }
    }

    pub fn preflight<A: Authority, D: ScopedDriver<S>>(
        &mut self,
        authority: &A,
        driver: &mut D,
        token: OwnerToken,
    ) -> Result<Progress<()>, Error<D::Error>> {
        self.ensure_open()?;
        let owner = *self.get(token)?;
        if owner.phase != Phase::Preflighting {
            return Err(Fault::WrongPhase.into());
        }
        if self.state.cleanup.is_some() {
            return Err(Fault::Busy.into());
        }
        self.drained(token)?;
        let scope = self.scope(authority, token, None, Step::Preflight)?;
        match driver
            .preflight(scope, &owner.plan)
            .map_err(Error::Driver)?
        {
            Progress::Pending => Ok(Progress::Pending),
            Progress::Complete(evidence) => {
                check_evidence(scope, &evidence)?;
                if evidence.value != owner.reservation {
                    return Err(Fault::InvalidEvidence.into());
                }
                self.get_mut(token)?.phase = Phase::Activating;
                Ok(Progress::Complete(()))
            }
        }
    }

    /// Must restore the durable service store and receive keys before ingress.
    pub fn prepare_service<A: Authority, D: ScopedDriver<S>>(
        &mut self,
        authority: &A,
        driver: &mut D,
        token: OwnerToken,
        service: ServiceId,
    ) -> Result<Progress<()>, Error<D::Error>> {
        self.ensure_open()?;
        if self.state.cleanup.is_some() {
            return Err(Fault::Busy.into());
        }
        let owner = self.get(token)?;
        if owner.phase != Phase::Activating {
            return Err(Fault::WrongPhase.into());
        }
        let index = service_index(owner, service)?;
        let state = owner.services[index].ok_or(Fault::UnknownService)?;
        let key = owner.key.ok_or(Fault::WrongPhase)?;
        let scope = self.scope(authority, token, Some(service), Step::PrepareService)?;
        if matches!(state.phase, ServicePhase::Prepared | ServicePhase::Active) {
            return Ok(Progress::Complete(()));
        }
        match driver
            .prepare_service(scope, key, state.plan)
            .map_err(Error::Driver)?
        {
            Progress::Pending => Ok(Progress::Pending),
            Progress::Complete(evidence) => {
                check_evidence(scope, &evidence)?;
                if !evidence.value.storage_ready
                    || (state.plan.requires_receive_state && !evidence.value.receive_state_restored)
                {
                    return Err(Fault::InvalidEvidence.into());
                }
                self.get_mut(token)?.services[index]
                    .as_mut()
                    .ok_or(Fault::UnknownService)?
                    .phase = ServicePhase::Prepared;
                Ok(Progress::Complete(()))
            }
        }
    }

    pub fn activate_service<A: Authority, D: ScopedDriver<S>>(
        &mut self,
        authority: &A,
        driver: &mut D,
        token: OwnerToken,
        service: ServiceId,
    ) -> Result<Progress<()>, Error<D::Error>> {
        self.ensure_open()?;
        if self.state.cleanup.is_some() {
            return Err(Fault::Busy.into());
        }
        let owner = self.get(token)?;
        if owner.phase != Phase::Activating {
            return Err(Fault::WrongPhase.into());
        }
        let index = service_index(owner, service)?;
        let state = owner.services[index].ok_or(Fault::UnknownService)?;
        let scope = self.scope(authority, token, Some(service), Step::ActivateService)?;
        if state.phase == ServicePhase::Active {
            return Ok(Progress::Complete(()));
        }
        if state.phase != ServicePhase::Prepared {
            return Err(Fault::StorageNotReady.into());
        }
        match driver
            .activate_service(scope, state.plan)
            .map_err(Error::Driver)?
        {
            Progress::Pending => Ok(Progress::Pending),
            Progress::Complete(evidence) => {
                check_evidence(scope, &evidence)?;
                self.get_mut(token)?.services[index]
                    .as_mut()
                    .ok_or(Fault::UnknownService)?
                    .phase = ServicePhase::Active;
                Ok(Progress::Complete(()))
            }
        }
    }

    pub fn finish_activation<A: Authority>(
        &mut self,
        authority: &A,
        token: OwnerToken,
    ) -> Result<ActivationReceipt, Fault> {
        self.ensure_open()?;
        if self.state.cleanup.is_some() {
            return Err(Fault::Busy);
        }
        let owner = self.get(token)?;
        authority.validate_effect(owner.ticket)?;
        if !matches!(
            owner.phase,
            Phase::Activating | Phase::AwaitingCatalogCommit
        ) || owner
            .services
            .iter()
            .flatten()
            .any(|service| service.phase != ServicePhase::Active)
        {
            return Err(Fault::WrongPhase);
        }
        self.get_mut(token)?.phase = Phase::AwaitingCatalogCommit;
        self.activation_receipt(token)
    }

    /// Diagnostic/idempotent receipt lookup after a catalog acknowledgement.
    pub fn activation_receipt(&self, token: OwnerToken) -> Result<ActivationReceipt, Fault> {
        let owner = self.get(token)?;
        if !matches!(owner.phase, Phase::AwaitingCatalogCommit | Phase::Running) {
            return Err(Fault::WrongPhase);
        }
        Ok(ActivationReceipt {
            owner: token,
            ticket: owner.ticket,
            generation: owner.generation,
        })
    }

    /// Open dispatch only after the lifecycle catalog acknowledges activation.
    pub fn confirm_activation<A: Authority>(
        &mut self,
        authority: &A,
        receipt: ActivationReceipt,
        admission: AdmissionGuard,
    ) -> Result<(), Fault> {
        self.ensure_open()?;
        if self.state.cleanup.is_some() {
            return Err(Fault::Busy);
        }
        let owner = self.get(receipt.owner)?;
        if self.activation_receipt(receipt.owner)? != receipt {
            return Err(Fault::StaleEvidence);
        }
        check_guard(owner, admission)?;
        if admission.class != AdmissionClass::Network {
            return Err(Fault::InvalidAdmission);
        }
        authority.validate_admission(admission)?;
        self.get_mut(receipt.owner)?.phase = Phase::Running;
        Ok(())
    }

    /// A lease admits one bounded service mutation, including durable offline
    /// publication. It does not authorize network dispatch or override the
    /// service's durable request-ID deduplication.
    pub fn admit_mutation<A: Authority>(
        &mut self,
        authority: &A,
        guard: AdmissionGuard,
        service: ServiceId,
        operation: MutationId,
    ) -> Result<MutationLease, Fault> {
        self.ensure_open()?;
        authority.validate_admission(guard)?;
        if !matches!(
            guard.class,
            AdmissionClass::ServiceMutation | AdmissionClass::Network
        ) {
            return Err(Fault::InvalidAdmission);
        }
        let token = self
            .owner(guard.identity.id)
            .ok_or(Fault::UnknownIdentity)?;
        let owner = self.get(token)?;
        check_guard(owner, guard)?;
        if !matches!(
            owner.phase,
            Phase::Activating | Phase::AwaitingCatalogCommit | Phase::Running
        ) || (guard.class == AdmissionClass::Network && owner.phase != Phase::Running)
        {
            return Err(Fault::WrongPhase);
        }
        let service_state =
            owner.services[service_index(owner, service)?].ok_or(Fault::UnknownService)?;
        if !matches!(
            service_state.phase,
            ServicePhase::Prepared | ServicePhase::Active
        ) {
            return Err(Fault::StorageNotReady);
        }
        if self.state.leases.iter().flatten().any(|lease| {
            lease.owner == token && lease.service == service && lease.operation == operation
        }) {
            return Err(Fault::DuplicateMutation);
        }
        let index = self
            .state
            .leases
            .iter()
            .position(Option::is_none)
            .ok_or(Fault::CapacityExceeded)?;
        let sequence = self
            .state
            .next_lease
            .checked_add(1)
            .ok_or(Fault::Exhausted)?;
        let lease = MutationLease {
            sequence,
            owner: token,
            service,
            operation,
            admission: guard,
        };
        self.state.leases[index] = Some(lease);
        self.state.next_lease = sequence;
        Ok(lease)
    }

    /// Inspect an existing exact lease under current recovery authority. This
    /// allocates nothing and never refreshes its original owner, guard or ID.
    /// Recovery may cross a lifecycle/session fence, including termination;
    /// only the immutable identity must still match the original owner.
    pub fn mutation_for_recovery<A: Authority>(
        &self,
        authority: &A,
        recovery: AdmissionGuard,
        owner: OwnerToken,
        service: ServiceId,
        operation: MutationId,
    ) -> Result<MutationLease, Fault> {
        authority.validate_admission(recovery)?;
        if recovery.class != AdmissionClass::Recovery {
            return Err(Fault::InvalidAdmission);
        }
        let current = self.get(owner)?;
        if current.identity.id != recovery.identity.id
            || current.identity.public_hash != recovery.identity.public_hash
        {
            return Err(Fault::WrongIdentity);
        }
        service_index(current, service)?;
        self.state
            .leases
            .iter()
            .flatten()
            .find(|lease| {
                lease.owner == owner && lease.service == service && lease.operation == operation
            })
            .copied()
            .ok_or(Fault::StaleLease)
    }

    /// Old admitted work may settle through pause/session changes; this exact
    /// tracked lease cannot be converted into fresh work or replayed after drain.
    pub fn validate_settlement(&self, lease: MutationLease) -> Result<(), Fault> {
        if self.state.leases.contains(&Some(lease)) {
            Ok(())
        } else {
            Err(Fault::StaleLease)
        }
    }

    pub fn complete_mutation(
        &mut self,
        lease: MutationLease,
        _settlement: Settlement,
    ) -> Result<(), Fault> {
        let index = self
            .state
            .leases
            .iter()
            .position(|current| *current == Some(lease))
            .ok_or(Fault::StaleLease)?;
        self.state.leases[index] = None;
        Ok(())
    }

    pub fn network_permit<A: Authority>(
        &self,
        authority: &A,
        guard: AdmissionGuard,
        service: ServiceId,
    ) -> Result<NetworkPermit, Fault> {
        self.ensure_open()?;
        authority.validate_admission(guard)?;
        if guard.class != AdmissionClass::Network {
            return Err(Fault::InvalidAdmission);
        }
        let token = self
            .owner(guard.identity.id)
            .ok_or(Fault::UnknownIdentity)?;
        let owner = self.get(token)?;
        check_guard(owner, guard)?;
        let state = owner.services[service_index(owner, service)?].ok_or(Fault::UnknownService)?;
        if owner.phase != Phase::Running || state.phase != ServicePhase::Active {
            return Err(Fault::WrongPhase);
        }
        Ok(NetworkPermit {
            owner: token,
            service,
            admission: guard,
            generation: owner.generation,
        })
    }

    /// Call under the same worker/dispatch lock immediately before the actual
    /// external effect, including after dequeue. Queue submission is insufficient.
    pub fn validate_network<A: Authority>(
        &self,
        authority: &A,
        permit: NetworkPermit,
    ) -> Result<(), Fault> {
        let current = self.network_permit(authority, permit.admission, permit.service)?;
        if current == permit {
            Ok(())
        } else {
            Err(Fault::StaleEvidence)
        }
    }

    /// Fence fresh work locally, preserving admitted mutations and reservations.
    /// Driver cleanup is impossible until every tracked mutation has settled.
    pub fn begin_quiescence<A: Authority>(
        &mut self,
        authority: &A,
        action: Action<S>,
    ) -> Result<OwnerToken, Fault> {
        self.ensure_open()?;
        authority.validate_action(&action)?;
        let Action::Quiesce {
            ticket,
            identity,
            services,
        } = action
        else {
            return Err(Fault::StaleEffect);
        };
        authority.validate_effect(ticket)?;
        validate_action(ticket, identity)?;
        if !matches!(
            ticket.kind,
            EffectKind::Quiesce | EffectKind::RestoreCleanup
        ) {
            return Err(Fault::StaleEffect);
        }
        let token = self.owner(identity.id).ok_or(Fault::UnknownIdentity)?;
        let owner = self.get(token)?;
        if owner.identity.public_hash != identity.public_hash {
            return Err(Fault::WrongIdentity);
        }
        validate_plan(&owner.plan, &services)?;
        if owner.ticket == ticket
            && matches!(
                owner.phase,
                Phase::FencingPreviousWorkers | Phase::Quiescing | Phase::Quiesced
            )
        {
            return Ok(token);
        }
        if owner.phase == Phase::Quiesced
            && (ticket.operation != owner.ticket.operation
                || ticket.kind != owner.ticket.kind
                || identity != owner.identity
                || ticket.session <= owner.ticket.session)
        {
            return Err(Fault::StaleEffect);
        }
        let needs_fence = ticket.session != owner.ticket.session
            || matches!(
                owner.phase,
                Phase::FencingActivation | Phase::FencingPreviousWorkers
            );
        let generation = owner.generation.checked_add(1).ok_or(Fault::Exhausted)?;
        let owner = self.get_mut(token)?;
        owner.identity = identity;
        owner.ticket = ticket;
        owner.generation = generation;
        owner.phase = if needs_fence {
            Phase::FencingPreviousWorkers
        } else {
            Phase::Quiescing
        };
        owner.report = None;
        if needs_fence {
            for service in owner.services.iter_mut().flatten() {
                service.phase = ServicePhase::Declared;
                service.dependencies = None;
            }
        }
        Ok(token)
    }

    pub fn quiesce_service<A: Authority, D: ScopedDriver<S>>(
        &mut self,
        authority: &A,
        driver: &mut D,
        token: OwnerToken,
        service: ServiceId,
    ) -> Result<Progress<()>, Error<D::Error>> {
        let owner = self.get(token)?;
        if owner.phase != Phase::Quiescing {
            return Err(Fault::WrongPhase.into());
        }
        self.drained(token)?;
        let index = service_index(owner, service)?;
        let state = owner.services[index].ok_or(Fault::UnknownService)?;
        let scope = self.scope(authority, token, Some(service), Step::QuiesceService)?;
        if state.phase == ServicePhase::Quiesced {
            return Ok(Progress::Complete(()));
        }
        match driver
            .quiesce_service(scope, state.plan)
            .map_err(Error::Driver)?
        {
            Progress::Pending => Ok(Progress::Pending),
            Progress::Complete(evidence) => {
                check_evidence(scope, &evidence)?;
                if !evidence
                    .value
                    .evidence
                    .complete(state.plan.requires_receive_state)
                    || evidence.value.dependencies.service != service
                    || !evidence.value.dependencies.quiescence_supported
                {
                    return Err(Fault::InvalidEvidence.into());
                }
                let service = self.get_mut(token)?.services[index]
                    .as_mut()
                    .ok_or(Fault::UnknownService)?;
                service.phase = ServicePhase::Quiesced;
                service.dependencies = Some(evidence.value.dependencies);
                Ok(Progress::Complete(()))
            }
        }
    }

    /// Release only scoped reservations after complete driver observations and
    /// re-read authoritative dependency facts. Driver barriers remain held until
    /// catalog completion; late recovery updates must be serialized with this.
    pub fn finish_quiescence<A: Authority, D: ScopedDriver<S>>(
        &mut self,
        authority: &A,
        driver: &mut D,
        token: OwnerToken,
    ) -> Result<Progress<QuiescenceReceipt<S>>, Error<D::Error>> {
        let owner = *self.get(token)?;
        if owner.phase == Phase::Quiesced {
            self.scope(authority, token, None, Step::ReleaseIdentity)?;
            return Ok(Progress::Complete(self.quiescence_receipt(token)?));
        }
        if owner.phase != Phase::Quiescing
            || owner
                .services
                .iter()
                .flatten()
                .any(|service| service.phase != ServicePhase::Quiesced)
        {
            return Err(Fault::WrongPhase.into());
        }
        self.drained(token)?;
        let scope = self.scope(authority, token, None, Step::ReleaseIdentity)?;
        match driver.release_identity(scope).map_err(Error::Driver)? {
            Progress::Pending => Ok(Progress::Pending),
            Progress::Complete(evidence) => {
                check_evidence(scope, &evidence)?;
                if !evidence.value.owned_reservations_released
                    || !evidence.value.shared_owners_preserved
                {
                    return Err(Fault::InvalidEvidence.into());
                }
                validate_report(&owner.plan, &evidence.value.report)?;
                for previous in owner
                    .services
                    .iter()
                    .flatten()
                    .filter_map(|service| service.dependencies)
                {
                    if evidence.value.report.entries.iter().flatten().any(|entry| {
                        entry.service == previous.service && entry.revision < previous.revision
                    }) {
                        return Err(Fault::InvalidEvidence.into());
                    }
                }
                let used = self
                    .state
                    .used
                    .checked_sub(owner.reservation)
                    .ok_or(Fault::InvalidEvidence)?;
                let owner = self.get_mut(token)?;
                owner.reservation = Resources::ZERO;
                owner.report = Some(evidence.value.report);
                owner.phase = Phase::Quiesced;
                self.state.used = used;
                Ok(Progress::Complete(self.quiescence_receipt(token)?))
            }
        }
    }

    pub fn quiescence_receipt(&self, token: OwnerToken) -> Result<QuiescenceReceipt<S>, Fault> {
        let owner = self.get(token)?;
        if owner.phase != Phase::Quiesced {
            return Err(Fault::WrongPhase);
        }
        Ok(QuiescenceReceipt {
            termination: self.state.termination,
            owner: token,
            ticket: owner.ticket,
            generation: owner.generation,
            report: owner.report.ok_or(Fault::InvalidEvidence)?,
        })
    }

    /// Remove bookkeeping only after the durable lifecycle acknowledgement. A
    /// failed/ambiguous catalog write retains the receipt and owner for recovery.
    pub fn confirm_quiescence<A: Authority>(
        &mut self,
        authority: &A,
        receipt: QuiescenceReceipt<S>,
    ) -> Result<(), Fault> {
        self.ensure_open()?;
        if self.quiescence_receipt(receipt.owner)? != receipt {
            return Err(Fault::StaleEvidence);
        }
        let owner = self.get(receipt.owner)?;
        authority.validate_quiescence_commit(receipt.ticket, owner.identity)?;
        self.drained(receipt.owner)?;
        let index = self.index(receipt.owner)?;
        if index == I {
            self.state.cleanup = None;
        } else {
            self.state.owners[index] = None;
        }
        Ok(())
    }

    fn insert(
        &mut self,
        identity: IdentityGuard,
        ticket: EffectTicket,
        key: Option<prns_identities_core::KeyReference>,
        plan: ActivationPlan<S>,
        phase: Phase,
    ) -> Result<OwnerToken, Fault> {
        let index = self
            .state
            .owners
            .iter()
            .position(Option::is_none)
            .ok_or(Fault::CapacityExceeded)?;
        let reservation = plan.total()?;
        let used = self
            .state
            .used
            .checked_add(reservation)
            .ok_or(Fault::Exhausted)?;
        if !used.fits(self.state.budget.application_capacity()?) {
            return Err(Fault::CapacityExceeded);
        }
        let sequence = self
            .state
            .next_owner
            .checked_add(1)
            .ok_or(Fault::Exhausted)?;
        let token = OwnerToken {
            incarnation: self.state.incarnation,
            sequence,
        };
        let services = service_states(&plan);
        self.state.owners[index] = Some(Owner {
            token,
            identity,
            ticket,
            generation: 1,
            key,
            plan,
            services,
            phase,
            reservation,
            recovering: true,
            report: None,
        });
        self.state.used = used;
        self.state.next_owner = sequence;
        Ok(token)
    }

    fn scope<A: Authority>(
        &self,
        authority: &A,
        token: OwnerToken,
        service: Option<ServiceId>,
        step: Step,
    ) -> Result<Scope, Fault> {
        let owner = self.get(token)?;
        if self.state.termination.is_some() {
            if !matches!(
                step,
                Step::FencePreviousWorkers | Step::QuiesceService | Step::ReleaseIdentity
            ) {
                return Err(Fault::Terminating);
            }
        } else {
            authority.validate_effect(owner.ticket)?;
        }
        Ok(Scope {
            owner: token,
            identity: owner.identity,
            lifecycle: owner.ticket,
            termination: self.state.termination,
            generation: owner.generation,
            service,
            step,
            recovering: owner.recovering,
        })
    }

    fn drained(&self, token: OwnerToken) -> Result<(), Fault> {
        if self
            .state
            .leases
            .iter()
            .flatten()
            .any(|lease| lease.owner == token)
        {
            Err(Fault::MutationsPending)
        } else {
            Ok(())
        }
    }

    fn index(&self, token: OwnerToken) -> Result<usize, Fault> {
        self.state
            .owners
            .iter()
            .position(|owner| owner.as_ref().is_some_and(|owner| owner.token == token))
            .or_else(|| {
                self.state
                    .cleanup
                    .as_ref()
                    .filter(|owner| owner.token == token)
                    .map(|_| I)
            })
            .ok_or(Fault::StaleOwner)
    }
    fn get(&self, token: OwnerToken) -> Result<&Owner<S>, Fault> {
        let index = self.index(token)?;
        if index == I {
            self.state.cleanup.as_ref()
        } else {
            self.state.owners[index].as_ref()
        }
        .ok_or(Fault::StaleOwner)
    }
    fn get_mut(&mut self, token: OwnerToken) -> Result<&mut Owner<S>, Fault> {
        let index = self.index(token)?;
        if index == I {
            self.state.cleanup.as_mut()
        } else {
            self.state.owners[index].as_mut()
        }
        .ok_or(Fault::StaleOwner)
    }
}

fn service_states<const S: usize>(plan: &ActivationPlan<S>) -> [Option<ServiceState>; S] {
    let mut services = [None; S];
    for (target, source) in services.iter_mut().zip(plan.services) {
        *target = source.map(|plan| ServiceState {
            plan,
            phase: ServicePhase::Declared,
            dependencies: None,
        });
    }
    services
}

fn check_evidence<T>(scope: Scope, evidence: &Evidence<T>) -> Result<(), Fault> {
    if evidence.scope == scope {
        Ok(())
    } else {
        Err(Fault::StaleEvidence)
    }
}

fn validate_action(ticket: EffectTicket, identity: IdentityGuard) -> Result<(), Fault> {
    if ticket.identity != identity.id || ticket.lifecycle_revision != identity.lifecycle_revision {
        Err(Fault::WrongIdentity)
    } else {
        Ok(())
    }
}

fn check_guard<const S: usize>(owner: &Owner<S>, guard: AdmissionGuard) -> Result<(), Fault> {
    if guard.identity != owner.identity || guard.session != owner.ticket.session {
        Err(Fault::InvalidAdmission)
    } else {
        Ok(())
    }
}

fn service_index<const S: usize>(owner: &Owner<S>, service: ServiceId) -> Result<usize, Fault> {
    owner
        .services
        .iter()
        .position(|current| {
            current
                .as_ref()
                .is_some_and(|current| current.plan.service == service)
        })
        .ok_or(Fault::UnknownService)
}

fn validate_plan<const S: usize>(
    plan: &ActivationPlan<S>,
    services: &ServiceSet<S>,
) -> Result<(), Fault> {
    if plan.services.iter().flatten().count() != services.iter().count() {
        return Err(Fault::InvalidPlan);
    }
    for plan in &plan.services {
        let Some(plan) = plan else {
            continue;
        };
        if !services.iter().any(|id| id == plan.service)
            || services.iter().filter(|id| *id == plan.service).count() != 1
        {
            return Err(Fault::InvalidPlan);
        }
        let caps = plan.capabilities;
        if !caps.scoped_teardown
            || !caps.admission_gate
            || !caps.durable_drain
            || (plan.requires_receive_state && !caps.durable_receive_state)
        {
            return Err(Fault::UnsupportedService);
        }
    }
    // Public value construction can bypass ActivationPlan::new.
    for (index, service) in plan.services.iter().enumerate() {
        if let Some(service) = service {
            if plan.services[..index]
                .iter()
                .flatten()
                .any(|prior| prior.service == service.service)
            {
                return Err(Fault::InvalidPlan);
            }
        }
    }
    plan.total()?;
    Ok(())
}

fn validate_report<const S: usize>(
    plan: &ActivationPlan<S>,
    report: &prns_identities_core::DependencyReport<S>,
) -> Result<(), Fault> {
    if !report.complete
        || report.entries.iter().flatten().count() != plan.services.iter().flatten().count()
    {
        return Err(Fault::InvalidEvidence);
    }
    for service in plan.services.iter().flatten() {
        let mut matching = report
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.service == service.service);
        let entry = matching.next().ok_or(Fault::InvalidEvidence)?;
        if matching.next().is_some() || !entry.quiescence_supported {
            return Err(Fault::InvalidEvidence);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use prns_identities_core::{KeyReference, PublicIdentityHash};

    struct Allow;
    impl Authority for Allow {
        fn validate_effect(&self, _: EffectTicket) -> Result<(), Fault> {
            Ok(())
        }
        fn validate_action<const N: usize>(&self, _: &Action<N>) -> Result<(), Fault> {
            Ok(())
        }
        fn validate_admission(&self, _: AdmissionGuard) -> Result<(), Fault> {
            Ok(())
        }
        fn validate_quiescence_commit(
            &self,
            _: EffectTicket,
            _: IdentityGuard,
        ) -> Result<(), Fault> {
            Ok(())
        }
    }

    #[test]
    fn owner_lease_and_generation_counters_never_wrap_or_evict() {
        let service = ServiceId(1);
        let identity = IdentityGuard {
            id: LocalIdentityId {
                incarnation: [1; 16],
                sequence: 1,
            },
            public_hash: PublicIdentityHash([2; 16]),
            lifecycle_revision: 1,
        };
        let ticket = EffectTicket {
            operation: None,
            identity: identity.id,
            session: 1,
            lifecycle_revision: 1,
            kind: EffectKind::Restore,
        };
        let services = ServiceSet::new(&[service]).unwrap();
        let action = Action::Activate {
            ticket,
            identity,
            key_reference: KeyReference([3; 32]),
            services,
        };
        let plan = ActivationPlan::new(
            Resources::ZERO,
            &[ServicePlan {
                service,
                resources: Resources::ZERO,
                requires_receive_state: false,
                capabilities: Capabilities::QUALIFIED,
            }],
        )
        .unwrap();
        let mut state = RuntimeState::<1, 1, 1>::new(
            [4; 16],
            Budget {
                capacity: Resources::ZERO,
                infrastructure_reserve: Resources::ZERO,
            },
        )
        .unwrap();
        state.next_owner = u64::MAX;
        let mut runtime = Controller::new(&mut state);
        assert_eq!(
            runtime.begin_activation(&Allow, action, plan),
            Err(Fault::Exhausted)
        );
        assert_eq!(runtime.state().owned_identities(), 0);
        // Test-only setup places counters at their final usable value.
        runtime.state.next_owner = u64::MAX - 1;
        let token = runtime.begin_activation(&Allow, action, plan).unwrap();
        runtime.state.next_lease = u64::MAX;
        let owner = runtime.get_mut(token).unwrap();
        owner.phase = Phase::Activating;
        owner.services[0].as_mut().unwrap().phase = ServicePhase::Prepared;
        let admission = AdmissionGuard {
            identity,
            session: 1,
            class: AdmissionClass::ServiceMutation,
        };
        assert_eq!(
            runtime.admit_mutation(&Allow, admission, service, MutationId([5; 16])),
            Err(Fault::Exhausted)
        );
        assert_eq!(runtime.state().in_flight_mutations(identity.id), 0);
        runtime.get_mut(token).unwrap().generation = u64::MAX;
        let stop = Action::Quiesce {
            ticket: EffectTicket {
                kind: EffectKind::RestoreCleanup,
                ..ticket
            },
            identity,
            services,
        };
        assert_eq!(
            runtime.begin_quiescence(&Allow, stop),
            Err(Fault::Exhausted)
        );
        assert_eq!(runtime.get(token).unwrap().generation, u64::MAX);
        assert_eq!(runtime.state().owned_identities(), 1);
        let termination = runtime.begin_termination();
        assert_eq!(runtime.get(token).unwrap().generation, u64::MAX);
        assert_eq!(runtime.phase(token).unwrap(), Phase::FencingPreviousWorkers);
        let scope = runtime
            .scope(&Allow, token, None, Step::FencePreviousWorkers)
            .unwrap();
        assert_eq!(scope.termination, Some(termination));
        runtime.validate_scope(&Allow, scope).unwrap();
    }
}
