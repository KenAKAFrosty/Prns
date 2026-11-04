use crate::{
    ApiError, ChangeToken, Conversation, Error, ExecutionContext, NetworkConfig, Outcome,
    RecordChanges, RecordDeliveryProgress, RecordPage, Request, StoreSession, StoredRecord,
};
#[cfg(feature = "text")]
use crate::{Message, MessagePage};
use prns_groups_reticulum::{SyncCursor, SyncOptions};
use prns_groups_service::{
    ClockReading, Endpoint, EnvelopeId, EnvelopeInfo, GroupId, Incoming, InventoryCursor,
    InventoryKind, InventoryPage, InventorySnapshot, RecordId,
};
use prns_groups_store::StoreIncarnation;
use std::sync::Arc;
use tokio::sync::{oneshot, Notify};

mod inputs;
pub(crate) use inputs::intent_heap_bytes;
pub use inputs::{ClientLimits, InputBudget};

/// Admission belongs to the existing database owner. Return Busy when its
/// bounded lane is full and Unavailable when that owner has stopped.
pub trait Submit: Send + Sync {
    /// The single shared input allowance retained by this owner/lane.
    fn input_budget(&self) -> &InputBudget;
    fn submit(&self, job: Job) -> Result<(), Error>;
    /// Native compositions may publish the retained runner's observation source
    /// independently of this client's submission lifetime. None uses this tree's
    /// own runner; Some(unknown) explicitly represents an inactive composition.
    #[cfg(feature = "network")]
    fn synchronization_progress(&self, _group: GroupId) -> Option<crate::GroupSyncProgress> {
        None
    }
}

pub struct Job {
    pub(crate) command: Command,
    pub(crate) group: Option<GroupId>,
    pub(crate) control: Option<prns_groups_core::ControlReference>,
    pub(crate) reply: oneshot::Sender<Result<Reply, Error>>,
    pub(crate) wake: Arc<Notify>,
    pub(crate) _input: Option<inputs::InputLease>,
}
impl Job {
    /// Narrow lifecycle-owner inspection. Catalog pages are pure reads; legacy
    /// configuration/carrier inspection can finish already admitted recovery.
    /// Only the exclusive lifecycle owner may use this mixed path after fencing
    /// ordinary client admissions; it is not a general read-only capability.
    pub fn is_lifecycle_inspection(&self) -> bool {
        matches!(
            &self.command,
            Command::Identity
                | Command::Conversations { .. }
                | Command::Conversation { .. }
                | Command::Members { .. }
                | Command::Invitation { .. }
                | Command::Invitations { .. }
                | Command::EnrollmentWork { .. }
        )
    }

    /// An admitted mutation commits even when its caller has dropped the reply.
    pub fn execute(self, session: &mut StoreSession<'_>) {
        let result = if self.group.is_some_and(|group| group != session.group())
            || self
                .control
                .is_some_and(|control| control != session.control())
        {
            Err(Error::Configuration)
        } else {
            session.execute(self.command)
        };
        session.publish_changes();
        Self::notify_commit(&self.wake, &result);
        let _ = self.reply.send(result);
    }
    pub(crate) fn notify_commit(wake: &Notify, result: &Result<Reply, Error>) {
        // The storage owner, rather than the reply waiter, owns this signal.
        // Cancellation after admission cannot strand an already committed send.
        if matches!(result, Ok(Reply::Outcome(_)))
            || matches!(result,Ok(Reply::Operation(Some(operation))) if matches!(operation.state,crate::retained::State::Completed(_)))
        {
            wake.notify_one();
        }
    }
    pub fn fail(self, error: Error) {
        let _ = self.reply.send(Err(error));
    }
}

