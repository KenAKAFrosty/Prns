//! Injected Tokio direct Link-packet LXMF engine.

use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::string::String;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex as LifecycleMutex, MutexGuard as LifecycleMutexGuard, Weak};
use std::time::Duration;
use std::vec::Vec;

use personal_rns::identity::{PrivateIdentityMaterial, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routing::announce::{derive_single_destination_hash, ExpandNameError};
use personal_rns::routing::delivery::Delivery;
use personal_rns::runtime::{Message, PrnsEvent};
use prns_lxmf_wire::{
    compose_basic_direct_lxmf, BasicLxmfComposeError, BasicLxmfSigner, CarrierIngress, MessageView,
    WireLimits, MAX_BASIC_LXMF_WIRE_BYTES, WIRE_HEADER_LENGTH,
};
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio::task::JoinHandle;

use crate::{LxmfDeliveryState, LxmfDirection, LxmfHealth, LxmfHealthState, LxmfVerification};

#[cfg(feature = "tokio-host")]
mod host;
#[cfg(feature = "tokio-host")]
pub use host::*;

mod limits;
pub use limits::DirectLimits;

mod peers;
pub(crate) use peers::{PeerCache, PeerResolver, ResolvePeerFailure};
pub use peers::{DISCOVERED_PEER_CAPACITY, DISCOVERED_PEER_MAX_AGE, RECIPIENT_RESOLUTION_TIMEOUT};

/// Number of copied direct-packet jobs retained outside Prns callbacks.
pub const DIRECT_JOB_CAPACITY: usize = 64;
/// Bound on copied announce app-data before the worker parses it.
pub const MAX_ANNOUNCE_APP_DATA_BYTES: usize = 512;
/// Bound on a displayed peer name.
pub const MAX_DISPLAY_NAME_BYTES: usize = 255;
/// Maximum time explicit shutdown waits for cancelled service-owned tasks.
pub const STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(2);
const KEY_USE_RETENTION_TIMEOUT: Duration = Duration::from_secs(1);

/// Boxed async result used by the injected direct-network seam.
pub type DirectNetworkFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub use crate::DirectSendFailure;

/// Proof-backed settlement facts returned by the direct network adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectDeliveryReceipt {
    pub rtt_millis: u64,
}

/// Failure to ask the owning node to emit the registered LXMF announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectAnnounceFailure {
    NodeRejected,
}

/// Bounded shutdown failed to join every service-owned task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectServiceStopError {
    JoinTimedOut,
    /// A service-owned task failed unexpectedly; successful cleanup is unproven.
    WorkerFailed,
}

/// Narrow network operations required by the direct engine.
///
/// [`PrnsDirectNetwork`] is the production adapter. The trait keeps deterministic
/// tests on the same public service API; production uses the shared host client.
pub trait DirectNetwork: Send + Sync + 'static {
    fn has_route(&self, destination: [u8; 16]) -> DirectNetworkFuture<'_, bool>;
    fn request_path(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectSendFailure>>;
    /// Establish a link whose responder proof authenticates this destination.
    /// Return one newly established Link. If the waiter is cancelled, retain
    /// establishment through its result and close any late successful Link.
    fn establish_link(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<[u8; 16], DirectSendFailure>>;
    /// Take ownership of this Link through settlement and closure, including
    /// rejected admission or a cancelled waiter. Do not close unrelated Links.
    fn send_link_packet(
        &self,
        link: [u8; 16],
        complete_wire: Vec<u8>,
    ) -> DirectNetworkFuture<'_, Result<DirectDeliveryReceipt, DirectSendFailure>>;
    fn destination_public_key(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Option<[u8; 64]>>;
    /// Best-effort refresh of the host's bounded used-key retention policy.
    /// Called only after authenticated link establishment or successful LXMF
    /// source/signature verification. It must not permanently retain a key,
    /// request a path, or announce. Failure must not change delivery outcomes;
    /// the service bounds the wait even when an adapter cannot complete it.
    fn mark_destination_used(&self, destination: [u8; 16]) -> DirectNetworkFuture<'_, ()>;
    fn announce(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectAnnounceFailure>>;
}

/// Retained signing authority and derived local `lxmf.delivery` address.
pub struct LocalLxmfIdentity {
    material: PrivateIdentityMaterial,
    destination: [u8; 16],
}

impl LocalLxmfIdentity {
    pub fn from_secret_bytes(
        secret: &[u8; IDENTITY_SECRET_KEY_LEN],
    ) -> Result<Self, ExpandNameError> {
        let material = PrivateIdentityMaterial::from_bytes(*secret);
        let destination = derive_single_destination_hash(
            &material.identity_hash(),
            prns_lxmf_wire::LXMF_APP_NAME,
            prns_lxmf_wire::LXMF_DELIVERY_ASPECTS,
        )?;
        Ok(Self {
            material,
            destination: *destination.as_bytes(),
        })
    }

    #[must_use]
    pub const fn destination(&self) -> [u8; 16] {
        self.destination
    }

    #[must_use]
    pub fn public_key(&self) -> [u8; 64] {
        *self.material.public().as_bytes()
    }
}

impl BasicLxmfSigner for LocalLxmfIdentity {
    fn sign_lxmf(&self, input: &[u8]) -> [u8; 64] {
        self.material.sign(input).0
    }
}

/// One authenticated peer learned through the accepted-announce observer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LxmfPeer {
    pub destination: [u8; 16],
    pub announced_identity: [u8; 16],
    pub display_name: Option<String>,
    pub required_stamp_cost: Option<u64>,
    pub observed_at_millis: u64,
    /// Monotonic age when this projection was captured; never a wall-clock date.
    pub age_millis: u64,
    pub source_interface: [u8; 8],
    pub hops: u8,
    pub is_path_response: bool,
}

/// One in-memory logical LXMF message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LxmfMessage {
    pub local_record_id: u64,
    pub message_id: [u8; 32],
    pub source: [u8; 16],
    pub destination: [u8; 16],
    pub timestamp_unix_ms: u64,
    pub title: Vec<u8>,
    pub content: Vec<u8>,
    pub exact_wire: Vec<u8>,
    pub direction: LxmfDirection,
    pub verification: LxmfVerification,
    pub delivery_state: LxmfDeliveryState,
    pub failure: Option<DirectSendFailure>,
    /// Monotonic Prns arrival instant for inbound records only.
    pub arrived_at_millis: Option<u64>,
    /// Prns interface that delivered an inbound record, when applicable.
    pub source_interface: Option<[u8; 8]>,
}

/// Coherent query snapshot; refresh notifications are only hints to query again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LxmfSnapshot {
    pub peers: Vec<LxmfPeer>,
    pub messages: Vec<LxmfMessage>,
    pub health: LxmfHealth,
}

