//! Optional Tokio direct delivery above the synchronous mailbox owner.
use super::*;
use crate::direct::{
    run_single_attempt, DirectAnnounceFailure, DirectNetwork, DirectSendFailure,
    DirectServiceStopError, Job, LaneHealth, LocalLxmfIdentity, LxmfCallbacks, LxmfPeer, PeerCache,
    PeerResolver, ResolvePeerFailure, DIRECT_JOB_CAPACITY, STOP_JOIN_TIMEOUT,
};
use prns_lxmf_wire::{compose_basic_direct_lxmf, BasicLxmfComposeError, CarrierIngress};
use std::collections::BTreeMap;
use std::mem;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot, watch, Mutex, Notify};
const SETTLEMENT_RETRY_DELAY: Duration = Duration::from_millis(25);
mod replay;
pub use replay::ReplayLimits;
use tokio::task::JoinHandle;

/// Wall-clock source retained outside the durable state machine for deterministic tests.
pub trait MailboxClock: Send + Sync + 'static {
    fn now_unix_millis(&self) -> u64;
}

/// System wall clock used by production composition.
#[derive(Debug, Default)]
pub struct SystemMailboxClock;

impl MailboxClock for SystemMailboxClock {
    fn now_unix_millis(&self) -> u64 {
        let elapsed = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(elapsed) => elapsed,
            Err(_) => Duration::ZERO,
        };
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    }
}

/// Storage-specific health that complements the existing callback-lane health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailboxProjectionHealth {
    Ready,
    Capacity,
    Busy,
    Unavailable(String),
    ResetRequired(String),
}

/// Authoritative durable snapshot plus process-local peer and active-attempt projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableLxmfSnapshot {
    pub peers: Vec<LxmfPeer>,
    pub messages: Vec<DurableLxmfMessage>,
    pub health: crate::LxmfHealth,
    pub mailbox_health: MailboxProjectionHealth,
    pub durable_revision: u64,
    pub projection_revision: u64,
}

