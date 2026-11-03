use prns_identities_core::{
    AdmissionGuard, DependencyReport, EffectTicket, IdentityGuard, KeyReference, LocalIdentityId,
    ServiceDependency, ServiceId,
};

/// Counts promised to this owner, including peak simultaneous use. They must
/// include queued commands/links as appropriate to the embedding host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityResources"))]
pub struct Resources {
    pub held_keys: u32,
    pub destinations: u32,
    pub request_handlers: u32,
    pub links: u32,
    pub queue_slots: u32,
}

impl Resources {
    pub const ZERO: Self = Self {
        held_keys: 0,
        destinations: 0,
        request_handlers: 0,
        links: 0,
        queue_slots: 0,
    };

    pub fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            held_keys: self.held_keys.checked_add(other.held_keys)?,
            destinations: self.destinations.checked_add(other.destinations)?,
            request_handlers: self.request_handlers.checked_add(other.request_handlers)?,
            links: self.links.checked_add(other.links)?,
            queue_slots: self.queue_slots.checked_add(other.queue_slots)?,
        })
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            held_keys: self.held_keys.checked_sub(other.held_keys)?,
            destinations: self.destinations.checked_sub(other.destinations)?,
            request_handlers: self.request_handlers.checked_sub(other.request_handlers)?,
            links: self.links.checked_sub(other.links)?,
            queue_slots: self.queue_slots.checked_sub(other.queue_slots)?,
        })
    }

    pub fn fits(self, available: Self) -> bool {
        available.checked_sub(self).is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityBudget"))]
pub struct Budget {
    pub capacity: Resources,
    /// Capacity withheld from ALL persona reservations, covering infrastructure
    /// peak use (for example management pairing), not just its current use.
    pub infrastructure_reserve: Resources,
}

impl Budget {
    pub fn application_capacity(self) -> Result<Resources, Fault> {
        self.capacity
            .checked_sub(self.infrastructure_reserve)
            .ok_or(Fault::InvalidBudget)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub scoped_teardown: bool,
    pub admission_gate: bool,
    pub durable_drain: bool,
    pub durable_receive_state: bool,
}

impl Capabilities {
    /// This is an adapter qualification claim, not a discovery/probing result.
    pub const QUALIFIED: Self = Self {
        scoped_teardown: true,
        admission_gate: true,
        durable_drain: true,
        durable_receive_state: true,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServicePlan {
    pub service: ServiceId,
    pub resources: Resources,
    pub requires_receive_state: bool,
    pub capabilities: Capabilities,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivationPlan<const S: usize> {
    pub identity_resources: Resources,
    pub services: [Option<ServicePlan>; S],
}

impl<const S: usize> ActivationPlan<S> {
    pub fn new(identity_resources: Resources, services: &[ServicePlan]) -> Result<Self, Fault> {
        if services.len() > S {
            return Err(Fault::CapacityExceeded);
        }
        let mut result = Self {
            identity_resources,
            services: [None; S],
        };
        for (index, service) in services.iter().copied().enumerate() {
            if services[..index]
                .iter()
                .any(|prior| prior.service == service.service)
            {
                return Err(Fault::InvalidPlan);
            }
            result.services[index] = Some(service);
        }
        result.total()?;
        Ok(result)
    }

    pub fn total(&self) -> Result<Resources, Fault> {
        self.services
            .iter()
            .flatten()
            .try_fold(self.identity_resources, |total, service| {
                total.checked_add(service.resources).ok_or(Fault::Exhausted)
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnerToken {
    pub(crate) incarnation: [u8; 16],
    pub(crate) sequence: u64,
}

impl OwnerToken {
    pub const fn incarnation(self) -> [u8; 16] {
        self.incarnation
    }
    pub const fn sequence(self) -> u64 {
        self.sequence
    }
}

/// Ephemeral authority to drain this exact runtime table. It authorizes no
/// durable lifecycle transition and cannot be constructed from a catalog ID.
/// Runtime incarnations must never be reused while old capabilities exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminationToken {
    pub(crate) incarnation: [u8; 16],
}
impl TerminationToken {
    pub const fn incarnation(self) -> [u8; 16] {
        self.incarnation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Preflight,
    PrepareService,
    ActivateService,
    FencePreviousWorkers,
    QuiesceService,
    ReleaseIdentity,
}

/// An idempotency/fencing handle for one driver step. The identity ID is the
/// ownership key; matching only the public identity hash is insufficient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub owner: OwnerToken,
    pub identity: IdentityGuard,
    /// Original lifecycle provenance. During termination this may no longer be
    /// current catalog authority; only the exact runtime cleanup scope applies.
    pub lifecycle: EffectTicket,
    /// Present only for fencing/draining already owned resources. This scope is
    /// disjoint from ordinary effect authority, even at an exhausted generation.
    pub termination: Option<TerminationToken>,
    pub generation: u64,
    pub service: Option<ServiceId>,
    pub step: Step,
    /// Recovery must reconcile old tagged resources and fence prior workers;
    /// it may not infer ownership from a shared transport/management key.
    pub recovering: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Evidence<T> {
    pub scope: Scope,
    pub value: T,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress<T> {
    Pending,
    Complete(T),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedService {
    pub storage_ready: bool,
    pub receive_state_restored: bool,
}

/// These are completion observations, not successful command submissions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuiescenceEvidence {
    pub admissions_closed: bool,
    pub in_flight_mutations: u32,
    pub incoming_links: u32,
    pub outgoing_links: u32,
    pub queued_egress: u32,
    pub scheduled_announces: u32,
    pub registered_destinations: u32,
    pub request_handlers: u32,
    pub receive_state_durable: bool,
    pub key_handles_reconciled: bool,
}

impl QuiescenceEvidence {
    pub const COMPLETE: Self = Self {
        admissions_closed: true,
        in_flight_mutations: 0,
        incoming_links: 0,
        outgoing_links: 0,
        queued_egress: 0,
        scheduled_announces: 0,
        registered_destinations: 0,
        request_handlers: 0,
        receive_state_durable: true,
        key_handles_reconciled: true,
    };

    pub(crate) fn complete(self, requires_receive_state: bool) -> bool {
        self.admissions_closed
            && self.in_flight_mutations == 0
            && self.incoming_links == 0
            && self.outgoing_links == 0
            && self.queued_egress == 0
            && self.scheduled_announces == 0
            && self.registered_destinations == 0
            && self.request_handlers == 0
            && (!requires_receive_state || self.receive_state_durable)
            && self.key_handles_reconciled
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuiescedService {
    pub evidence: QuiescenceEvidence,
    pub dependencies: ServiceDependency,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReleasedIdentity<const S: usize> {
    pub owned_reservations_released: bool,
    pub shared_owners_preserved: bool,
    /// Re-read authoritative service dependencies after all drain barriers.
    /// The driver must hold those barriers through catalog acknowledgement.
    pub report: DependencyReport<S>,
}

/// Synchronous, idempotent steps over the supplied ownership scope. Async
/// adapters return Pending and later poll a result carrying the exact Scope.
/// Every actual worker dispatch must revalidate the complete scope under the
/// owner gate. A termination scope grants only exact owned-resource cleanup,
/// even if its original catalog ticket is stale or unavailable. It never grants
/// fresh effects or permits shutting down a host, interfaces, or another owner.
pub trait ScopedDriver<const S: usize> {
    type Error;
    fn preflight(
        &mut self,
        scope: Scope,
        plan: &ActivationPlan<S>,
    ) -> Result<Progress<Evidence<Resources>>, Self::Error>;
    fn prepare_service(
        &mut self,
        scope: Scope,
        key: KeyReference,
        plan: ServicePlan,
    ) -> Result<Progress<Evidence<PreparedService>>, Self::Error>;
    fn activate_service(
        &mut self,
        scope: Scope,
        plan: ServicePlan,
    ) -> Result<Progress<Evidence<()>>, Self::Error>;
    fn fence_previous_workers(
        &mut self,
        scope: Scope,
    ) -> Result<Progress<Evidence<bool>>, Self::Error>;
    fn quiesce_service(
        &mut self,
        scope: Scope,
        plan: ServicePlan,
    ) -> Result<Progress<Evidence<QuiescedService>>, Self::Error>;
    fn release_identity(
        &mut self,
        scope: Scope,
    ) -> Result<Progress<Evidence<ReleasedIdentity<S>>>, Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MutationId(pub [u8; 16]);

/// Dropping this value never cancels or releases the admitted mutation. Only a
/// definitive owner settlement can drain it. The service owns durable request-ID
/// deduplication; a lease is not a substitute for its durable retry record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MutationLease {
    pub(crate) sequence: u64,
    pub(crate) owner: OwnerToken,
    pub(crate) service: ServiceId,
    pub(crate) operation: MutationId,
    pub(crate) admission: AdmissionGuard,
}

impl MutationLease {
    pub const fn owner(self) -> OwnerToken {
        self.owner
    }
    pub const fn service(self) -> ServiceId {
        self.service
    }
    pub const fn operation(self) -> MutationId {
        self.operation
    }
    pub const fn admission(self) -> AdmissionGuard {
        self.admission
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settlement {
    Committed,
    Rejected,
    /// The physical attempt ended and a confirmed durable domain record now
    /// owns its unresolved recovery. This does not claim a final domain outcome.
    RecoveryRetained,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkPermit {
    pub(crate) owner: OwnerToken,
    pub(crate) service: ServiceId,
    pub(crate) admission: AdmissionGuard,
    pub(crate) generation: u64,
}

impl NetworkPermit {
    pub const fn owner(self) -> OwnerToken {
        self.owner
    }
    pub const fn service(self) -> ServiceId {
        self.service
    }
    pub const fn admission(self) -> AdmissionGuard {
        self.admission
    }
    pub const fn generation(self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivationReceipt {
    pub(crate) owner: OwnerToken,
    pub(crate) ticket: EffectTicket,
    pub(crate) generation: u64,
}

impl ActivationReceipt {
    pub const fn ticket(self) -> EffectTicket {
        self.ticket
    }
    pub const fn owner(self) -> OwnerToken {
        self.owner
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuiescenceReceipt<const S: usize> {
    pub(crate) termination: Option<TerminationToken>,
    pub(crate) owner: OwnerToken,
    pub(crate) ticket: EffectTicket,
    pub(crate) generation: u64,
    pub(crate) report: DependencyReport<S>,
}

impl<const S: usize> QuiescenceReceipt<S> {
    /// A termination receipt must go only to `confirm_termination`, never to a
    /// catalog acknowledgment or `confirm_quiescence`.
    pub const fn termination(self) -> Option<TerminationToken> {
        self.termination
    }
    pub const fn ticket(self) -> EffectTicket {
        self.ticket
    }
    pub const fn report(self) -> DependencyReport<S> {
        self.report
    }
    pub const fn owner(self) -> OwnerToken {
        self.owner
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    FencingActivation,
    Preflighting,
    Activating,
    AwaitingCatalogCommit,
    Running,
    FencingPreviousWorkers,
    Quiescing,
    Quiesced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    InvalidBudget,
    InvalidIncarnation,
    InvalidPlan,
    UnsupportedService,
    CapacityExceeded,
    Exhausted,
    UnknownIdentity,
    UnknownService,
    WrongIdentity,
    Busy,
    WrongPhase,
    OwnerStillActive,
    StaleOwner,
    StaleEffect,
    StaleEvidence,
    InvalidEvidence,
    MutationsPending,
    DuplicateMutation,
    StaleLease,
    InvalidAdmission,
    Terminating,
    StorageNotReady,
    AuthorityUnavailable,
    Authority(prns_identities_core::Fault),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    Fault(Fault),
    Driver(E),
}

impl<E> From<Fault> for Error<E> {
    fn from(value: Fault) -> Self {
        Self::Fault(value)
    }
}

pub(crate) fn authority_error(error: prns_identities_core::Fault) -> Fault {
    Fault::Authority(error)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServicePhase {
    Declared,
    Prepared,
    Active,
    Quiesced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ServiceState {
    pub plan: ServicePlan,
    pub phase: ServicePhase,
    pub dependencies: Option<ServiceDependency>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Owner<const S: usize> {
    pub token: OwnerToken,
    pub identity: IdentityGuard,
    pub ticket: EffectTicket,
    pub generation: u64,
    pub key: Option<KeyReference>,
    pub plan: ActivationPlan<S>,
    pub services: [Option<ServiceState>; S],
    pub phase: Phase,
    pub reservation: Resources,
    pub recovering: bool,
    pub report: Option<DependencyReport<S>>,
}

/// Caller-owned runtime bookkeeping. Recreating a Controller around this value
/// preserves resources, in-flight mutations and generation fences. Do not replace
/// it while old workers exist; after process loss a fresh incarnation requires
/// qualified `recover_quiescence` before adopting external resources.
pub struct RuntimeState<const I: usize, const S: usize, const L: usize> {
    pub(crate) incarnation: [u8; 16],
    pub(crate) termination: Option<TerminationToken>,
    pub(crate) budget: Budget,
    pub(crate) used: Resources,
    pub(crate) next_owner: u64,
    pub(crate) next_lease: u64,
    pub(crate) owners: [Option<Owner<S>>; I],
    /// One cleanup-only owner, requiring no new activation resources. While
    /// present, unknown external resource debt blocks further activation.
    pub(crate) cleanup: Option<Owner<S>>,
    pub(crate) leases: [Option<MutationLease>; L],
}

impl<const I: usize, const S: usize, const L: usize> RuntimeState<I, S, L> {
    pub fn new(incarnation: [u8; 16], budget: Budget) -> Result<Self, Fault> {
        if incarnation == [0; 16] {
            return Err(Fault::InvalidIncarnation);
        }
        budget.application_capacity()?;
        Ok(Self {
            incarnation,
            termination: None,
            budget,
            used: Resources::ZERO,
            next_owner: 0,
            next_lease: 0,
            owners: [None; I],
            cleanup: None,
            leases: [None; L],
        })
    }

    pub const fn is_terminating(&self) -> bool {
        self.termination.is_some()
    }
    /// Actual retained owners, including cleanup-only debt. This remains usable
    /// when the catalog cannot be read; it grants no fresh service admission.
    pub fn owner_tokens(&self) -> impl Iterator<Item = OwnerToken> + '_ {
        self.owners
            .iter()
            .flatten()
            .chain(self.cleanup.iter())
            .map(|owner| owner.token)
    }
    pub const fn reserved_resources(&self) -> Resources {
        self.used
    }
    pub const fn budget(&self) -> Budget {
        self.budget
    }
    pub fn owned_identities(&self) -> usize {
        self.owners.iter().flatten().count() + usize::from(self.cleanup.is_some())
    }
    pub const fn cleanup_pending(&self) -> bool {
        self.cleanup.is_some()
    }
    pub fn in_flight_mutations(&self, identity: LocalIdentityId) -> usize {
        self.leases
            .iter()
            .flatten()
            .filter(|lease| lease.admission.identity.id == identity)
            .count()
    }
}
