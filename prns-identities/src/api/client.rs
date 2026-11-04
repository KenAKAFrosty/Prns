//! One native client implementation for Rust, generated bindings, and selected
//! compositions. Admission occurs on first poll. Once submitted, losing this
//! future only loses an observer; the worker retains the job and its effects.
use super::*;
use std::sync::Arc;
use tokio::sync::oneshot;

type Sender<T> = oneshot::Sender<Result<T, ApiError>>;

/// Opaque identity work for the existing exclusive owner lane. Embeddings can
/// carry this item but cannot forge its response or alter its command.
pub struct IdentityJob {
    pub(crate) message: Message,
}
pub(crate) enum Message {
    List(Sender<Vec<IdentityRecord>>),
    Operations(Sender<Vec<OperationRecord>>),
    FindReserved(Command, CommandDigest, Sender<Option<OperationToken>>),
    CancelReserved(OperationToken, Sender<()>),
    Prune(u64, Sender<()>),
    Scope(Sender<CreateRequestScope>),
    Create(
        CreateRequestId,
        CreateRequest,
        Option<Arc<ImportKey>>,
        Sender<CreateOutcome>,
    ),
    Retire(Vec<CreateRequestId>, Sender<CreateRequestScope>),
    Snapshot(IdentityReference, Sender<Snapshot>),
    #[cfg(feature = "contact-card")]
    ContactCard(
        IdentityReference,
        crate::contact_card::ServiceNameHash,
        String,
        Sender<Vec<u8>>,
    ),
    Prepare(
        IdentityReference,
        TransitionTarget,
        Sender<PreparedTransition>,
    ),
    Reserve(PreparedTransition, CommandDigest, Sender<OperationToken>),
    Begin(
        IdentityReference,
        OperationToken,
        PreparedTransition,
        Sender<OperationState>,
    ),
    Stop(
        IdentityReference,
        PreparedTransition,
        CommandDigest,
        Sender<OperationToken>,
    ),
    Operation(OperationToken, Command, Sender<OperationState>),
    Recover(IdentityReference, Sender<()>),
    Metadata(
        IdentityReference,
        MetadataGuard,
        Metadata,
        Sender<MetadataGuard>,
    ),
}
impl IdentityJob {
    pub(crate) fn identity_reference(&self) -> Option<IdentityReference> {
        let prepared = |value: &PreparedTransition| IdentityReference {
            id: value.identity.id,
            public_hash: value.identity.public_hash,
        };
        match &self.message {
            Message::Snapshot(reference, _)
            | Message::Prepare(reference, ..)
            | Message::Begin(reference, ..)
            | Message::Stop(reference, ..)
            | Message::Recover(reference, _)
            | Message::Metadata(reference, ..) => Some(*reference),
            #[cfg(feature = "contact-card")]
            Message::ContactCard(reference, ..) => Some(*reference),
            Message::Reserve(value, ..) => Some(prepared(value)),
            Message::FindReserved(Command::Transition(value), ..)
            | Message::Operation(_, Command::Transition(value), _) => Some(prepared(value)),
            _ => None,
        }
    }
    pub(crate) fn uses_control_lane(&self) -> bool {
        let stopping = |target| {
            matches!(
                target,
                TransitionTarget::Pause
                    | TransitionTarget::Retire
                    | TransitionTarget::RemoveService(_)
            )
        };
        match &self.message {
            Message::Prepare(_, target, _) => stopping(*target),
            Message::Stop(..) => true,
            Message::Reserve(prepared, ..) | Message::Begin(_, _, prepared, _) => {
                stopping(prepared.target)
            }
            _ => false,
        }
    }
}
/// The constructor must bind the actual owner's ordinary and control lanes.
/// Queue rejection means no job was admitted. Successful submission transfers
/// ownership of execution even if the client or result future is then dropped.
pub trait Submit: Send + Sync {
    fn submit(&self, job: IdentityJob) -> Result<(), SubmissionFailure>;
}