#[derive(Clone)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Object))]
pub struct Client {
    submitter: Arc<dyn Submit>,
    pub(crate) wake: Arc<Notify>,
    group: Option<GroupId>,
    control: Option<prns_groups_core::ControlReference>,
    #[cfg(feature = "network")]
    pub(crate) progress: Arc<crate::progress::ProgressTracker>,
}
impl Client {
    #[cfg(feature = "network")]
    pub(crate) async fn bind_profile_support(
        &self,
        support: crate::ProfileSupport,
    ) -> Result<(), Error> {
        match self.call(Command::ProfileSupport(support)).await? {
            Reply::Done => Ok(()),
            _ => Err(Error::Corrupt),
        }
    }
    /// Read-only handle for a native composition to publish this runner's source.
    #[cfg(feature = "network")]
    pub fn synchronization_source(&self) -> crate::SyncProgressSource {
        crate::SyncProgressSource::new(self.progress.clone())
    }
    pub fn new(submitter: Arc<dyn Submit>) -> Self {
        Self {
            submitter,
            wake: Arc::new(Notify::new()),
            group: None,
            control: None,
            #[cfg(feature = "network")]
            progress: Arc::default(),
        }
    }
    pub fn for_group(&self, group: GroupId) -> Self {
        Self {
            group: Some(group),
            control: None,
            ..self.clone()
        }
    }
    /// Fence reads and carrier work to a membership. Mutations always use the
    /// explicit original_scope in their Request, independently of this view.
    pub fn at_control(&self, control: prns_groups_core::ControlReference) -> Self {
        Self {
            control: Some(control),
            ..self.clone()
        }
    }
    pub fn wake(&self) {
        self.wake.notify_one();
    }
    pub(crate) async fn call(&self, command: Command) -> Result<Reply, Error> {
        self.enqueue(command)?
            .await
            .map_err(|_| Error::Unavailable)?
    }
    fn enqueue(&self, command: Command) -> Result<oneshot::Receiver<Result<Reply, Error>>, Error> {
        // Reserve before structural validation so even validation work is
        // bounded by the selected input allocation policy.
        let input = self.submitter.input_budget().admit(&command)?;
        let intent = match &command {
            Command::Execute { request, .. } => {
                request.validate()?;
                Some(&request.intent)
            }
            Command::SubmitRetained { intent, .. } => {
                intent.validate()?;
                Some(intent)
            }
            _ => None,
        };
        if intent.is_some_and(|intent| {
            self.group
                .is_some_and(|bound| intent.group().is_some_and(|group| group != bound))
        }) {
            return Err(Error::Configuration);
        }
        let (reply, receiver) = oneshot::channel();
        self.submitter.submit(Job {
            group: if intent.is_some() { None } else { self.group },
            control: if intent.is_some() { None } else { self.control },
            command,
            reply,
            wake: Arc::clone(&self.wake),
            _input: Some(input),
        })?;
        Ok(receiver)
    }

    /// Queue ownership transfers synchronously. A dropped waiter cannot cancel
    /// accepted work; durable acceptance is reported only by its stored Operation.
    pub fn admit_submit(
        &self,
        id: crate::RequestId,
        intent: crate::Intent,
        context: ExecutionContext,
    ) -> Result<
        impl std::future::Future<Output = Result<crate::retained::Operation, ApiError>> + Send + 'static,
        ApiError,
    > {
        let receiver = self.enqueue(Command::SubmitRetained {
            id,
            intent,
            context,
        })?;
        Ok(async move {
            match receiver.await.map_err(|_| ApiError::Unavailable)?? {
                Reply::Operation(Some(operation)) => Ok(operation),
                _ => Err(ApiError::Corrupt),
            }
        })
    }
    /// Exact current configuration. Managed clients must first select a group
    /// with `for_group`; only fixed provisioning has an implicit singleton.
    pub async fn network_config(&self) -> Result<NetworkConfig, Error> {
        match self.call(Command::Config).await? {
            Reply::Config(value) => Ok(value),
            _ => Err(Error::Corrupt),
        }
    }

    async fn call_api(&self, command: Command) -> Result<Reply, ApiError> {
        self.call(command).await.map_err(ApiError::from)
    }