/// Durable submission returns after the queued transaction, before network settlement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableSendDirectTextOutcome {
    MailboxFull,
    Accepted {
        local_record_id: u64,
    },
    NeedsResource {
        wire_bytes: usize,
    },
    UnsupportedRemoteStampRequirement {
        required_stamp_cost: u64,
    },
    PeerIdentityUnavailable,
    PeerIdentityConflict {
        expected: [u8; 16],
        observed: [u8; 16],
    },
    RecipientResolutionTimedOut,
    LocalNodeStopped,
    Busy,
    InvalidMessage,
    Unavailable {
        detail: String,
    },
    ResetRequired {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryLxmfMessageOutcome {
    Accepted { local_record_id: u64 },
    NotFound,
    NotFailed { current: DurableLxmfDeliveryState },
    LocalNodeStopped,
    Busy,
    Unavailable { detail: String },
    ResetRequired { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelLxmfMessageOutcome {
    Cancelled { local_record_id: u64 },
    NotFound,
    AlreadyDelivered,
    AlreadyCancelled,
    NotCancellable { current: DurableLxmfDeliveryState },
    LocalNodeStopped,
    Busy,
    Unavailable { detail: String },
    ResetRequired { reason: String },
}

/// Failure to start a durable direct service generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableDirectServiceStartError {
    NoTokioRuntime,
    InvalidLimits,
    Mailbox(MailboxFailure),
    UnexpectedMailboxReply,
}

/// Callback lane prepared before the owning aggregate constructs its PRNS node.
pub struct PendingDurableDirectLxmfService {
    limits: ReplayLimits,
    identity: LocalLxmfIdentity,
    callbacks: LxmfCallbacks,
    receiver: mpsc::Receiver<Job>,
    refresh: watch::Sender<u64>,
    shutdown: watch::Sender<bool>,
    shutdown_receiver: watch::Receiver<bool>,
}

/// One durable direct LXMF generation.
#[derive(Clone)]
pub struct DurableDirectLxmfService {
    shared: Arc<DurableShared>,
    lifecycle: Arc<DurableLifecycle>,
    callbacks: LxmfCallbacks,
}

struct DurableShared {
    local_destination: [u8; 16],
    identity: Arc<LocalLxmfIdentity>,
    network: Arc<dyn DirectNetwork>,
    mailbox: Arc<dyn MailboxSubmitter>,
    clock: Arc<dyn MailboxClock>,
    peers: Mutex<PeerCache>,
    resolver: PeerResolver,
    health: Arc<LaneHealth>,
    mailbox_health: StdMutex<MailboxHealthTracker>,
    late_proof_count: AtomicU64,
    refresh: watch::Sender<u64>,
}

#[derive(Default)]
struct MailboxHealthTracker {
    read_failure: Option<MailboxProjectionHealth>,
    write_failure: Option<MailboxProjectionHealth>,
}

impl MailboxHealthTracker {
    fn projection(&self) -> MailboxProjectionHealth {
        for failure in [&self.write_failure, &self.read_failure]
            .into_iter()
            .flatten()
        {
            if matches!(failure, MailboxProjectionHealth::ResetRequired(_)) {
                return failure.clone();
            }
        }
        self.write_failure
            .as_ref()
            .or(self.read_failure.as_ref())
            .cloned()
            .unwrap_or(MailboxProjectionHealth::Ready)
    }
}

struct DurableLifecycle {
    shutdown: watch::Sender<bool>,
    activated: AtomicBool,
    stopped: Arc<AtomicBool>,
    stop_complete: AtomicBool,
    stop_transition: Mutex<()>,
    worker_failed: AtomicBool,
    transition: Arc<Mutex<()>>,
    worker: StdMutex<Option<JoinHandle<()>>>,
    attempts: StdMutex<BTreeMap<AttemptKey, JoinHandle<()>>>,
    limits: ReplayLimits,
    replay_wake: Arc<Notify>,
    replayer: StdMutex<Option<JoinHandle<()>>>,
}

impl DurableLifecycle {
    fn worker(&self) -> StdMutexGuard<'_, Option<JoinHandle<()>>> {
        self.worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn attempts(&self) -> StdMutexGuard<'_, BTreeMap<AttemptKey, JoinHandle<()>>> {
        self.attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn replayer(&self) -> StdMutexGuard<'_, Option<JoinHandle<()>>> {
        self.replayer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for DurableLifecycle {
    fn drop(&mut self) {
        if let Some(worker) = self
            .worker
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            worker.abort();
        }
        if let Some(replayer) = self
            .replayer
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            replayer.abort();
        }
        let attempts = mem::take(
            self.attempts
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for (_key, attempt) in attempts {
            attempt.abort();
        }
    }
}

impl DurableDirectLxmfService {
    /// Prepare callbacks before the aggregate constructs its one node.
    #[must_use]
    pub fn prepare(
        identity: LocalLxmfIdentity,
    ) -> (PendingDurableDirectLxmfService, LxmfCallbacks) {
        let local_destination = identity.destination();
        let (jobs, receiver) = mpsc::channel(DIRECT_JOB_CAPACITY);
        let (refresh, _refresh_receiver) = watch::channel(1_u64);
        let health = Arc::new(LaneHealth::new(refresh.clone()));
        let callbacks = LxmfCallbacks {
            local_destination,
            jobs,
            health,
        };
        let (shutdown, shutdown_receiver) = watch::channel(false);
        (
            PendingDurableDirectLxmfService {
                limits: ReplayLimits::default(),
                identity,
                callbacks: callbacks.clone(),
                receiver,
                refresh,
                shutdown,
                shutdown_receiver,
            },
            callbacks,
        )
    }

    /// Select bounded replay work independently of the persisted backlog quota.
    pub fn prepare_with_limits(
        identity: LocalLxmfIdentity,
        limits: ReplayLimits,
    ) -> Result<(PendingDurableDirectLxmfService, LxmfCallbacks), DurableDirectServiceStartError>
    {
        limits.validate()?;
        let (mut pending, callbacks) = Self::prepare(identity);
        pending.limits = limits;
        Ok((pending, callbacks))
    }

    #[must_use]
    pub fn callbacks(&self) -> LxmfCallbacks {
        self.callbacks.clone()
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.shared.refresh.subscribe()
    }

    /// Saturating process-local count of delivery proofs rejected as stale.
    ///
    /// The diagnostic resets with the service generation and is never written to
    /// the durable mailbox or treated as delivery state.
    #[must_use]
    pub fn late_proof_count(&self) -> u64 {
        self.shared.late_proof_count.load(Ordering::Acquire)
    }

    #[must_use]
    pub const fn local_destination(&self) -> [u8; 16] {
        self.callbacks.local_destination
    }

    pub async fn announce(&self) -> Result<(), DirectAnnounceFailure> {
        self.shared
            .network
            .announce(self.shared.local_destination)
            .await
    }

    /// Compose/sign once, commit `Queued`, wake bounded replay, then return `Accepted`.
    pub async fn send_direct_text(
        &self,
        destination: [u8; 16],
        timestamp_unix_ms: u64,
        title: &[u8],
        content: &[u8],
    ) -> DurableSendDirectTextOutcome {
        self.send_direct_text_to_identity(destination, None, timestamp_unix_ms, title, content)
            .await
    }

    /// Resolve missing authenticated metadata before admission, then check a saved
    /// identity association before composing or inserting any durable message.
    pub async fn send_direct_text_to_identity(
        &self,
        destination: [u8; 16],
        expected_identity: Option<[u8; 16]>,
        timestamp_unix_ms: u64,
        title: &[u8],
        content: &[u8],
    ) -> DurableSendDirectTextOutcome {
        if self.lifecycle.stopped.load(Ordering::Acquire) {
            return DurableSendDirectTextOutcome::LocalNodeStopped;
        }
        if destination == self.local_destination() {
            return DurableSendDirectTextOutcome::PeerIdentityUnavailable;
        }
        let peer = match self
            .shared
            .resolver
            .resolve(
                destination,
                &self.shared.peers,
                self.shared.network.as_ref(),
                self.subscribe(),
                self.lifecycle.shutdown.subscribe(),
            )
            .await
        {
            Ok(peer) => peer,
            Err(ResolvePeerFailure::TimedOut) => {
                return DurableSendDirectTextOutcome::RecipientResolutionTimedOut
            }
            Err(ResolvePeerFailure::Stopped) => {
                return DurableSendDirectTextOutcome::LocalNodeStopped
            }
            Err(ResolvePeerFailure::Unavailable) => {
                return DurableSendDirectTextOutcome::PeerIdentityUnavailable
            }
            Err(ResolvePeerFailure::Busy) => return DurableSendDirectTextOutcome::Busy,
        };
        if let Some(rejected) = validate_recipient(&peer, expected_identity) {
            return rejected;
        }
        let message = match compose_new_outbound(
            self.shared.local_destination,
            destination,
            timestamp_unix_ms,
            title,
            content,
            self.shared.identity.as_ref(),
        ) {
            Ok(message) => message,
            Err(BasicLxmfComposeError::NeedsResource { wire_bytes }) => {
                return DurableSendDirectTextOutcome::NeedsResource { wire_bytes };
            }
            Err(_) => return DurableSendDirectTextOutcome::InvalidMessage,
        };

        let _transition = self.lifecycle.transition.lock().await;
        if self.lifecycle.stopped.load(Ordering::Acquire) {
            return DurableSendDirectTextOutcome::LocalNodeStopped;
        }
        // Another admitted database operation may have held the transition gate
        // while a newer accepted announce changed the recipient's stamp policy.
        let current = self.shared.peers.lock().await.recipient(&destination);
        let Some(current) = current else {
            // Expiry or capacity eviction while waiting cannot resurrect the
            // earlier policy. A later explicit submission may resolve it again.
            return DurableSendDirectTextOutcome::PeerIdentityUnavailable;
        };
        if let Some(rejected) = validate_recipient(&current, expected_identity) {
            return rejected;
        }
        let reply = match self
            .shared
            .mailbox
            .submit(MailboxRequest::InsertOutbound(message))
            .await
        {
            Ok(reply) => reply,
            Err(failure) => {
                self.shared.set_mailbox_write_failure(&failure);
                return send_failure_outcome(failure);
            }
        };
        let MailboxReply::OutboundInserted { message, .. } = reply else {
            self.shared.set_unexpected_mailbox_write_failure(
                "the mailbox owner returned an unexpected insert reply",
            );
            return DurableSendDirectTextOutcome::Unavailable {
                detail: "the mailbox owner returned an unexpected insert reply".to_owned(),
            };
        };
        self.shared.set_mailbox_write_ready();
        self.shared.notify();
        let local_record_id = message.local_record_id;
        schedule_attempt(&self.lifecycle);
        DurableSendDirectTextOutcome::Accepted { local_record_id }
    }

    /// Clear visible discovery only. Saved contacts, routes, recipient metadata,
    /// and accepted durable work remain unchanged.
    pub async fn clear_discovered_peers(&self) {
        self.shared.peers.lock().await.clear_discovered();
        self.shared.notify();
    }

    /// Conditionally requeue the exact stored wire with a new attempt generation.
    pub async fn retry_lxmf_message(&self, local_record_id: u64) -> RetryLxmfMessageOutcome {
        let _transition = self.lifecycle.transition.lock().await;
        if self.lifecycle.stopped.load(Ordering::Acquire) {
            return RetryLxmfMessageOutcome::LocalNodeStopped;
        }
        let reply = match self
            .shared
            .mailbox
            .submit(MailboxRequest::Retry { local_record_id })
            .await
        {
            Ok(reply) => reply,
            Err(failure) => {
                self.shared.set_mailbox_write_failure(&failure);
                return retry_failure_outcome(failure);
            }
        };
        let MailboxReply::Retry(transition) = reply else {
            self.shared.set_unexpected_mailbox_write_failure(
                "the mailbox owner returned an unexpected retry reply",
            );
            return RetryLxmfMessageOutcome::Unavailable {
                detail: "the mailbox owner returned an unexpected retry reply".to_owned(),
            };
        };
        match transition {
            RetryTransition::Accepted { .. } => {
                self.shared.set_mailbox_write_ready();
                self.shared.notify();
                schedule_attempt(&self.lifecycle);
                RetryLxmfMessageOutcome::Accepted { local_record_id }
            }
            RetryTransition::NotFound => {
                self.shared.set_mailbox_read_ready();
                RetryLxmfMessageOutcome::NotFound
            }
            RetryTransition::NotFailed { current } => {
                self.shared.set_mailbox_read_ready();
                RetryLxmfMessageOutcome::NotFailed {
                    current: self.project_current_state(local_record_id, current),
                }
            }
        }
    }

    /// Conditionally commit cancellation before aborting the matching active generation.
    pub async fn cancel_lxmf_message(
        &self,
        local_record_id: u64,
        cancelled_at_millis: u64,
    ) -> CancelLxmfMessageOutcome {
        let _transition = self.lifecycle.transition.lock().await;
        if self.lifecycle.stopped.load(Ordering::Acquire) {
            return CancelLxmfMessageOutcome::LocalNodeStopped;
        }
        let reply = match self
            .shared
            .mailbox
            .submit(MailboxRequest::Cancel {
                local_record_id,
                cancelled_at_millis,
            })
            .await
        {
            Ok(reply) => reply,
            Err(failure) => {
                self.shared.set_mailbox_write_failure(&failure);
                return cancel_failure_outcome(failure);
            }
        };
        let MailboxReply::Cancel(transition) = reply else {
            self.shared.set_unexpected_mailbox_write_failure(
                "the mailbox owner returned an unexpected cancel reply",
            );
            return CancelLxmfMessageOutcome::Unavailable {
                detail: "the mailbox owner returned an unexpected cancel reply".to_owned(),
            };
        };
        match transition {
            CancelTransition::Cancelled { message, .. } => {
                self.shared.set_mailbox_write_ready();
                self.shared.notify();
                if let Some(generation) = message.generation() {
                    let key = AttemptKey {
                        local_record_id,
                        generation,
                    };
                    let attempt = self.lifecycle.attempts().remove(&key);
                    if let Some(attempt) = attempt {
                        attempt.abort();
                        self.shared.notify();
                    }
                    self.lifecycle.replay_wake.notify_one();
                }
                CancelLxmfMessageOutcome::Cancelled { local_record_id }
            }
            CancelTransition::NotFound => {
                self.shared.set_mailbox_read_ready();
                CancelLxmfMessageOutcome::NotFound
            }
            CancelTransition::AlreadyDelivered => {
                self.shared.set_mailbox_read_ready();
                CancelLxmfMessageOutcome::AlreadyDelivered
            }
            CancelTransition::AlreadyCancelled => {
                self.shared.set_mailbox_read_ready();
                CancelLxmfMessageOutcome::AlreadyCancelled
            }
            CancelTransition::NotCancellable { current } => {
                self.shared.set_mailbox_read_ready();
                CancelLxmfMessageOutcome::NotCancellable { current }
            }
        }
    }

    fn project_current_state(
        &self,
        local_record_id: u64,
        current: DurableLxmfDeliveryState,
    ) -> DurableLxmfDeliveryState {
        let DurableLxmfDeliveryState::Queued { failed_attempts } = current else {
            return current;
        };
        if self
            .lifecycle
            .attempts()
            .keys()
            .any(|key| key.local_record_id == local_record_id)
        {
            DurableLxmfDeliveryState::Sending { failed_attempts }
        } else {
            current
        }
    }

    /// Enable queued network attempts after the owning node restores persistence.
    ///
    /// This is idempotent. Rows accepted while paused remain durably `Queued` and
    /// join the same activation batch; stopping before activation never touches
    /// their durable state.
    pub async fn activate_queued_attempts(&self) -> bool {
        let _transition = self.lifecycle.transition.lock().await;
        if self.lifecycle.stopped.load(Ordering::Acquire) {
            return false;
        }
        if self.lifecycle.activated.swap(true, Ordering::AcqRel) {
            return true;
        }
        self.lifecycle.replay_wake.notify_one();
        true
    }

    /// Query committed records and overlay only matching active generations as `Sending`.
    pub async fn snapshot(
        &self,
        request: MailboxListRequest,
    ) -> Result<DurableLxmfSnapshot, MailboxFailure> {
        self.query_snapshot(MailboxRequest::List(request)).await
    }

    /// Query each peer's latest record without a mailbox-wide recent-row cutoff.
    /// Uses the same health and active-attempt projection as [`Self::snapshot`].
    pub async fn conversation_snapshot(
        &self,
        request: MailboxConversationListRequest,
    ) -> Result<DurableLxmfSnapshot, MailboxFailure> {
        self.query_snapshot(MailboxRequest::ListConversations(request))
            .await
    }

    async fn query_snapshot(
        &self,
        request: MailboxRequest,
    ) -> Result<DurableLxmfSnapshot, MailboxFailure> {
        let _transition = self.lifecycle.transition.lock().await;
        let conversations = matches!(&request, MailboxRequest::ListConversations(_));
        let reply = self.shared.mailbox.submit(request).await;
        let reply = match reply {
            Ok(reply) => {
                self.shared.set_mailbox_read_ready();
                reply
            }
            Err(failure) => {
                self.shared.set_mailbox_read_failure(&failure);
                return Err(failure);
            }
        };
        let (mut messages, revision) = match reply {
            MailboxReply::Listed { messages, revision } if !conversations => (messages, revision),
            MailboxReply::ConversationsListed { messages, revision } if conversations => {
                (messages, revision)
            }
            _ => {
                self.shared.set_unexpected_mailbox_read_failure(
                    "the mailbox owner returned an unexpected list reply",
                );
                return Err(MailboxFailure::unavailable(
                    "the mailbox owner returned an unexpected list reply",
                ));
            }
        };
        let active: Vec<AttemptKey> = self.lifecycle.attempts().keys().copied().collect();
        for message in &mut messages {
            if let Some(key) = active
                .iter()
                .find(|key| key.local_record_id == message.local_record_id)
            {
                let _projected = message.project_sending(key.generation);
            }
        }
        Ok(DurableLxmfSnapshot {
            peers: self.shared.peers.lock().await.discovered(),
            messages,
            health: self.shared.health.snapshot(),
            mailbox_health: self.shared.mailbox_health(),
            durable_revision: revision,
            projection_revision: *self.shared.refresh.borrow(),
        })
    }

    /// Fence fresh callbacks and attempts, then drain already accepted callbacks.
    ///
    /// The embedding must retire protocol ingress before calling this method.
    /// A bounded wait or cancellation retains every join handle for a retry;
    /// durable queued outbound rows remain available to the next generation.
    pub async fn stop(&self) -> Result<(), DirectServiceStopError> {
        if self.lifecycle.stop_complete.load(Ordering::Acquire) {
            return Ok(());
        }
        self.lifecycle.stopped.store(true, Ordering::Release);
        self.lifecycle.activated.store(false, Ordering::Release);
        self.shared.health.stop();
        let _changed = self.lifecycle.shutdown.send(true);
        let deadline = tokio::time::Instant::now() + STOP_JOIN_TIMEOUT;
        let _stop = tokio::time::timeout_at(deadline, self.lifecycle.stop_transition.lock())
            .await
            .map_err(|_| DirectServiceStopError::JoinTimedOut)?;
        if self.lifecycle.stop_complete.load(Ordering::Acquire) {
            return Ok(());
        }
        {
            // Outbound attempts hold this gate through any admitted commit.
            // Release it before joining the worker, whose accepted callbacks
            // still need the same gate to finish durable inbox insertion.
            let _transition = tokio::time::timeout_at(deadline, self.lifecycle.transition.lock())
                .await
                .map_err(|_| DirectServiceStopError::JoinTimedOut)?;
            let attempts = self.lifecycle.attempts();
            for attempt in attempts.values() {
                attempt.abort();
            }
            self.lifecycle.replay_wake.notify_one();
        }

        // Poll in place: taking a JoinHandle across an await would detach it if
        // the caller cancelled stop, allowing a later call to claim completion.
        tokio::time::timeout_at(
            deadline,
            std::future::poll_fn(|cx| {
                let mut worker = self.lifecycle.worker();
                let Some(handle) = worker.as_mut() else {
                    return std::task::Poll::Ready(());
                };
                match Pin::new(handle).poll(cx) {
                    std::task::Poll::Ready(result) => {
                        if result.is_err() {
                            self.lifecycle.worker_failed.store(true, Ordering::Release);
                        }
                        worker.take();
                        std::task::Poll::Ready(())
                    }
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }),
        )
        .await
        .map_err(|_| DirectServiceStopError::JoinTimedOut)?;

        tokio::time::timeout_at(
            deadline,
            std::future::poll_fn(|cx| {
                let mut replayer = self.lifecycle.replayer();
                let Some(handle) = replayer.as_mut() else {
                    return std::task::Poll::Ready(());
                };
                match Pin::new(handle).poll(cx) {
                    std::task::Poll::Ready(result) => {
                        if result.is_err() {
                            self.lifecycle.worker_failed.store(true, Ordering::Release);
                        }
                        replayer.take();
                        std::task::Poll::Ready(())
                    }
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }),
        )
        .await
        .map_err(|_| DirectServiceStopError::JoinTimedOut)?;

        while tokio::time::timeout_at(
            deadline,
            std::future::poll_fn(|cx| {
                let mut attempts = self.lifecycle.attempts();
                let Some(mut entry) = attempts.first_entry() else {
                    return std::task::Poll::Ready(false);
                };
                match Pin::new(entry.get_mut()).poll(cx) {
                    std::task::Poll::Ready(_) => {
                        entry.remove();
                        std::task::Poll::Ready(true)
                    }
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }),
        )
        .await
        .map_err(|_| DirectServiceStopError::JoinTimedOut)?
        {
            self.shared.notify();
        }
        if self.lifecycle.worker_failed.load(Ordering::Acquire) {
            return Err(DirectServiceStopError::WorkerFailed);
        }
        self.lifecycle.stop_complete.store(true, Ordering::Release);
        Ok(())
    }
}

impl PendingDurableDirectLxmfService {
    /// Start the prepared service and immediately activate queued generations.
    ///
    /// Aggregate owners with a persistence-restored readiness barrier must use
    /// [`Self::start_paused`] and call
    /// [`DurableDirectLxmfService::activate_queued_attempts`] after that barrier.
    pub async fn start(
        self,
        network: Arc<dyn DirectNetwork>,
        mailbox: Arc<dyn MailboxSubmitter>,
        clock: Arc<dyn MailboxClock>,
    ) -> Result<DurableDirectLxmfService, DurableDirectServiceStartError> {
        let service = self.start_paused(network, mailbox, clock).await?;
        let activated = service.activate_queued_attempts().await;
        debug_assert!(activated, "a newly started service must activate");
        Ok(service)
    }

    /// Start the callback worker but keep every network attempt paused.
    pub async fn start_paused(
        self,
        network: Arc<dyn DirectNetwork>,
        mailbox: Arc<dyn MailboxSubmitter>,
        clock: Arc<dyn MailboxClock>,
    ) -> Result<DurableDirectLxmfService, DurableDirectServiceStartError> {
        let Self {
            limits,
            identity,
            callbacks,
            receiver,
            refresh,
            shutdown,
            shutdown_receiver,
        } = self;
        limits.validate()?;
        let runtime = match tokio::runtime::Handle::try_current() {
            Ok(runtime) => runtime,
            Err(_) => {
                callbacks.health.stop();
                return Err(DurableDirectServiceStartError::NoTokioRuntime);
            }
        };
        let shared = Arc::new(DurableShared {
            local_destination: identity.destination(),
            identity: Arc::new(identity),
            network,
            mailbox,
            clock,
            peers: Mutex::new(PeerCache::default()),
            resolver: PeerResolver::default(),
            health: callbacks.health.clone(),
            mailbox_health: StdMutex::new(MailboxHealthTracker::default()),
            late_proof_count: AtomicU64::new(0),
            refresh,
        });
        let mailbox_policy = match shared.mailbox.submit(MailboxRequest::Policy).await {
            Ok(MailboxReply::Policy(policy)) => policy,
            Ok(_) => {
                shared.health.stop();
                return Err(DurableDirectServiceStartError::UnexpectedMailboxReply);
            }
            Err(failure) => {
                shared.health.stop();
                return Err(DurableDirectServiceStartError::Mailbox(failure));
            }
        };
        mailbox_policy
            .validate()
            .map_err(DurableDirectServiceStartError::Mailbox)?;
        let limits = ReplayLimits {
            max_page_rows: limits
                .max_page_rows
                .min(mailbox_policy.max_page_rows as usize),
            ..limits
        };
        let stopped = Arc::new(AtomicBool::new(false));
        let transition = Arc::new(Mutex::new(()));
        let worker = runtime.spawn(run_durable_worker(
            shared.clone(),
            Arc::clone(&transition),
            receiver,
            shutdown_receiver,
        ));
        let lifecycle = Arc::new(DurableLifecycle {
            shutdown,
            activated: AtomicBool::new(false),
            stopped,
            stop_complete: AtomicBool::new(false),
            stop_transition: Mutex::new(()),
            worker_failed: AtomicBool::new(false),
            transition,
            worker: StdMutex::new(Some(worker)),
            attempts: StdMutex::new(BTreeMap::new()),
            limits,
            replay_wake: Arc::new(Notify::new()),
            replayer: StdMutex::new(None),
        });
        let replayer = runtime.spawn(replay::run(
            shared.clone(),
            Arc::downgrade(&lifecycle),
            lifecycle.transition.clone(),
            lifecycle.replay_wake.clone(),
            lifecycle.shutdown.subscribe(),
        ));
        *lifecycle.replayer() = Some(replayer);
        Ok(DurableDirectLxmfService {
            shared,
            lifecycle,
            callbacks,
        })
    }
}

impl DurableShared {
    fn notify(&self) {
        self.refresh.send_modify(|revision| {
            *revision = revision.saturating_add(1);
        });
    }

    fn mailbox_health(&self) -> MailboxProjectionHealth {
        self.mailbox_health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .projection()
    }

    fn record_late_proof(&self) {
        if {
            let mut previous = self.late_proof_count.load(Ordering::Acquire);
            loop {
                let Some(next) = previous.checked_add(1) else {
                    break Err(previous);
                };
                match self.late_proof_count.compare_exchange_weak(
                    previous,
                    next,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(previous) => break Ok(previous),
                    Err(current) => previous = current,
                }
            }
        }
        .is_ok()
        {
            self.notify();
        }
    }

    fn set_mailbox_read_ready(&self) {
        self.update_mailbox_health(|health| {
            if !matches!(
                health.read_failure.as_ref(),
                Some(MailboxProjectionHealth::ResetRequired(_))
            ) {
                health.read_failure = None;
            }
        });
    }

    fn set_mailbox_write_ready(&self) {
        self.update_mailbox_health(|health| {
            if !matches!(
                health.read_failure.as_ref(),
                Some(MailboxProjectionHealth::ResetRequired(_))
            ) {
                health.read_failure = None;
            }
            if !matches!(
                health.write_failure.as_ref(),
                Some(MailboxProjectionHealth::ResetRequired(_))
            ) {
                health.write_failure = None;
            }
        });
    }

    fn set_mailbox_read_failure(&self, failure: &MailboxFailure) {
        let failure = Self::project_mailbox_failure(failure);
        self.update_mailbox_health(|health| {
            if !matches!(
                health.read_failure.as_ref(),
                Some(MailboxProjectionHealth::ResetRequired(_))
            ) {
                health.read_failure = Some(failure);
            }
        });
    }

    fn set_mailbox_write_failure(&self, failure: &MailboxFailure) {
        let failure = Self::project_mailbox_failure(failure);
        self.update_mailbox_health(|health| {
            if !matches!(
                health.write_failure.as_ref(),
                Some(MailboxProjectionHealth::ResetRequired(_))
            ) {
                health.write_failure = Some(failure);
            }
        });
    }

    fn project_mailbox_failure(failure: &MailboxFailure) -> MailboxProjectionHealth {
        match failure {
            MailboxFailure::Busy => MailboxProjectionHealth::Busy,
            MailboxFailure::Capacity => MailboxProjectionHealth::Capacity,
            MailboxFailure::Unavailable(detail) => {
                MailboxProjectionHealth::Unavailable(detail.clone())
            }
            MailboxFailure::ResetRequired(reason) => {
                MailboxProjectionHealth::ResetRequired(reason.clone())
            }
        }
    }

    fn set_unexpected_mailbox_write_failure(&self, detail: &str) {
        self.set_mailbox_write_failure(&MailboxFailure::unavailable(detail));
    }

    fn set_unexpected_mailbox_read_failure(&self, detail: &str) {
        self.set_mailbox_read_failure(&MailboxFailure::unavailable(detail));
    }

    fn update_mailbox_health(&self, update: impl FnOnce(&mut MailboxHealthTracker)) {
        let mut current = self
            .mailbox_health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = current.projection();
        update(&mut current);
        if current.projection() != before {
            drop(current);
            self.notify();
        }
    }
}

fn compose_new_outbound(
    source: [u8; 16],
    destination: [u8; 16],
    timestamp_unix_ms: u64,
    title: &[u8],
    content: &[u8],
    identity: &LocalLxmfIdentity,
) -> Result<NewOutboundMessage, BasicLxmfComposeError> {
    let mut output = [0_u8; MAX_BASIC_LXMF_WIRE_BYTES];
    let prepared = compose_basic_direct_lxmf(
        destination,
        source,
        timestamp_unix_ms,
        title,
        content,
        None,
        identity,
        &mut output,
    )?;
    Ok(NewOutboundMessage {
        message_id: prepared.message_id(),
        source,
        destination,
        timestamp_unix_ms,
        title: title.to_vec(),
        content: content.to_vec(),
        exact_wire: output[..usize::from(prepared.wire_len())].to_vec(),
    })
}

fn validate_recipient(
    peer: &LxmfPeer,
    expected_identity: Option<[u8; 16]>,
) -> Option<DurableSendDirectTextOutcome> {
    if let Some(expected) = expected_identity {
        if peer.announced_identity != expected {
            return Some(DurableSendDirectTextOutcome::PeerIdentityConflict {
                expected,
                observed: peer.announced_identity,
            });
        }
    }
    peer.required_stamp_cost.map(|required_stamp_cost| {
        DurableSendDirectTextOutcome::UnsupportedRemoteStampRequirement {
            required_stamp_cost,
        }
    })
}

fn spawn_attempt(
    shared: &Arc<DurableShared>,
    lifecycle: &Arc<DurableLifecycle>,
    message: DurableLxmfMessage,
) {
    let Some(generation) = message.generation() else {
        return;
    };
    if !matches!(
        message.delivery_state,
        DurableLxmfDeliveryState::Queued { .. }
    ) {
        return;
    }
    let key = AttemptKey {
        local_record_id: message.local_record_id,
        generation,
    };
    let mut attempts = lifecycle.attempts();
    if attempts.contains_key(&key) || attempts.len() >= lifecycle.limits.max_active_attempts {
        return;
    }
    let (registered, registration) = oneshot::channel();
    let task_shared = shared.clone();
    let weak_lifecycle: Weak<DurableLifecycle> = Arc::downgrade(lifecycle);
    let destination = message.destination;
    let exact_wire = message.exact_wire;
    let attempt = tokio::spawn(async move {
        if registration.await.is_err() {
            return;
        }
        let result =
            run_single_attempt(task_shared.network.as_ref(), destination, exact_wire).await;
        settle_durable_attempt(task_shared, weak_lifecycle, key, result).await;
    });
    attempts.insert(key, attempt);
    let _task_alive = registered.send(());
    drop(attempts);
    shared.notify();
}

fn schedule_attempt(lifecycle: &DurableLifecycle) {
    // Durable admission, not this wakeup, owns the accepted work. A lost wake
    // cannot remove a row; replay also wakes after settlement and on activation.
    lifecycle.replay_wake.notify_one();
}

async fn settle_durable_attempt(
    shared: Arc<DurableShared>,
    lifecycle: Weak<DurableLifecycle>,
    key: AttemptKey,
    result: Result<crate::direct::DirectDeliveryReceipt, DirectSendFailure>,
) {
    let Some(owner) = lifecycle.upgrade() else {
        return;
    };
    let completion = match result {
        Ok(receipt) => AttemptCompletion::Delivered {
            delivered_at_millis: shared.clock.now_unix_millis(),
            rtt_millis: Some(receipt.rtt_millis),
        },
        Err(failure) => AttemptCompletion::Failed { failure },
    };
    let mut shutdown = owner.shutdown.subscribe();
    let transition_owner = owner.transition.clone();
    drop(owner);
    loop {
        let transition = transition_owner.lock().await;
        let Some(owner) = lifecycle.upgrade() else {
            return;
        };
        if owner.stopped.load(Ordering::Acquire) {
            remove_active_attempt(&shared, &owner, key);
            return;
        }
        drop(owner);
        let reply = shared
            .mailbox
            .submit(MailboxRequest::CompleteAttempt { key, completion })
            .await;
        let Some(owner) = lifecycle.upgrade() else {
            return;
        };
        let retry = match reply {
            Ok(MailboxReply::Completion(CompletionTransition::Applied { .. })) => {
                shared.set_mailbox_write_ready();
                shared.notify();
                false
            }
            Ok(MailboxReply::Completion(CompletionTransition::Stale { .. })) => {
                shared.set_mailbox_read_ready();
                if matches!(completion, AttemptCompletion::Delivered { .. }) {
                    shared.record_late_proof();
                }
                false
            }
            Ok(_) => {
                shared.set_unexpected_mailbox_write_failure(
                    "the mailbox owner returned an unexpected completion reply",
                );
                true
            }
            Err(
                failure @ (MailboxFailure::Busy
                | MailboxFailure::Unavailable(_)
                | MailboxFailure::Capacity),
            ) => {
                shared.set_mailbox_write_failure(&failure);
                true
            }
            Err(failure @ MailboxFailure::ResetRequired(_)) => {
                owner.activated.store(false, Ordering::Release);
                shared.set_mailbox_write_failure(&failure);
                false
            }
        };
        if !retry {
            remove_active_attempt(&shared, &owner, key);
            return;
        }
        drop(owner);
        drop(transition);
        tokio::select! {
            () = tokio::time::sleep(SETTLEMENT_RETRY_DELAY) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    if let Some(owner) = lifecycle.upgrade() {
                        remove_active_attempt(&shared, &owner, key);
                    }
                    return;
                }
            }
        }
    }
}

fn remove_active_attempt(shared: &DurableShared, lifecycle: &DurableLifecycle, key: AttemptKey) {
    if lifecycle.attempts().remove(&key).is_some() {
        lifecycle.replay_wake.notify_one();
        shared.notify();
    }
}

async fn run_durable_worker(
    shared: Arc<DurableShared>,
    transition: Arc<Mutex<()>>,
    mut jobs: mpsc::Receiver<Job>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    // Receiver closure atomically rejects new queue entries;
                    // everything that won admission before it must be settled.
                    jobs.close();
                    while let Some(job) = jobs.recv().await {
                        process_durable_job(&shared, &transition, job).await;
                    }
                    break;
                }
            }
            job = jobs.recv() => {
                let Some(job) = job else {
                    shared.health.stop();
                    return;
                };
                process_durable_job(&shared, &transition, job).await;
                if jobs.capacity() > 0 {
                    shared.health.recover();
                }
            }
        }
    }
}