/// Terminal method result from one awaited direct-send attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendDirectTextOutcome {
    /// Selected retained-history policy refuses a new record; existing records remain.
    HistoryFull,
    /// The in-memory record was started and its one Link packet obtained proof.
    Started {
        local_record_id: u64,
    },
    NeedsResource {
        wire_bytes: usize,
    },
    UnsupportedRemoteStampRequirement,
    PeerIdentityUnavailable,
    RecipientResolutionTimedOut,
    Busy,
    NoRoute,
    LinkFailed,
    DeliveryTimedOut,
    LocalNodeStopped,
    InvalidMessage,
}

/// Result of a synchronous Prns callback handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackOutcome {
    Enqueued,
    Ignored,
    InvalidDestinationAssociation,
    Oversized,
    LaneFull,
    Stopped,
}

/// Construction failure for the host worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectServiceStartError {
    NoTokioRuntime,
    InvalidLimits,
    AllocationFailed,
}

#[derive(Clone)]
pub struct LxmfCallbacks {
    pub(crate) local_destination: [u8; 16],
    pub(crate) jobs: mpsc::Sender<Job>,
    pub(crate) health: Arc<LaneHealth>,
}

impl LxmfCallbacks {
    /// Build the binding-neutral authenticated observer installed by the shared host.
    #[cfg(feature = "tokio-host")]
    pub fn authenticated_announce_observer(
        &self,
    ) -> impl for<'a> FnMut(prns_host_runtime::AuthenticatedAnnounce<'a>) + Send + 'static {
        let callbacks = self.clone();
        move |observation| {
            let _outcome = callbacks.on_authenticated_announce(&observation);
        }
    }

    /// Accept a signature-verified announce from the owning protocol's accepted
    /// announce observer. Raw/unverified packets must never call this method.
    /// The identity/name association is checked again before bounded copying.
    pub fn on_accepted_announce(
        &self,
        observation: &personal_rns::routing::announce::AnnounceObservation<'_>,
    ) -> CallbackOutcome {
        if self.health.state() == LxmfHealthState::Stopped {
            return CallbackOutcome::Stopped;
        }
        if observation.destination.as_bytes() == &self.local_destination {
            return CallbackOutcome::Ignored;
        }
        let identity = observation.announced_identity;
        let Ok(expected) = derive_single_destination_hash(
            &identity,
            prns_lxmf_wire::LXMF_APP_NAME,
            prns_lxmf_wire::LXMF_DELIVERY_ASPECTS,
        ) else {
            return CallbackOutcome::InvalidDestinationAssociation;
        };
        if expected.as_bytes() != observation.destination.as_bytes() {
            return CallbackOutcome::InvalidDestinationAssociation;
        }
        if observation.app_data.len() > MAX_ANNOUNCE_APP_DATA_BYTES {
            return CallbackOutcome::Oversized;
        }
        let job = Job::Peer(PeerJob {
            destination: *observation.destination.as_bytes(),
            announced_identity: *observation.announced_identity.as_bytes(),
            app_data: observation.app_data.to_vec(),
            observed_at_millis: observation.arrived_at.0,
            observed_at: tokio::time::Instant::now(),
            source_interface: *observation.source_interface.as_bytes(),
            hops: observation.hops.0,
            is_path_response: observation.is_path_response,
        });
        self.try_enqueue(job, false)
    }

    /// Adapt the canonical host's authenticated observation to the same engine path.
    #[cfg(feature = "tokio-host")]
    pub fn on_authenticated_announce(
        &self,
        observation: &prns_host_runtime::AuthenticatedAnnounce<'_>,
    ) -> CallbackOutcome {
        self.on_accepted_announce(&personal_rns::routing::announce::AnnounceObservation {
            destination: personal_rns::wire::DestinationHash::new(
                observation.destination.into_bytes(),
            ),
            announced_identity: personal_rns::identity::IdentityHash::new(
                observation.announced_identity.into_bytes(),
            ),
            source_interface: personal_rns::interfaces::InterfaceId::new(
                observation.source_interface.into_bytes(),
            ),
            hops: personal_rns::units::HopCount(observation.hops),
            arrived_at: personal_rns::units::InstantMillis(observation.arrived_at_millis),
            app_data: observation.app_data,
            is_path_response: observation.is_path_response,
        })
    }

    /// Route a canonical Link DATA event only when the authenticated link belongs
    /// to this service's registered local destination.
    #[cfg(feature = "tokio-host")]
    pub fn on_host_link_delivery(&self, delivery: &prns_host::LinkDelivery) -> CallbackOutcome {
        self.on_scoped_link_delivery(
            delivery
                .local_destination
                .map(prns_host::DestinationHash::into_bytes),
            &delivery.plaintext,
            delivery.arrived_at_millis,
            delivery.source_interface.into_bytes(),
        )
    }

    /// Native protocol callback compatibility; it applies the same destination
    /// ownership check as canonical ingress and never treats all Link DATA as LXMF.
    pub fn on_prns_event(&self, event: &PrnsEvent<'_>) -> CallbackOutcome {
        let PrnsEvent::Message(Message::Delivered(Delivery::Link(delivery))) = event else {
            return CallbackOutcome::Ignored;
        };
        self.on_scoped_link_delivery(
            delivery
                .local_destination
                .map(|destination| *destination.as_bytes()),
            delivery.plaintext,
            delivery.arrived_at.0,
            *delivery.source_interface.as_bytes(),
        )
    }

    fn on_scoped_link_delivery(
        &self,
        local_destination: Option<[u8; 16]>,
        plaintext: &[u8],
        arrived_at_millis: u64,
        source_interface: [u8; 8],
    ) -> CallbackOutcome {
        if local_destination != Some(self.local_destination) {
            return CallbackOutcome::Ignored;
        }
        if plaintext.len() > MAX_BASIC_LXMF_WIRE_BYTES {
            return CallbackOutcome::Oversized;
        }
        let job = Job::Inbound(InboundJob {
            wire: plaintext.to_vec(),
            arrived_at_millis,
            source_interface,
        });
        self.try_enqueue(job, true)
    }

    fn try_enqueue(&self, job: Job, inbound: bool) -> CallbackOutcome {
        if self.health.state() == LxmfHealthState::Stopped {
            return CallbackOutcome::Stopped;
        }
        match self.jobs.try_send(job) {
            Ok(()) => CallbackOutcome::Enqueued,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.health.degrade(inbound);
                CallbackOutcome::LaneFull
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.health.stop();
                CallbackOutcome::Stopped
            }
        }
    }
}