    /// Transfer live ownership synchronously to the selected owner. Dropping
    /// the returned waiter never cancels accepted work. Catalog-authorized native
    /// owners retain the exact Request before fresh domain dispatch; independent
    /// owners may require caller-owned persistence. Preserve the Request across
    /// ambiguous results; its execution context may be refreshed for recovery.
    pub fn admit_execute(
        &self,
        request: Request,
        context: ExecutionContext,
    ) -> Result<
        impl std::future::Future<Output = Result<Outcome, ApiError>> + Send + 'static,
        ApiError,
    > {
        #[cfg(feature = "network")]
        let publication_group = request.intent.group();
        let receiver = self.enqueue(Command::Execute { request, context })?;
        Ok(async move {
            match receiver.await.map_err(|_| ApiError::Unavailable)?? {
                Reply::Outcome(value) => {
                    #[cfg(feature = "network")]
                    if let (Some(group), Outcome::Published(record)) = (publication_group, value) {
                        crate::diagnostics::local_send_result(&group.0, &record.0);
                    }
                    Ok(value)
                }
                _ => Err(ApiError::Corrupt),
            }
        })
    }

    /// Read one bounded live page of enrollment work. Empty pages can carry a
    /// continuation; start a new traversal only after `next` is None.
    /// This does not run recovery or cleanup. Schedule `maintain` explicitly
    /// on the owner lane, independently of page traversal.
    pub async fn enrollment_work(
        &self,
        now: u64,
        after: Option<[u8; 32]>,
        limit: crate::PageLimits,
    ) -> Result<crate::enrollment::WorkPage, Error> {
        limit.validate().map_err(Error::Api)?;
        match self
            .call(Command::EnrollmentWork { now, after, limit })
            .await?
        {
            Reply::EnrollmentWork(value) => Ok(value),
            _ => Err(Error::Corrupt),
        }
    }
    pub async fn enrollment_handle(
        &self,
        requester: prns_host::IdentityHash,
        frame: Vec<u8>,
        now: u64,
    ) -> Result<Vec<u8>, Error> {
        match self
            .call(Command::EnrollmentHandle {
                requester,
                frame,
                now,
            })
            .await?
        {
            Reply::EnrollmentFrame(value) => Ok(value),
            _ => Err(Error::Corrupt),
        }
    }
    pub async fn enrollment_complete(
        &self,
        id: [u8; 32],
        response: Vec<u8>,
        now: u64,
    ) -> Result<crate::enrollment::CompletionActivity, Error> {
        match self
            .call(Command::EnrollmentComplete { id, response, now })
            .await?
        {
            Reply::EnrollmentCompleted(activity) => {
                if activity == crate::enrollment::CompletionActivity::Progress {
                    self.wake();
                }
                Ok(activity)
            }
            _ => Err(Error::Corrupt),
        }
    }
    #[cfg_attr(not(feature = "network"), allow(dead_code))]
    pub(crate) async fn pending_delivery(
        &self,
        peer: Endpoint,
        after: InventoryCursor,
        clock: ClockReading,
    ) -> Result<Option<EnvelopeInfo>, Error> {
        match self
            .call(Command::PendingDelivery { peer, after, clock })
            .await?
        {
            Reply::PendingDelivery(value) => Ok(value),
            _ => Err(Error::Corrupt),
        }
    }
    #[cfg_attr(not(feature = "network"), allow(dead_code))]
    pub(crate) async fn load_cursor(&self, peer: Endpoint) -> Result<Option<SyncCursor>, Error> {
        match self.call(Command::LoadCursor(peer)).await? {
            Reply::Cursor(value) => Ok(value),
            _ => Err(Error::Corrupt),
        }
    }
    #[cfg_attr(not(feature = "network"), allow(dead_code))]
    pub(crate) async fn save_cursor(
        &self,
        peer: Endpoint,
        cursor: Option<SyncCursor>,
    ) -> Result<(), Error> {
        match self.call(Command::SaveCursor(peer, cursor)).await? {
            Reply::Done => Ok(()),
            _ => Err(Error::Corrupt),
        }
    }
    /// Run one bounded maintenance pass on the owned storage lane.
    pub async fn maintain(&self, clock: ClockReading) -> Result<(), Error> {
        match self.call(Command::Maintain(clock)).await? {
            Reply::Done => Ok(()),
            _ => Err(Error::Corrupt),
        }
    }
    #[cfg_attr(not(feature = "network"), allow(dead_code))]
    pub(crate) async fn repair_receipts(&self) -> Result<(), Error> {
        match self.call(Command::RepairReceipts).await? {
            Reply::Done => Ok(()),
            _ => Err(Error::Corrupt),
        }
    }
}