async fn process_durable_job(shared: &Arc<DurableShared>, transition: &Mutex<()>, job: Job) {
    match job {
        Job::Peer(job) => {
            if shared.peers.lock().await.observe(job) {
                shared.notify();
            }
        }
        Job::Inbound(job) => process_durable_inbound(shared, transition, job.wire).await,
    }
}

async fn process_durable_inbound(
    shared: &Arc<DurableShared>,
    transition: &Mutex<()>,
    wire: Vec<u8>,
) {
    let Ok(message) = MessageView::parse_ingress(
        CarrierIngress::LinkDataContextNone {
            expected_destination: &shared.local_destination,
            payload: &wire,
        },
        direct_wire_limits(),
    ) else {
        return;
    };
    if message.payload().stamp().is_some() {
        return;
    }
    let message_id = message.message_id();
    let source = *message.source_hash();
    let verification = crate::direct::verify_inbound_source(shared.network.as_ref(), message).await;
    let Some(timestamp_unix_ms) = timestamp_millis(message.payload().timestamp()) else {
        return;
    };
    let inbound = NewInboundMessage {
        message_id,
        source,
        destination: *message.destination_hash(),
        timestamp_unix_ms,
        title: message.payload().title().as_bytes().to_vec(),
        content: message.payload().content().as_bytes().to_vec(),
        exact_wire: wire,
        verification,
    };
    let _transition = transition.lock().await;
    match shared
        .mailbox
        .submit(MailboxRequest::InsertInbound(inbound))
        .await
    {
        Ok(MailboxReply::Inbound(
            InboundInsertOutcome::Inserted { .. }
            | InboundInsertOutcome::VerificationUpgraded { .. },
        )) => {
            shared.set_mailbox_write_ready();
            shared.notify();
        }
        Ok(MailboxReply::Inbound(InboundInsertOutcome::Duplicate { .. })) => {
            shared.set_mailbox_read_ready();
        }
        Ok(_) => shared.set_unexpected_mailbox_write_failure(
            "the mailbox owner returned an unexpected inbound reply",
        ),
        Err(failure) => shared.set_mailbox_write_failure(&failure),
    }
}