/// Cloneable owner of one in-memory direct-delivery generation.
#[derive(Clone)]
pub struct DirectLxmfService {
    shared: Arc<Shared>,
    callbacks: LxmfCallbacks,
    lifecycle: Arc<Lifecycle>,
}

/// Prepared callback lane used while the owning aggregate constructs its node.
///
/// Preparing before node construction breaks the intentional lifecycle cycle:
/// callbacks are installed into the node recipe first, then the node handle is
/// wrapped in a [`PrnsDirectNetwork`] and starts this pending service.
pub struct PendingDirectLxmfService {
    limits: DirectLimits,
    identity: LocalLxmfIdentity,
    callbacks: LxmfCallbacks,
    receiver: mpsc::Receiver<Job>,
    refresh: watch::Sender<u64>,
    shutdown: watch::Sender<bool>,
    shutdown_receiver: watch::Receiver<bool>,
}

struct Lifecycle {
    shutdown: watch::Sender<bool>,
    worker: LifecycleMutex<Option<JoinHandle<()>>>,
    attempts: LifecycleMutex<BTreeMap<u64, JoinHandle<()>>>,
    stop_lock: Mutex<()>,
    worker_failed: AtomicBool,
}

impl Lifecycle {
    fn worker(&self) -> LifecycleMutexGuard<'_, Option<JoinHandle<()>>> {
        match self.worker.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn attempts(&self) -> LifecycleMutexGuard<'_, BTreeMap<u64, JoinHandle<()>>> {
        match self.attempts.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn observe_join(&self, result: Result<(), tokio::task::JoinError>) {
        if result.is_err_and(|error| !error.is_cancelled()) {
            self.worker_failed.store(true, Ordering::Release);
        }
    }
}