#[cfg(feature = "network")]
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
impl Client {
    /// Bounded, disposable observations without a store read or network work.
    /// Compare the returned control with current detail. Unknown/partial state
    /// and observation age are not delivery evidence; use record receipts.
    pub fn synchronization_progress(&self, group: GroupId) -> crate::GroupSyncProgress {
        let mut value = self
            .submitter
            .synchronization_progress(group)
            .unwrap_or_else(|| self.progress.snapshot(group));
        if self.group.is_some_and(|selected| selected != group)
            || self
                .control
                .is_some_and(|control| value.control != Some(control))
        {
            value.observed = false;
            value.control = None;
            value.peers.clear();
        }
        value
    }
}

#[cfg(feature = "text")]
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
impl Client {
    /// Decode one retained text record in its exact membership incarnation.
    /// Non-text records and absent records return None, matching text history.
    pub async fn message(
        &self,
        group: GroupId,
        incarnation: StoreIncarnation,
        record: RecordId,
        clock: ClockReading,
    ) -> Result<Option<Message>, ApiError> {
        self.record(group, incarnation, record, clock)
            .await?
            .filter(|item| crate::RecordSelection::Text.includes(&item.record))
            .map(crate::session::records::message)
            .transpose()
            .map_err(ApiError::from)
    }

    /// Optional text projection over the same incarnation-aware record history.
    /// An empty page may have a continuation when it visited non-text records.
    pub async fn messages(
        &self,
        group: GroupId,
        before: Option<crate::RecordCursor>,
        limit: u32,
        clock: ClockReading,
    ) -> Result<MessagePage, ApiError> {
        if limit == 0 || limit > 100 {
            return Err(ApiError::InvalidInput);
        }
        let limit = usize::try_from(limit).map_err(|_| ApiError::InvalidInput)?;
        match self
            .call_api(Command::Messages {
                group,
                before,
                limit,
                clock,
            })
            .await?
        {
            Reply::Messages(value) => Ok(value),
            _ => Err(ApiError::Corrupt),
        }
    }
}

#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
impl Client {
    /// Exact visible invitation, independent of the currently loaded page.
    /// Applies the same expiry, dismissal and local-delete projection as pages.
    /// This pure read never performs maintenance or initializes storage.
    pub async fn invitation(
        &self,
        id: crate::InvitationId,
        now: u64,
    ) -> Result<Option<crate::enrollment::Invitation>, ApiError> {
        match self.call_api(Command::Invitation { id, now }).await? {
            Reply::Invitation(value) => Ok(value),
            _ => Err(ApiError::Corrupt),
        }
    }

    /// One bounded live invitation page. This pure query never advances
    /// enrollment or maintenance. Continue through empty pages while next is set.
    pub async fn invitations(
        &self,
        now: u64,
        after: Option<crate::InvitationId>,
        limit: crate::PageLimits,
    ) -> Result<crate::InvitationPage, ApiError> {
        limit.validate()?;
        match self
            .call_api(Command::Invitations { now, after, limit })
            .await?
        {
            Reply::Invitations(value) => Ok(value),
            _ => Err(ApiError::Corrupt),
        }
    }