/// A secret capability with no foreign byte accessor or constructor. Native
/// callers may import their existing zeroizing key; a key-lease adapter can
/// retain its source owner until the import job has finished using it.
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Object))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityImportKey"))]
pub struct ImportKey {
    secret: crate::IdentitySecretKey,
    _source: Option<crate::keys::KeyLease>,
}
impl From<crate::IdentitySecretKey> for ImportKey {
    fn from(secret: crate::IdentitySecretKey) -> Self {
        Self {
            secret,
            _source: None,
        }
    }
}
impl ImportKey {
    /// Native capability conversion. Deliberately outside the exported API.
    pub fn from_lease(lease: crate::keys::KeyLease) -> Self {
        let secret = crate::IdentitySecretKey::new(*lease.material().as_bytes());
        Self {
            secret,
            _source: Some(lease),
        }
    }
    pub(crate) fn secret(&self) -> &crate::IdentitySecretKey {
        &self.secret
    }
}

pub type Client = IdentityCatalogClient;

#[derive(Clone)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Object))]
pub struct IdentityCatalogClient {
    submit: Arc<dyn Submit>,
    operation_capacity: usize,
    service_capacity: usize,
}
impl IdentityCatalogClient {
    /// The capacities must be the exact const bounds used by the native owner.
    /// This constructor creates no worker, storage, key, or runtime.
    pub fn new(
        submit: Arc<dyn Submit>,
        operation_capacity: usize,
        service_capacity: usize,
    ) -> Self {
        Self {
            submit,
            operation_capacity,
            service_capacity,
        }
    }
    async fn request<T>(&self, build: impl FnOnce(Sender<T>) -> Message) -> Result<T, ApiError> {
        let (sender, receiver) = oneshot::channel();
        self.submit.submit(IdentityJob {
            message: build(sender),
        })?;
        receiver.await.map_err(|_| ApiError::WorkerStopped)?
    }
    fn services(&self, count: usize) -> Result<(), ApiError> {
        if count > self.service_capacity {
            Err(SubmissionFailure::Bounds.into())
        } else {
            Ok(())
        }
    }
    fn command(&self, command: &Command) -> Result<(), ApiError> {
        self.services(match command {
            Command::Create(value) => value.services.len(),
            Command::Transition(value) => value.review.entries.len(),
        })
    }
}
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
impl IdentityCatalogClient {
    pub async fn identities(&self) -> Result<Vec<IdentityRecord>, ApiError> {
        self.request(Message::List).await
    }
    pub async fn operations(&self) -> Result<Vec<OperationRecord>, ApiError> {
        self.request(Message::Operations).await
    }
    pub async fn find_reserved(
        &self,
        command: Command,
        digest: CommandDigest,
    ) -> Result<Option<OperationToken>, ApiError> {
        self.command(&command)?;
        self.request(|tx| Message::FindReserved(command, digest, tx))
            .await
    }
    pub async fn cancel_reserved(&self, token: OperationToken) -> Result<(), ApiError> {
        self.request(|tx| Message::CancelReserved(token, tx)).await
    }
    /// Explicit confirmation; pending work and retained-key obligations still
    /// fence pruning. This never silently discards an uncertain result.
    pub async fn prune_completed(&self, through_sequence: u64) -> Result<(), ApiError> {
        self.request(|tx| Message::Prune(through_sequence, tx))
            .await
    }
    pub async fn creation_scope(&self) -> Result<CreateRequestScope, ApiError> {
        self.request(Message::Scope).await
    }
    /// Persist this exact ID and intent before polling. A new scope/nonce is new
    /// work, never an automatic recovery of an uncertain previous result.
    pub async fn create_or_recover(
        &self,
        id: CreateRequestId,
        intent: CreateRequest,
        import: Option<Arc<ImportKey>>,
    ) -> Result<CreateOutcome, ApiError> {
        self.services(intent.services.len())?;
        self.request(|tx| Message::Create(id, intent, import, tx))
            .await
    }
    pub async fn retire_creations(
        &self,
        confirmed: Vec<CreateRequestId>,
    ) -> Result<CreateRequestScope, ApiError> {
        if confirmed.len() > self.operation_capacity {
            return Err(SubmissionFailure::Bounds.into());
        }
        self.request(|tx| Message::Retire(confirmed, tx)).await
    }
    /// Inert reservation. Persist its token and exact prepared input before
    /// begin. Losing this reply cannot begin a lifecycle effect.
    pub async fn reserve(
        &self,
        prepared: PreparedTransition,
        digest: CommandDigest,
    ) -> Result<OperationToken, ApiError> {
        self.services(prepared.review.entries.len())?;
        self.request(|tx| Message::Reserve(prepared, digest, tx))
            .await
    }
    pub async fn operation(
        &self,
        token: OperationToken,
        command: Command,
    ) -> Result<OperationState, ApiError> {
        self.command(&command)?;
        self.request(|tx| Message::Operation(token, command, tx))
            .await
    }
    /// Construct an inert bound handle. The fallible boundary preserves typed
    /// representation errors from foreign callers; existence and current
    /// authority are checked by each submitted operation.
    pub fn identity(&self, reference: IdentityReference) -> Result<IdentityClient, ApiError> {
        Ok(IdentityClient {
            client: self.clone(),
            reference,
        })
    }
}