impl Drop for Lifecycle {
    fn drop(&mut self) {
        let worker = self.worker().take();
        if let Some(worker) = worker {
            worker.abort();
        }
        let attempts = mem::take(&mut *self.attempts());
        for (_record_id, attempt) in attempts {
            attempt.abort();
        }
    }
}

impl PendingDirectLxmfService {
    /// Start the prepared lane after the aggregate has created its node handle.
    pub fn start(
        self,
        network: Arc<dyn DirectNetwork>,
    ) -> Result<DirectLxmfService, DirectServiceStartError> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| DirectServiceStartError::NoTokioRuntime)?;
        let Self {
            limits,
            identity,
            callbacks,
            receiver,
            refresh,
            shutdown,
            shutdown_receiver,
        } = self;
        let shared = Arc::new(Shared {
            local_destination: identity.destination(),
            identity: Arc::new(identity),
            network,
            peers: Mutex::new(PeerCache::default()),
            resolver: PeerResolver::default(),
            state: Mutex::new(ServiceState::new(limits)?),
            limits,
            health: callbacks.health.clone(),
            refresh,
        });
        let worker = runtime.spawn(run_worker(shared.clone(), receiver, shutdown_receiver));
        let lifecycle = Arc::new(Lifecycle {
            shutdown,
            worker: LifecycleMutex::new(Some(worker)),
            attempts: LifecycleMutex::new(BTreeMap::new()),
            stop_lock: Mutex::new(()),
            worker_failed: AtomicBool::new(false),
        });
        Ok(DirectLxmfService {
            shared,
            callbacks,
            lifecycle,
        })
    }
}

impl DirectLxmfService {
    /// Prepare callbacks before the aggregate constructs its one Prns node.
    #[must_use]
    pub fn prepare(identity: LocalLxmfIdentity) -> (PendingDirectLxmfService, LxmfCallbacks) {
        let local_destination = identity.destination();
        let (jobs, receiver) = mpsc::channel(DIRECT_JOB_CAPACITY);
        let (refresh, _refresh_receiver) = watch::channel(0u64);
        let health = Arc::new(LaneHealth::new(refresh.clone()));
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let callbacks = LxmfCallbacks {
            local_destination,
            jobs,
            health,
        };
        let pending = PendingDirectLxmfService {
            limits: DirectLimits::default(),
            identity,
            callbacks: callbacks.clone(),
            receiver,
            refresh,
            shutdown,
            shutdown_receiver,
        };
        (pending, callbacks)
    }

    /// Select finite retained history and active work. Full history is never
    /// evicted automatically; admission reports capacity until a new generation.
    pub fn prepare_with_limits(
        identity: LocalLxmfIdentity,
        limits: DirectLimits,
    ) -> Result<(PendingDirectLxmfService, LxmfCallbacks), DirectServiceStartError> {
        limits.validate()?;
        let (mut pending, callbacks) = Self::prepare(identity);
        pending.limits = limits;
        Ok((pending, callbacks))
    }

    /// Convenience start for hosts that do not need callbacks during node construction.
    pub fn start(
        identity: LocalLxmfIdentity,
        network: Arc<dyn DirectNetwork>,
    ) -> Result<Self, DirectServiceStartError> {
        let (pending, _callbacks) = Self::prepare(identity);
        pending.start(network)
    }

    #[must_use]
    pub fn callbacks(&self) -> LxmfCallbacks {
        self.callbacks.clone()
    }

    #[must_use]
    pub const fn local_destination(&self) -> [u8; 16] {
        self.callbacks.local_destination
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.shared.refresh.subscribe()
    }

    pub async fn snapshot(&self) -> LxmfSnapshot {
        let state = self.shared.state.lock().await;
        LxmfSnapshot {
            peers: self.shared.peers.lock().await.discovered(),
            messages: state.messages.clone(),
            health: self.shared.health.snapshot(),
        }
    }