    /// Native-owned durable submission with scope captured on the owner lane.
    /// Uses a schema explicitly selected at startup and an authorized executor.
    pub async fn submit(
        &self,
        id: crate::RequestId,
        intent: crate::Intent,
        context: ExecutionContext,
    ) -> Result<crate::retained::Operation, ApiError> {
        self.admit_submit(id, intent, context)?.await
    }
    /// Identity-wide retained status. None never proves no effect occurred.
    pub async fn operation(
        &self,
        id: crate::RequestId,
    ) -> Result<Option<crate::retained::Operation>, ApiError> {
        match self.call_api(Command::RetainedOperation(id)).await? {
            Reply::Operation(operation) => Ok(operation),
            _ => Err(ApiError::Corrupt),
        }
    }
    /// Identity-wide bounded listing in exact RequestId byte order.
    pub async fn operations(
        &self,
        after: Option<crate::RequestId>,
        limit: crate::PageLimits,
    ) -> Result<Vec<crate::retained::Operation>, ApiError> {
        limit.validate()?;
        match self
            .call_api(Command::RetainedOperations { after, limit })
            .await?
        {
            Reply::Operations(operations) => Ok(operations),
            _ => Err(ApiError::Corrupt),
        }
    }
    /// Forget a terminal local result and atomically fence replay. Unresolved
    /// operations cannot be dismissed; this does not erase domain receipts.
    pub async fn dismiss_operation(&self, id: crate::RequestId) -> Result<(), ApiError> {
        match self.call_api(Command::DismissRetained(id)).await? {
            Reply::Done => Ok(()),
            _ => Err(ApiError::Corrupt),
        }
    }

    /// Register with the owner before reading any snapshot. Independent clients
    /// sharing that owner observe the same commits; the reply waiter never
    /// publishes changes. At most MAX_OBSERVATIONS subscriptions may be live.
    pub async fn subscribe(&self) -> Result<Arc<crate::GroupObservation>, ApiError> {
        match self.call_api(Command::Subscribe).await? {
            Reply::Subscription(subscription) => Ok(subscription),
            _ => Err(ApiError::Corrupt),
        }
    }

    /// Read the exact current context without maintenance or mutation admission.
    pub async fn group_scope(&self, group: GroupId) -> Result<crate::OriginalScope, ApiError> {
        match self.call_api(Command::GroupScope(group)).await? {
            Reply::GroupScope(scope) => Ok(scope),
            _ => Err(ApiError::Corrupt),
        }
    }
    /// Bounded catalog summaries without loading each conversation's roster.
    /// Each page is a live snapshot; additions before its cursor appear on refresh.
    pub async fn conversations(
        &self,
        after: Option<GroupId>,
        limit: crate::PageLimits,
    ) -> Result<crate::ConversationPage, ApiError> {
        limit.validate()?;
        match self
            .call_api(Command::Conversations { after, limit })
            .await?
        {
            Reply::Conversations(value) => Ok(value),
            _ => Err(ApiError::Corrupt),
        }
    }

    /// Exact selected-group detail, independent of any loaded summary page.
    pub async fn conversation(
        &self,
        group: GroupId,
        limit: crate::PageLimits,
    ) -> Result<Conversation, ApiError> {
        limit.validate()?;
        match self
            .call_api(Command::Conversation { group, limit })
            .await?
        {
            Reply::Conversation(value) => Ok(value),
            _ => Err(ApiError::Corrupt),
        }
    }