#[derive(Clone)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Object))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityClient"))]
pub struct IdentityClient {
    client: IdentityCatalogClient,
    reference: IdentityReference,
}
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
impl IdentityClient {
    pub fn reference(&self) -> IdentityReference {
        self.reference
    }
    pub async fn snapshot(&self) -> Result<Snapshot, ApiError> {
        self.client
            .request(|tx| Message::Snapshot(self.reference, tx))
            .await
    }
    pub async fn prepare(&self, target: TransitionTarget) -> Result<PreparedTransition, ApiError> {
        self.client
            .request(|tx| Message::Prepare(self.reference, target, tx))
            .await
    }
    pub async fn begin(
        &self,
        token: OperationToken,
        prepared: PreparedTransition,
    ) -> Result<OperationState, ApiError> {
        self.client.services(prepared.review.entries.len())?;
        self.client
            .request(|tx| Message::Begin(self.reference, token, prepared, tx))
            .await
    }
    /// Pause/Retire/RemoveService use the reserved control queue and the core's
    /// stop journal capacity. Repeating the same input recovers its exact token.
    pub async fn apply_stop(
        &self,
        prepared: PreparedTransition,
        digest: CommandDigest,
    ) -> Result<OperationToken, ApiError> {
        self.client.services(prepared.review.entries.len())?;
        self.client
            .request(|tx| Message::Stop(self.reference, prepared, digest, tx))
            .await
    }
    pub async fn recover(&self) -> Result<(), ApiError> {
        self.client
            .request(|tx| Message::Recover(self.reference, tx))
            .await
    }
    pub async fn update_metadata(
        &self,
        expected: MetadataGuard,
        metadata: Metadata,
    ) -> Result<MetadataGuard, ApiError> {
        self.client
            .request(|tx| Message::Metadata(self.reference, expected, metadata, tx))
            .await
    }
}

#[cfg(feature = "contact-card")]
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
impl IdentityClient {
    /// Sign a service/name claim without activating an identity or service.
    /// The owner obtains fresh OfflineSigning authority at actual dispatch;
    /// paused identities are allowed, pending/retired identities are not.
    /// Returns canonical public card bytes, never a private signing capability.
    pub async fn contact_card(
        &self,
        service_name_hash: crate::contact_card::ServiceNameHash,
        display_name: String,
    ) -> Result<Vec<u8>, ApiError> {
        crate::contact_card::validate_name(&display_name)?;
        self.client
            .request(|tx| Message::ContactCard(self.reference, service_name_hash, display_name, tx))
            .await
    }
}