    /// Clear visible observations without deleting retained recipient metadata or routes.
    pub async fn clear_discovered_peers(&self) {
        self.shared.peers.lock().await.clear_discovered();
        self.shared.notify();
    }

    pub async fn announce(&self) -> Result<(), DirectAnnounceFailure> {
        self.shared
            .network
            .announce(self.shared.local_destination)
            .await
    }

    /// Compose once, publish `Sending`, await one attempt, then return its terminal outcome.
    pub async fn send_direct_text(
        &self,
        destination: [u8; 16],
        timestamp_unix_ms: u64,
        title: &[u8],
        content: &[u8],
    ) -> SendDirectTextOutcome {
        if self.shared.health.state() == LxmfHealthState::Stopped {
            return SendDirectTextOutcome::LocalNodeStopped;
        }
        if destination == self.local_destination() {
            return SendDirectTextOutcome::PeerIdentityUnavailable;
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
                return SendDirectTextOutcome::RecipientResolutionTimedOut
            }
            Err(ResolvePeerFailure::Stopped) => return SendDirectTextOutcome::LocalNodeStopped,
            Err(ResolvePeerFailure::Unavailable) => {
                return SendDirectTextOutcome::PeerIdentityUnavailable
            }
            Err(ResolvePeerFailure::Busy) => return SendDirectTextOutcome::Busy,
        };
        if peer.required_stamp_cost.is_some() {
            return SendDirectTextOutcome::UnsupportedRemoteStampRequirement;
        }