    /// Current member bindings in exclusive Endpoint order. If the returned
    /// control changes between pages, restart the roster traversal.
    pub async fn members(
        &self,
        group: GroupId,
        after: Option<Endpoint>,
        limit: crate::PageLimits,
    ) -> Result<crate::MemberPage, ApiError> {
        limit.validate()?;
        match self
            .call_api(Command::Members {
                group,
                after,
                limit,
            })
            .await?
        {
            Reply::Members(value) => Ok(value),
            _ => Err(ApiError::Corrupt),
        }
    }
    pub async fn execute(
        &self,
        request: Request,
        context: ExecutionContext,
    ) -> Result<Outcome, ApiError> {
        self.admit_execute(request, context)?.await
    }
    /// Newest retained membership first, then descending insertion positions.
    /// A bounded page may be empty with a continuation; keep paging until None.
    pub async fn records(
        &self,
        group: GroupId,
        before: Option<crate::RecordCursor>,
        limit: u32,
        clock: ClockReading,
    ) -> Result<RecordPage, ApiError> {
        if limit == 0 || limit > 100 {
            return Err(ApiError::InvalidInput);
        }
        let limit = usize::try_from(limit).map_err(|_| ApiError::InvalidInput)?;
        match self
            .call_api(Command::Records {
                selection: crate::RecordSelection::All,
                group,
                before,
                limit,
                clock,
            })
            .await?
        {
            Reply::Records(page) => Ok(page),
            _ => Err(ApiError::Corrupt),
        }
    }
    /// Look up a record only in its explicit retained membership incarnation.
    pub async fn record(
        &self,
        group: GroupId,
        incarnation: prns_groups_store::StoreIncarnation,
        record: RecordId,
        clock: ClockReading,
    ) -> Result<Option<StoredRecord>, ApiError> {
        match self
            .call_api(Command::Record {
                group,
                incarnation,
                record,
                clock,
            })
            .await?
        {
            Reply::Record(record) => Ok(record.map(|record| *record)),
            _ => Err(ApiError::Corrupt),
        }
    }
    /// Read original target receipt/removal facts in endpoint order. This is a
    /// read-only view of retained evidence, available without a network runner.
    /// Each call decodes one profile-bounded record; output slots obey limits.
    pub async fn delivery_targets(
        &self,
        group: GroupId,
        incarnation: StoreIncarnation,
        record: RecordId,
        after_endpoint: Option<Endpoint>,
        limits: crate::PageLimits,
    ) -> Result<Option<crate::DeliveryTargetPage>, ApiError> {
        crate::DeliveryTargetPage::capacity(limits)?;
        match self
            .call_api(Command::DeliveryTargets {
                group,
                incarnation,
                record,
                after: after_endpoint,
                limit: limits,
            })
            .await?
        {
            Reply::DeliveryTargets(page) => Ok(page),
            _ => Err(ApiError::Corrupt),
        }
    }

    /// Device receipt evidence is reconstructed from committed state.
    pub async fn delivery_progress(
        &self,
        group: GroupId,
        incarnation: prns_groups_store::StoreIncarnation,
        record: RecordId,
        clock: ClockReading,
    ) -> Result<Option<RecordDeliveryProgress>, ApiError> {
        match self
            .call_api(Command::Progress {
                group,
                incarnation,
                record,
                clock,
            })
            .await?
        {
            Reply::Progress(progress) => Ok(progress),
            _ => Err(ApiError::Corrupt),
        }
    }
    /// Read durable changes after a saved cursor. Hints/notifications are not
    /// authoritative, and `history_pruned` requires rebuilding retained state.
    /// A token selects its retained incarnation; None starts the current feed.
    pub async fn changes(
        &self,
        group: GroupId,
        after: Option<ChangeToken>,
        limit: u32,
    ) -> Result<RecordChanges, ApiError> {
        if limit == 0 || limit > 100 {
            return Err(ApiError::InvalidInput);
        }
        let limit = usize::try_from(limit).map_err(|_| ApiError::InvalidInput)?;
        match self
            .call_api(Command::Changes {
                group,
                after,
                limit,
            })
            .await?
        {
            Reply::Changes(changes) => Ok(changes),
            _ => Err(ApiError::Corrupt),
        }
    }
    pub async fn identity(&self) -> Result<prns_host::IdentityHash, ApiError> {
        match self.call_api(Command::Identity).await? {
            Reply::Identity(value) => Ok(value),
            _ => Err(ApiError::Corrupt),
        }
    }
}