fn send_failure_outcome(failure: MailboxFailure) -> DurableSendDirectTextOutcome {
    match failure {
        MailboxFailure::Busy => DurableSendDirectTextOutcome::Busy,
        MailboxFailure::Capacity => DurableSendDirectTextOutcome::MailboxFull,
        MailboxFailure::Unavailable(detail) => DurableSendDirectTextOutcome::Unavailable { detail },
        MailboxFailure::ResetRequired(reason) => {
            DurableSendDirectTextOutcome::ResetRequired { reason }
        }
    }
}

fn retry_failure_outcome(failure: MailboxFailure) -> RetryLxmfMessageOutcome {
    match failure {
        MailboxFailure::Busy | MailboxFailure::Capacity => RetryLxmfMessageOutcome::Busy,
        MailboxFailure::Unavailable(detail) => RetryLxmfMessageOutcome::Unavailable { detail },
        MailboxFailure::ResetRequired(reason) => RetryLxmfMessageOutcome::ResetRequired { reason },
    }
}

fn cancel_failure_outcome(failure: MailboxFailure) -> CancelLxmfMessageOutcome {
    match failure {
        MailboxFailure::Busy | MailboxFailure::Capacity => CancelLxmfMessageOutcome::Busy,
        MailboxFailure::Unavailable(detail) => CancelLxmfMessageOutcome::Unavailable { detail },
        MailboxFailure::ResetRequired(reason) => CancelLxmfMessageOutcome::ResetRequired { reason },
    }
}

#[cfg(all(test, feature = "tokio-host"))]
mod tests;