        let mut output = [0u8; MAX_BASIC_LXMF_WIRE_BYTES];
        let prepared = match compose_basic_direct_lxmf(
            destination,
            self.shared.local_destination,
            timestamp_unix_ms,
            title,
            content,
            None,
            self.shared.identity.as_ref(),
            &mut output,
        ) {
            Ok(prepared) => prepared,
            Err(BasicLxmfComposeError::NeedsResource { wire_bytes }) => {
                return SendDirectTextOutcome::NeedsResource { wire_bytes };
            }
            Err(_) => return SendDirectTextOutcome::InvalidMessage,
        };
        let exact_wire = output[..usize::from(prepared.wire_len())].to_vec();
        let admission = self.lifecycle.stop_lock.lock().await;
        if self.shared.health.state() == LxmfHealthState::Stopped {
            return SendDirectTextOutcome::LocalNodeStopped;
        }
        let current = self.shared.peers.lock().await.recipient(&destination);
        let Some(current) = current else {
            return SendDirectTextOutcome::PeerIdentityUnavailable;
        };
        if current.required_stamp_cost.is_some() {
            return SendDirectTextOutcome::UnsupportedRemoteStampRequirement;
        }
        if self.lifecycle.attempts().len() >= self.shared.limits.max_active_attempts {
            return SendDirectTextOutcome::Busy;
        }
        let local_record_id = {
            let mut state = self.shared.state.lock().await;
            let message = LxmfMessage {
                local_record_id: 0,
                message_id: prepared.message_id(),
                source: self.shared.local_destination,
                destination,
                timestamp_unix_ms,
                title: title.to_vec(),
                content: content.to_vec(),
                exact_wire: exact_wire.clone(),
                direction: LxmfDirection::Outbound,
                verification: LxmfVerification::Verified,
                delivery_state: LxmfDeliveryState::Sending,
                failure: None,
                arrived_at_millis: None,
                source_interface: None,
            };
            match state.admit(message, self.shared.limits) {
                Some(id) => id,
                None => return SendDirectTextOutcome::HistoryFull,
            }
        };
        self.shared.notify();
        let completed = {
            let mut attempts = self.lifecycle.attempts();
            let shared = self.shared.clone();
            let lifecycle: Weak<Lifecycle> = Arc::downgrade(&self.lifecycle);
            let (completion, completed) = oneshot::channel();
            let (registered, registration) = oneshot::channel();
            let attempt = tokio::spawn(async move {
                if registration.await.is_err() {
                    return;
                }
                let result =
                    run_single_attempt(shared.network.as_ref(), destination, exact_wire).await;
                let mut state = shared.state.lock().await;
                if let Some(message) = state
                    .messages
                    .iter_mut()
                    .find(|message| message.local_record_id == local_record_id)
                {
                    match result {
                        Ok(_) => message.delivery_state = LxmfDeliveryState::Delivered,
                        Err(failure) => {
                            message.delivery_state = LxmfDeliveryState::Failed;
                            message.failure = Some(failure);
                        }
                    }
                }
                drop(state);
                shared.notify();
                if completion
                    .send(send_outcome(local_record_id, result))
                    .is_err()
                {
                    if let Some(lifecycle) = lifecycle.upgrade() {
                        lifecycle.attempts().remove(&local_record_id);
                    }
                }
            });
            attempts.insert(local_record_id, attempt);
            let _task_alive = registered.send(());
            completed
        };
        drop(admission);
        let outcome = completed
            .await
            .unwrap_or(SendDirectTextOutcome::LocalNodeStopped);
        let attempt = {
            let mut attempts = self.lifecycle.attempts();
            attempts.remove(&local_record_id)
        };
        if let Some(attempt) = attempt {
            self.lifecycle.observe_join(attempt.await);
        }
        outcome
    }

    /// Cancel and join every service-owned task within a fixed bound.
    ///
    /// This generation's peers, messages, and dedup set remain queryable only
    /// through existing clones and are intentionally disposable on restart.
    pub async fn stop(&self) -> Result<(), DirectServiceStopError> {
        self.shared.health.stop();
        let _changed = self.lifecycle.shutdown.send(true);
        let deadline = tokio::time::Instant::now() + STOP_JOIN_TIMEOUT;
        let _stop_guard = tokio::time::timeout_at(deadline, self.lifecycle.stop_lock.lock())
            .await
            .map_err(|_| DirectServiceStopError::JoinTimedOut)?;

        for task in self.lifecycle.attempts().values() {
            task.abort();
        }
        if let Some(worker) = self.lifecycle.worker().as_ref() {
            worker.abort();
        }

        let mut state = tokio::time::timeout_at(deadline, self.shared.state.lock())
            .await
            .map_err(|_| DirectServiceStopError::JoinTimedOut)?;
        let mut changed = false;
        for message in &mut state.messages {
            if message.delivery_state == LxmfDeliveryState::Sending {
                message.delivery_state = LxmfDeliveryState::Failed;
                message.failure = Some(DirectSendFailure::LocalNodeStopped);
                changed = true;
            }
        }
        drop(state);
        if changed {
            self.shared.notify();
        }

        // Keep handles in the owner while awaiting them. A cancelled or timed
        // out stop can retry the same join boundary instead of detaching work.
        tokio::time::timeout_at(
            deadline,
            std::future::poll_fn(|cx| {
                let mut worker = self.lifecycle.worker();
                let Some(handle) = worker.as_mut() else {
                    return std::task::Poll::Ready(());
                };
                match Pin::new(handle).poll(cx) {
                    std::task::Poll::Ready(result) => {
                        self.lifecycle.observe_join(result);
                        worker.take();
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
                    std::task::Poll::Ready(result) => {
                        self.lifecycle.observe_join(result);
                        entry.remove();
                        std::task::Poll::Ready(true)
                    }
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }),
        )
        .await
        .map_err(|_| DirectServiceStopError::JoinTimedOut)?
        {}
        if self.lifecycle.worker_failed.load(Ordering::Acquire) {
            return Err(DirectServiceStopError::WorkerFailed);
        }
        Ok(())
    }
}

const fn send_outcome(
    local_record_id: u64,
    result: Result<DirectDeliveryReceipt, DirectSendFailure>,
) -> SendDirectTextOutcome {
    match result {
        Ok(_) => SendDirectTextOutcome::Started { local_record_id },
        Err(DirectSendFailure::NoRoute) => SendDirectTextOutcome::NoRoute,
        Err(DirectSendFailure::LinkFailed) => SendDirectTextOutcome::LinkFailed,
        Err(DirectSendFailure::DeliveryTimedOut) => SendDirectTextOutcome::DeliveryTimedOut,
        Err(DirectSendFailure::LocalNodeStopped) => SendDirectTextOutcome::LocalNodeStopped,
    }
}

pub(crate) async fn run_single_attempt(
    network: &dyn DirectNetwork,
    destination: [u8; 16],
    exact_wire: Vec<u8>,
) -> Result<DirectDeliveryReceipt, DirectSendFailure> {
    if !network.has_route(destination).await {
        network.request_path(destination).await?;
    }
    let link = network.establish_link(destination).await?;
    // Transfer the Link to the guarded send without yielding to unrelated
    // bookkeeping after establishment. Cancellation cannot strand it between
    // these two operations.
    let result = network.send_link_packet(link, exact_wire).await;
    mark_authenticated_key_used(network, destination).await;
    result
}

async fn mark_authenticated_key_used(network: &dyn DirectNetwork, destination: [u8; 16]) {
    // Retention is bookkeeping, not part of transport proof or message verification.
    // A stopped or stalled host must not turn an otherwise valid result into failure.
    let _retention = tokio::time::timeout(
        KEY_USE_RETENTION_TIMEOUT,
        network.mark_destination_used(destination),
    )
    .await;
}

pub(crate) async fn verify_inbound_source(
    network: &dyn DirectNetwork,
    message: MessageView<'_>,
) -> LxmfVerification {
    let source = *message.source_hash();
    let Some(public) = network.destination_public_key(source).await else {
        return LxmfVerification::SourceUnknown;
    };
    let Ok(bound) = message.bind_source_identity(&public) else {
        return LxmfVerification::InvalidSignature;
    };
    if message.verify_signature(&bound).is_err() {
        return LxmfVerification::InvalidSignature;
    }
    mark_authenticated_key_used(network, source).await;
    LxmfVerification::Verified
}

struct Shared {
    local_destination: [u8; 16],
    identity: Arc<LocalLxmfIdentity>,
    network: Arc<dyn DirectNetwork>,
    peers: Mutex<PeerCache>,
    resolver: PeerResolver,
    state: Mutex<ServiceState>,
    limits: DirectLimits,
    health: Arc<LaneHealth>,
    refresh: watch::Sender<u64>,
}

impl Shared {
    fn notify(&self) {
        self.refresh.send_modify(|revision| {
            *revision = revision.saturating_add(1);
        });
    }
}

struct ServiceState {
    messages: Vec<LxmfMessage>,
    seen_message_ids: HashSet<[u8; 32]>,
    next_record_id: u64,
    message_bytes: usize,
}

impl ServiceState {
    fn new(limits: DirectLimits) -> Result<Self, DirectServiceStartError> {
        limits.validate()?;
        let mut messages = Vec::new();
        messages
            .try_reserve_exact(limits.max_messages)
            .map_err(|_| DirectServiceStartError::AllocationFailed)?;
        let message_bytes = messages
            .capacity()
            .checked_mul(mem::size_of::<LxmfMessage>())
            .ok_or(DirectServiceStartError::InvalidLimits)?;
        if message_bytes > limits.max_message_bytes {
            return Err(DirectServiceStartError::InvalidLimits);
        }
        Ok(Self {
            messages,
            seen_message_ids: HashSet::new(),
            next_record_id: 0,
            message_bytes,
        })
    }

    fn admit(&mut self, mut message: LxmfMessage, limits: DirectLimits) -> Option<u64> {
        if self.messages.len() >= limits.max_messages {
            return None;
        }
        let heap = message
            .title
            .capacity()
            .checked_add(message.content.capacity())?
            .checked_add(message.exact_wire.capacity())?;
        let bytes = self.message_bytes.checked_add(heap)?;
        let id = self.next_record_id.checked_add(1)?;
        if bytes > limits.max_message_bytes {
            return None;
        }
        message.local_record_id = id;
        // The slots were reserved at construction, so this push cannot grow them.
        self.messages.push(message);
        self.next_record_id = id;
        self.message_bytes = bytes;
        Some(id)
    }
}

pub(crate) struct LaneHealth {
    state: AtomicU8,
    inbound_overflow_count: AtomicU64,
    refresh: watch::Sender<u64>,
}

impl LaneHealth {
    pub(crate) fn new(refresh: watch::Sender<u64>) -> Self {
        Self {
            state: AtomicU8::new(0),
            inbound_overflow_count: AtomicU64::new(0),
            refresh,
        }
    }

    pub(crate) fn state(&self) -> LxmfHealthState {
        match self.state.load(Ordering::Acquire) {
            0 => LxmfHealthState::Ready,
            1 => LxmfHealthState::Degraded,
            _ => LxmfHealthState::Stopped,
        }
    }

    pub(crate) fn snapshot(&self) -> LxmfHealth {
        LxmfHealth {
            state: self.state(),
            inbound_overflow_count: self.inbound_overflow_count.load(Ordering::Acquire),
        }
    }

    fn degrade(&self, inbound: bool) {
        if inbound {
            let _previous = {
                let mut previous = self.inbound_overflow_count.load(Ordering::Acquire);
                loop {
                    let next = previous.saturating_add(1);
                    match self.inbound_overflow_count.compare_exchange_weak(
                        previous,
                        next,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => break next,
                        Err(current) => previous = current,
                    }
                }
            };
        }
        if self
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.notify();
        }
    }

    pub(crate) fn recover(&self) {
        if self
            .state
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.notify();
        }
    }

    pub(crate) fn stop(&self) {
        if self.state.swap(2, Ordering::AcqRel) != 2 {
            self.notify();
        }
    }

    fn notify(&self) {
        self.refresh.send_modify(|revision| {
            *revision = revision.saturating_add(1);
        });
    }
}