// Carrier storage operations remain available for offline recovery/testing.
#[cfg_attr(not(feature = "network"), allow(dead_code))]
pub(crate) enum Command {
    Subscribe,
    ProfileSupport(crate::ProfileSupport),
    GroupScope(GroupId),
    Identity,
    SubmitRetained {
        id: crate::RequestId,
        intent: crate::Intent,
        context: ExecutionContext,
    },
    RetainedOperation(crate::RequestId),
    RetainedOperations {
        after: Option<crate::RequestId>,
        limit: crate::PageLimits,
    },
    DismissRetained(crate::RequestId),
    Execute {
        request: Request,
        context: ExecutionContext,
    },
    Invitation {
        id: crate::InvitationId,
        now: u64,
    },
    Invitations {
        now: u64,
        after: Option<crate::InvitationId>,
        limit: crate::PageLimits,
    },
    EnrollmentWork {
        now: u64,
        after: Option<[u8; 32]>,
        limit: crate::PageLimits,
    },
    EnrollmentHandle {
        requester: prns_host::IdentityHash,
        frame: Vec<u8>,
        now: u64,
    },
    EnrollmentComplete {
        id: [u8; 32],
        response: Vec<u8>,
        now: u64,
    },
    Config,
    Conversations {
        after: Option<GroupId>,
        limit: crate::PageLimits,
    },
    Conversation {
        group: GroupId,
        limit: crate::PageLimits,
    },
    Members {
        group: GroupId,
        after: Option<Endpoint>,
        limit: crate::PageLimits,
    },
    #[cfg(feature = "text")]
    Messages {
        group: GroupId,
        before: Option<crate::RecordCursor>,
        limit: usize,
        clock: ClockReading,
    },

    Records {
        selection: crate::RecordSelection,
        group: GroupId,
        before: Option<crate::RecordCursor>,
        limit: usize,
        clock: ClockReading,
    },
    Record {
        group: GroupId,
        incarnation: prns_groups_store::StoreIncarnation,
        record: RecordId,
        clock: ClockReading,
    },
    Progress {
        group: GroupId,
        incarnation: prns_groups_store::StoreIncarnation,
        record: RecordId,
        clock: ClockReading,
    },
    DeliveryTargets {
        group: GroupId,
        incarnation: StoreIncarnation,
        record: RecordId,
        after: Option<Endpoint>,
        limit: crate::PageLimits,
    },
    Changes {
        group: GroupId,
        after: Option<ChangeToken>,
        limit: usize,
    },
    LoadCursor(Endpoint),
    SaveCursor(Endpoint, Option<SyncCursor>),
    Maintain(ClockReading),
    RepairReceipts,
    Snapshot,
    PendingDelivery {
        peer: Endpoint,
        after: InventoryCursor,
        clock: ClockReading,
    },
    Receipt(RecordId),
    Inventory {
        snapshot: InventorySnapshot,
        after: InventoryCursor,
        kind: InventoryKind,
        recipient: Endpoint,
        scan_limit: usize,
    },
    Envelope(EnvelopeId),
    Known {
        info: EnvelopeInfo,
        incarnation: StoreIncarnation,
    },
    Receive {
        wire: Vec<u8>,
        options: SyncOptions,
    },
}
// Carrier storage operations remain available for offline recovery/testing.
#[cfg_attr(not(feature = "network"), allow(dead_code))]
pub(crate) enum Reply {
    Subscription(Arc<crate::GroupObservation>),
    Operation(Option<crate::retained::Operation>),
    Operations(Vec<crate::retained::Operation>),
    GroupScope(crate::OriginalScope),
    Identity(prns_host::IdentityHash),
    Outcome(Outcome),
    Invitation(Option<crate::enrollment::Invitation>),
    Invitations(crate::InvitationPage),
    EnrollmentWork(crate::enrollment::WorkPage),
    EnrollmentFrame(Vec<u8>),
    EnrollmentCompleted(crate::enrollment::CompletionActivity),
    Config(NetworkConfig),
    Conversations(crate::ConversationPage),
    Conversation(Conversation),
    Members(crate::MemberPage),
    #[cfg(feature = "text")]
    Messages(MessagePage),
    Records(RecordPage),
    Record(Option<Box<StoredRecord>>),
    Progress(Option<RecordDeliveryProgress>),
    DeliveryTargets(Option<crate::DeliveryTargetPage>),
    Changes(RecordChanges),
    Cursor(Option<SyncCursor>),
    Done,
    Snapshot(Option<InventorySnapshot>),
    PendingDelivery(Option<EnvelopeInfo>),
    Inventory(InventoryPage),
    Envelope(Option<Vec<u8>>),
    Known(bool),
    Incoming(Incoming),
}

