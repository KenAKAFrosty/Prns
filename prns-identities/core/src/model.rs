//! Public bounded values. Storage adapters may encode these fields explicitly;
//! every loaded catalog is validated before it grants any authority.

pub const CATALOG_FORMAT: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityLocalId"))]
pub struct LocalIdentityId {
    pub incarnation: [u8; 16],
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyReference(pub [u8; 32]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicIdentityHash(pub [u8; 16]);

/// Stable catalog-local identity binding. This selects an identity, never grants
/// lifecycle or service authority; each operation checks current authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityReference"))]
pub struct IdentityReference {
    pub id: LocalIdentityId,
    pub public_hash: PublicIdentityHash,
}

/// Supplied by the embedding application's canonical command encoder. The core
/// ALSO retains and compares the complete command, so digest collisions cannot
/// change the command attached to a token. This is not a secret capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandDigest(pub [u8; 32]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityOperationToken"))]
pub struct OperationToken {
    pub incarnation: [u8; 16],
    pub sequence: u64,
    pub digest: CommandDigest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoundedText<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> BoundedText<N> {
    pub fn new(text: &str) -> Result<Self, Fault> {
        if text.len() > N || text.chars().any(char::is_control) {
            return Err(Fault::InvalidMetadata);
        }
        let mut result = Self {
            bytes: [0; N],
            length: text.len(),
        };
        result.bytes[..text.len()].copy_from_slice(text.as_bytes());
        Ok(result)
    }

    pub fn as_str(&self) -> &str {
        // Only the checked constructor can populate the private representation.
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityMetadata"))]
pub struct Metadata {
    pub local_label: BoundedText<64>,
    pub published_name: BoundedText<128>,
}

impl Metadata {
    pub fn new(local_label: &str, published_name: Option<&str>) -> Result<Self, Fault> {
        if local_label.is_empty() {
            return Err(Fault::InvalidMetadata);
        }
        Ok(Self {
            local_label: BoundedText::new(local_label)?,
            published_name: BoundedText::new(published_name.unwrap_or(""))?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceSet<const S: usize> {
    pub entries: [Option<ServiceId>; S],
}

impl<const S: usize> ServiceSet<S> {
    pub const fn empty() -> Self {
        Self { entries: [None; S] }
    }

    pub fn new(services: &[ServiceId]) -> Result<Self, Fault> {
        if services.len() > S {
            return Err(Fault::CapacityExceeded);
        }
        let mut result = Self::empty();
        for (index, service) in services.iter().copied().enumerate() {
            if services[..index].contains(&service) {
                return Err(Fault::InvalidServices);
            }
            result.entries[index] = Some(service);
        }
        Ok(result)
    }

    pub fn iter(&self) -> impl Iterator<Item = ServiceId> + '_ {
        self.entries.iter().flatten().copied()
    }
}

/// An authoritative service-owner observation, never a UI progress projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(
    feature = "uniffi-bindings",
    uniffi(name = "IdentityServiceDependency")
)]
pub struct ServiceDependency {
    pub service: ServiceId,
    /// Revision of authoritative durable dependency facts, not transient runtime
    /// progress. Merely inspecting or quiescing an unchanged service must not
    /// increment this; changed accepted work/ownership/recovery facts must.
    pub revision: u64,
    pub pending_messages: u32,
    pub unsettled_control: u32,
    pub authority_count: u32,
    pub recovery_pending: bool,
    /// Includes draining admitted mutations, durable receive-key preservation,
    /// scoped ingress/egress shutdown and restoration before reopening ingress.
    pub quiescence_supported: bool,
}

impl ServiceDependency {
    pub const fn clear(service: ServiceId, revision: u64) -> Self {
        Self {
            service,
            revision,
            pending_messages: 0,
            unsettled_control: 0,
            authority_count: 0,
            recovery_pending: false,
            quiescence_supported: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DependencyReport<const S: usize> {
    pub entries: [Option<ServiceDependency>; S],
    pub complete: bool,
    /// Advisory only: this catalog never tears down infrastructure key owners.
    pub other_key_uses: u32,
}

impl<const S: usize> DependencyReport<S> {
    pub const fn empty() -> Self {
        Self {
            entries: [None; S],
            complete: true,
            other_key_uses: 0,
        }
    }

    /// Restore exact bounded slot placement when crossing an allocating native
    /// boundary. Reservations compare full commands, so sparse reports must not
    /// be compacted during inspection/replay.
    pub fn from_slots(
        entries: &[Option<ServiceDependency>],
        complete: bool,
        other_key_uses: u32,
    ) -> Result<Self, Fault> {
        if entries.len() > S {
            return Err(Fault::CapacityExceeded);
        }
        for (index, entry) in entries.iter().enumerate() {
            if let Some(entry) = entry {
                if entries[..index]
                    .iter()
                    .flatten()
                    .any(|prior| prior.service == entry.service)
                {
                    return Err(Fault::InvalidServices);
                }
            }
        }
        let mut report = Self::empty();
        report.entries[..entries.len()].copy_from_slice(entries);
        report.complete = complete;
        report.other_key_uses = other_key_uses;
        Ok(report)
    }

    pub fn new(entries: &[ServiceDependency], other_key_uses: u32) -> Result<Self, Fault> {
        if entries.len() > S {
            return Err(Fault::CapacityExceeded);
        }
        let mut report = Self::empty();
        report.other_key_uses = other_key_uses;
        for (index, entry) in entries.iter().copied().enumerate() {
            if entries[..index].iter().any(|e| e.service == entry.service) {
                return Err(Fault::InvalidServices);
            }
            report.entries[index] = Some(entry);
        }
        Ok(report)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityDesiredState"))]
pub enum DesiredState {
    Enabled,
    Paused,
    Retired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityTransitionTarget"))]
pub enum TransitionTarget {
    Enable,
    Pause,
    Retire,
    AddService(ServiceId),
    RemoveService(ServiceId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityGuard"))]
pub struct IdentityGuard {
    pub id: LocalIdentityId,
    pub public_hash: PublicIdentityHash,
    pub lifecycle_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityMetadataGuard"))]
pub struct MetadataGuard {
    pub id: LocalIdentityId,
    pub public_hash: PublicIdentityHash,
    pub metadata_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedTransition<const S: usize> {
    pub identity: IdentityGuard,
    pub target: TransitionTarget,
    pub review: DependencyReport<S>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityKeySource"))]
pub enum KeySource {
    Generate,
    Import { expected_hash: PublicIdentityHash },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreateRequest<const S: usize> {
    pub source: KeySource,
    pub metadata: Metadata,
    pub services: ServiceSet<S>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<const S: usize> {
    Create(CreateRequest<S>),
    Transition(PreparedTransition<S>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityServiceFailure"))]
pub enum ServiceFailure {
    CapacityExceeded,
    StorageUnavailable,
    Unavailable(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityRetirementBlock"))]
pub enum RetirementBlock {
    ChangedDependencies,
    IncompleteReport,
    UnqualifiedService,
    RecoveryPending,
    AuthorityOwned,
    UnsettledControl,
    PendingMessages,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityOperationResult"))]
pub enum OperationResult {
    Created {
        identity: LocalIdentityId,
    },
    Existing {
        identity: LocalIdentityId,
    },
    Enabled {
        identity: LocalIdentityId,
    },
    Paused {
        identity: LocalIdentityId,
    },
    Retired {
        identity: LocalIdentityId,
    },
    RetirementBlocked {
        identity: LocalIdentityId,
        reason: RetirementBlock,
    },
    ActivationFailed {
        identity: LocalIdentityId,
        reason: ServiceFailure,
    },
    ServiceAdded {
        identity: LocalIdentityId,
        service: ServiceId,
    },
    ServiceRemoved {
        identity: LocalIdentityId,
        service: ServiceId,
    },
    ServiceRemovalBlocked {
        identity: LocalIdentityId,
        service: ServiceId,
        reason: RetirementBlock,
    },
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityOperationState"))]
pub enum OperationState {
    Reserved,
    AwaitingKey,
    Transitioning,
    Completed(OperationResult),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationRecord<const S: usize> {
    pub token: OperationToken,
    pub command: Command<S>,
    pub state: OperationState,
    /// A duplicate import may have durably written a new vault reference before
    /// learning that another publication won. It is never silently erased. This
    /// fence prevents pruning until the adapter durably accounts for that key.
    pub unpublished_key: Option<KeyReference>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityTransitionPhase"))]
pub enum TransitionPhase {
    Activating,
    Quiescing,
    ActivationCleanup(ServiceFailure),
    InitializingService,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(
    feature = "uniffi-bindings",
    uniffi(name = "IdentityPendingTransition")
)]
pub struct PendingTransition {
    pub operation: OperationToken,
    pub target: TransitionTarget,
    pub phase: TransitionPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityRecord<const S: usize> {
    pub id: LocalIdentityId,
    pub public_hash: PublicIdentityHash,
    pub key_reference: KeyReference,
    pub metadata: Metadata,
    pub metadata_revision: u64,
    pub lifecycle_revision: u64,
    pub desired: DesiredState,
    /// Immutable creation provenance, independent of later configuration changes.
    pub initial_services: ServiceSet<S>,
    pub services: ServiceSet<S>,
    pub pending: Option<PendingTransition>,
    /// Reserved per-identity journal capacity for partial runtime restoration.
    /// This is independent of operation-result slots and never consumes one.
    pub recovery_cleanup: Option<ServiceFailure>,
}

impl<const S: usize> IdentityRecord<S> {
    pub const fn reference(&self) -> IdentityReference {
        IdentityReference {
            id: self.id,
            public_hash: self.public_hash,
        }
    }

    pub fn guard(&self) -> IdentityGuard {
        IdentityGuard {
            id: self.id,
            public_hash: self.public_hash,
            lifecycle_revision: self.lifecycle_revision,
        }
    }

    pub fn metadata_guard(&self) -> MetadataGuard {
        MetadataGuard {
            id: self.id,
            public_hash: self.public_hash,
            metadata_revision: self.metadata_revision,
        }
    }
}

/// Two buffers of this type are supplied to the coordinator by its caller.
/// It deliberately is not `Copy`: mutation copies directly into caller staging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog<const I: usize, const O: usize, const S: usize> {
    pub format: u16,
    pub incarnation: [u8; 16],
    pub revision: u64,
    pub session: u64,
    pub issued_sequence: u64,
    pub completed_floor: u64,
    pub identities: [Option<IdentityRecord<S>>; I],
    pub operations: [Option<OperationRecord<S>>; O],
}

impl<const I: usize, const O: usize, const S: usize> Catalog<I, O, S> {
    /// Incarnation must be new for a new catalog, never reused after data loss.
    pub const fn new(incarnation: [u8; 16]) -> Self {
        Self {
            format: CATALOG_FORMAT,
            incarnation,
            revision: 0,
            session: 0,
            issued_sequence: 0,
            completed_floor: 0,
            identities: [None; I],
            operations: [None; O],
        }
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn identities(&self) -> impl Iterator<Item = &IdentityRecord<S>> {
        self.identities.iter().flatten()
    }

    pub fn operations(&self) -> impl Iterator<Item = &OperationRecord<S>> {
        self.operations.iter().flatten()
    }

    pub fn copy_from(&mut self, source: &Self) {
        self.format = source.format;
        self.incarnation = source.incarnation;
        self.revision = source.revision;
        self.session = source.session;
        self.issued_sequence = source.issued_sequence;
        self.completed_floor = source.completed_floor;
        self.identities.copy_from_slice(&source.identities);
        self.operations.copy_from_slice(&source.operations);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectKind {
    PublishKey,
    Activate,
    Quiesce,
    Restore,
    RestoreCleanup,
    InitializeService,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectTicket {
    pub operation: Option<OperationToken>,
    pub identity: LocalIdentityId,
    pub session: u64,
    pub lifecycle_revision: u64,
    pub kind: EffectKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action<const S: usize> {
    /// Explicitly initialize or reconcile durable state for an absent service.
    /// The stable operation identifies this preparation across process sessions.
    /// Do not activate workers or retain live resources after the driver returns.
    InitializeService {
        ticket: EffectTicket,
        identity: IdentityGuard,
        key_reference: KeyReference,
        service: ServiceId,
    },
    /// Idempotently create/import this reference or reopen its existing key.
    /// Never generate a replacement on an uncertain result; verify durability
    /// and the key-derived public hash before acknowledgement.
    PublishKey {
        ticket: EffectTicket,
        key_reference: KeyReference,
        source: KeySource,
    },
    /// Preflight all resources; restore retained receive state before ingress.
    /// On any failure call `activation_failed`, then perform scoped cleanup.
    Activate {
        ticket: EffectTicket,
        identity: IdentityGuard,
        key_reference: KeyReference,
        services: ServiceSet<S>,
    },
    /// Fence admissions and drain prior mutations before reporting dependencies.
    /// Stop only this identity's application services, never the host or vault.
    Quiesce {
        ticket: EffectTicket,
        identity: IdentityGuard,
        services: ServiceSet<S>,
    },
}

impl<const S: usize> Action<S> {
    pub fn ticket(&self) -> EffectTicket {
        match self {
            Self::InitializeService { ticket, .. }
            | Self::PublishKey { ticket, .. }
            | Self::Activate { ticket, .. }
            | Self::Quiesce { ticket, .. } => *ticket,
        }
    }
}

/// Trusted domain witness: its current supported schema and immutable identity
/// binding are durably prepared, and the preparation retains no live resources.
/// A driver may return this only after actual storage completion, never merely
/// queueing work. Ordinary activation/reopen must not initialize missing state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitializedService {
    pub ticket: EffectTicket,
    pub service: ServiceId,
}

/// Trusted adapter witness, not a proof checked by this non-cryptographic core.
/// The adapter must read/verify the durable key, not an in-memory generated key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedKey {
    pub key_reference: KeyReference,
    pub public_hash: PublicIdentityHash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionClass {
    History,
    /// Only exact-result lookup/settlement of previously admitted records. The
    /// service must verify that durable record ownership; never fresh execution.
    Recovery,
    /// Private local contacts, drafts and settings only. Durable publication,
    /// enrollment and other eventual network work use `ServiceMutation`, even
    /// when their immediate implementation is just a local database insertion.
    LocalMutation,
    /// Explicit scoped contact-card signing, without service activation.
    OfflineSigning,
    /// Fresh durable service work such as publication/enrollment. Enabled is
    /// required, but transport need not be ready; the service separately checks
    /// its durable-store readiness before accepting offline intent.
    ServiceMutation,
    Network,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionGuard {
    pub identity: IdentityGuard,
    pub session: u64,
    pub class: AdmissionClass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityRuntimeStatus"))]
pub enum RuntimeStatus {
    Starting,
    Running,
    Stopping,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityFault"))]
pub enum Fault {
    Poisoned,
    InvalidCatalog,
    InvalidMetadata,
    InvalidServices,
    InvalidTransition,
    CapacityExceeded,
    Exhausted,
    UnknownIdentity,
    IdentityChanged,
    StaleLifecycle,
    StaleMetadata,
    StaleSession,
    StaleEffect,
    InvalidToken,
    ReplayExpired,
    UnknownOperation,
    CommandMismatch,
    AlreadyStarted,
    Busy,
    Retired,
    Paused,
    RuntimeUnavailable,
    InvalidReport,
    IncompleteReport,
    UnqualifiedService,
    KeyMismatch,
    MissingKey,
    CannotPrunePending,
    UnaccountedKey,
    InvalidPruneFloor,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    Fault(Fault),
    Storage(E),
}

impl<E> From<Fault> for Error<E> {
    fn from(value: Fault) -> Self {
        Self::Fault(value)
    }
}