pub(crate) enum Job {
    Peer(PeerJob),
    Inbound(InboundJob),
}

pub(crate) struct PeerJob {
    pub(crate) destination: [u8; 16],
    pub(crate) announced_identity: [u8; 16],
    pub(crate) app_data: Vec<u8>,
    pub(crate) observed_at_millis: u64,
    pub(crate) observed_at: tokio::time::Instant,
    pub(crate) source_interface: [u8; 8],
    pub(crate) hops: u8,
    pub(crate) is_path_response: bool,
}

pub(crate) struct InboundJob {
    pub(crate) wire: Vec<u8>,
    pub(crate) arrived_at_millis: u64,
    pub(crate) source_interface: [u8; 8],
}

async fn run_worker(
    shared: Arc<Shared>,
    mut jobs: mpsc::Receiver<Job>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            job = jobs.recv() => {
                let Some(job) = job else {
                    shared.health.stop();
                    return;
                };
                process_job(&shared, job).await;
                if jobs.capacity() > 0 {
                    shared.health.recover();
                }
            }
        }
    }
}

async fn process_job(shared: &Arc<Shared>, job: Job) {
    match job {
        Job::Peer(job) => process_peer(shared, job).await,
        Job::Inbound(job) => process_inbound(shared, job).await,
    }
}