#[cfg(all(test, feature = "text", feature = "network"))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::enrollment::CompletionActivity;
    use futures_util::FutureExt;

    struct DeferredOwner(tokio::sync::mpsc::UnboundedSender<Job>, InputBudget);
    impl Submit for DeferredOwner {
        fn input_budget(&self) -> &InputBudget {
            &self.1
        }
        fn submit(&self, job: Job) -> Result<(), Error> {
            self.0.send(job).map_err(|_| Error::Unavailable)
        }
    }

    #[tokio::test]
    async fn admitted_send_wakes_after_commit_even_if_the_caller_drops_the_waiter() {
        let now = prns_groups_service::Timestamp(1_792_000_000);
        let clock =
            ClockReading::Trusted(prns_groups_service::ClockInterval::new(now, now).unwrap());
        let identities = [
            prns_host::IdentityHash::new([1; 16]),
            prns_host::IdentityHash::new([2; 16]),
        ];
        let fixtures = crate::fixture::generate("Commit wake", &identities, now).unwrap();
        // Both public owner paths must signal: standalone and the app manager.
        for managed in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let db = redb::Database::create(directory.path().join("wake.redb")).unwrap();
            let config =
                crate::fixture::load_and_install(&db, &fixtures[0], identities[0]).unwrap();
            let (sender, mut jobs) = tokio::sync::mpsc::unbounded_channel();
            let client = Client::new(Arc::new(DeferredOwner(sender, InputBudget::default())));
            let prepared = crate::test_support::text(
                config.group,
                crate::OriginalScope::Control {
                    control: config.control,
                    incarnation: config.incarnation,
                },
                crate::RequestToken::new(now.0, [7; 20])
                    .unwrap()
                    .into_bytes(),
                "A cancelled waiter must not strand this send".into(),
                clock,
            );
            let waiter = client.admit_execute(prepared.0, prepared.1).unwrap();
            drop(waiter);
            assert!(client.wake.notified().now_or_never().is_none());
            let job = jobs.recv().await.unwrap();
            if managed {
                let mut owner = crate::GroupManager::open_fixed(&db, config).unwrap();
                owner.execute(job);
            } else {
                let mut session = StoreSession::open(&db, config).unwrap();
                job.execute(&mut session);
            }
            assert!(client.wake.notified().now_or_never().is_some());
            assert!(client.wake.notified().now_or_never().is_none());
        }
    }

    struct CompletionOwner(CompletionActivity, InputBudget);
    impl Submit for CompletionOwner {
        fn input_budget(&self) -> &InputBudget {
            &self.1
        }
        fn submit(&self, job: Job) -> Result<(), Error> {
            if !matches!(job.command, Command::EnrollmentComplete { .. }) {
                job.fail(Error::InvalidInput);
                return Ok(());
            }
            let _ = job.reply.send(Ok(Reply::EnrollmentCompleted(self.0)));
            Ok(())
        }
    }

    #[tokio::test]
    async fn only_committed_enrollment_progress_wakes_content_scheduling() {
        for activity in [CompletionActivity::Idle, CompletionActivity::Progress] {
            let client = Client::new(Arc::new(CompletionOwner(activity, InputBudget::default())));
            // The client consumes only a typed storage result, not a network
            // reply tag; correlation/validation remains on the enrollment lane.
            assert_eq!(
                client
                    .enrollment_complete([1; 32], vec![0], 1_800_000_000)
                    .await
                    .unwrap(),
                activity
            );
            assert_eq!(
                client.wake.notified().now_or_never().is_some(),
                activity == CompletionActivity::Progress
            );
        }
    }
}