async fn process_peer(shared: &Arc<Shared>, job: PeerJob) {
    if shared.peers.lock().await.observe(job) {
        shared.notify();
    }
}

async fn process_inbound(shared: &Arc<Shared>, job: InboundJob) {
    let limits = WireLimits::new(
        MAX_BASIC_LXMF_WIRE_BYTES,
        MAX_BASIC_LXMF_WIRE_BYTES,
        128,
        512,
        8_192,
        16,
    );
    let Ok(message) = MessageView::parse_ingress(
        CarrierIngress::LinkDataContextNone {
            expected_destination: &shared.local_destination,
            payload: &job.wire,
        },
        limits,
    ) else {
        return;
    };
    // Stamped five-item payloads are retained by the generic wire parser for a
    // later package but are not admitted by this exact-four direct service.
    if message.payload().stamp().is_some() {
        return;
    }
    let message_id = message.message_id();
    let source = *message.source_hash();
    let verification = verify_inbound_source(shared.network.as_ref(), message).await;
    let timestamp_seconds = message.payload().timestamp();
    let timestamp_unix_ms = if timestamp_seconds.is_finite() && timestamp_seconds >= 0.0 {
        (timestamp_seconds * 1_000.0) as u64
    } else {
        0
    };
    let mut state = shared.state.lock().await;
    if state.seen_message_ids.contains(&message_id) {
        let Some(existing) = state.messages.iter_mut().find(|record| {
            record.direction == LxmfDirection::Inbound && record.message_id == message_id
        }) else {
            return;
        };
        // A message ID excludes its signature. Keep the first receipt's local
        // identity and arrival metadata, but allow a source-verified replay to
        // replace an unverified signature for the exact same signed body.
        if existing.verification != LxmfVerification::Verified
            && verification == LxmfVerification::Verified
            && existing.source == source
            && &existing.destination == message.destination_hash()
            && existing.exact_wire[WIRE_HEADER_LENGTH..] == job.wire[WIRE_HEADER_LENGTH..]
        {
            let old_bytes = existing.exact_wire.capacity();
            let new_bytes = job.wire.capacity();
            // Both buffers come from the bounded copied ingress. Replacing a
            // signature still updates actual retained capacity accounting.
            let total = state
                .message_bytes
                .checked_sub(old_bytes)
                .and_then(|bytes| bytes.checked_add(new_bytes));
            if total.is_none_or(|bytes| bytes > shared.limits.max_message_bytes) {
                shared.health.degrade(true);
                return;
            }
            let Some(existing) = state.messages.iter_mut().find(|record| {
                record.direction == LxmfDirection::Inbound && record.message_id == message_id
            }) else {
                return;
            };
            existing.verification = verification;
            existing.exact_wire = job.wire;
            state.message_bytes = total.unwrap_or(state.message_bytes);
            drop(state);
            shared.notify();
        }
        return;
    }
    let record = LxmfMessage {
        local_record_id: 0,
        message_id,
        source,
        destination: *message.destination_hash(),
        timestamp_unix_ms,
        title: message.payload().title().as_bytes().to_vec(),
        content: message.payload().content().as_bytes().to_vec(),
        exact_wire: job.wire,
        direction: LxmfDirection::Inbound,
        verification,
        delivery_state: LxmfDeliveryState::Received,
        failure: None,
        arrived_at_millis: Some(job.arrived_at_millis),
        source_interface: Some(job.source_interface),
    };
    if state.admit(record, shared.limits).is_none() {
        shared.health.degrade(true);
        return;
    }
    state.seen_message_ids.insert(message_id);
    drop(state);
    shared.notify();
}

#[cfg(all(test, feature = "tokio-host"))]
mod tests;

#[cfg(test)]
mod runtime_tests;
