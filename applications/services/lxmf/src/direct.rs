//! Tokio-host direct Link-packet LXMF engine.

use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::string::String;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex as LifecycleMutex, MutexGuard as LifecycleMutexGuard, Weak};
use std::time::Duration;
use std::vec::Vec;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, DeliveryEvidence, EstablishLinkFailure,
    EstablishLinkRejection, SendToLinkFailure,
};
use personal_rns::identity::{PrivateIdentityMaterial, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::node_introspection::{DestinationIdentityQuery, NodeIntrospection};
use personal_rns::routing::announce::{
    derive_single_destination_hash, AnnounceObservation, ExpandNameError,
};
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::establish::WriteEstablishLinkRejection;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::links::LinkId;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Message, PreConfiguredDestination, PrnsEvent, ServeMyRequestEndpoints,
};
use personal_rns::units::ByteLimit;
use personal_rns::wire::DestinationHash;
use personal_rns::{PrnsNodeHandle, RatchetPolicy, SendError};
use prns_lxmf_wire::{
    compose_basic_direct_lxmf, normalize_lxmf_display_name, parse_lxmf_announce,
    BasicLxmfComposeError, BasicLxmfSigner, CarrierIngress, MessageView, WireLimits,
    MAX_BASIC_LXMF_WIRE_BYTES,
};
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio::task::JoinHandle;

use crate::{LxmfDeliveryState, LxmfDirection, LxmfHealth, LxmfHealthState, LxmfVerification};

/// Number of copied direct-packet jobs retained outside Prns callbacks.
pub const DIRECT_JOB_CAPACITY: usize = 64;
/// Bound on copied announce app-data before the worker parses it.
pub const MAX_ANNOUNCE_APP_DATA_BYTES: usize = 512;
/// Bound on a displayed peer name in the first service slice.
pub const MAX_DISPLAY_NAME_BYTES: usize = 255;
/// Maximum time explicit shutdown waits for cancelled service-owned tasks.
pub const STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Boxed async result used by the injected direct-network seam.
pub type DirectNetworkFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Network-side failure classification retained with a failed outbound record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectSendFailure {
    NoRoute,
    LinkFailed,
    DeliveryTimedOut,
    LocalNodeStopped,
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
}

/// Narrow network operations required by the direct engine.
///
/// [`PrnsDirectNetwork`] is the production adapter. The trait keeps deterministic
/// tests on the same public service API without pretending Prns has a portable
/// async node facade.
pub trait DirectNetwork: Send + Sync + 'static {
    fn has_route(&self, destination: [u8; 16]) -> DirectNetworkFuture<'_, bool>;
    fn request_path(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectSendFailure>>;
    fn establish_link(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<[u8; 16], DirectSendFailure>>;
    fn send_link_packet(
        &self,
        link: [u8; 16],
        complete_wire: Vec<u8>,
    ) -> DirectNetworkFuture<'_, Result<(), DirectSendFailure>>;
    fn destination_public_key(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Option<[u8; 64]>>;
    fn announce(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectAnnounceFailure>>;
}

/// Direct network adapter over one existing Tokio [`PrnsNodeHandle`].
#[derive(Clone)]
pub struct PrnsDirectNetwork {
    handle: PrnsNodeHandle,
}

impl PrnsDirectNetwork {
    #[must_use]
    pub fn new(handle: PrnsNodeHandle) -> Self {
        Self { handle }
    }
}

impl DirectNetwork for PrnsDirectNetwork {
    fn has_route(&self, destination: [u8; 16]) -> DirectNetworkFuture<'_, bool> {
        Box::pin(async move {
            self.handle
                .route(DestinationHash::new(destination))
                .await
                .is_some()
        })
    }

    fn request_path(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectSendFailure>> {
        Box::pin(async move {
            match self
                .handle
                .request_path(DestinationHash::new(destination))
                .await
            {
                Ok(_) => Ok(()),
                Err(personal_rns::runtime::RequestPathError::NodeStopped) => {
                    Err(DirectSendFailure::LocalNodeStopped)
                }
                Err(_) => Err(DirectSendFailure::NoRoute),
            }
        })
    }

    fn establish_link(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<[u8; 16], DirectSendFailure>> {
        Box::pin(async move {
            self.handle
                .establish_link(DestinationHash::new(destination))
                .await
                .map(|link| *link.as_bytes())
                .map_err(classify_establish_link_failure)
        })
    }

    fn send_link_packet(
        &self,
        link: [u8; 16],
        complete_wire: Vec<u8>,
    ) -> DirectNetworkFuture<'_, Result<(), DirectSendFailure>> {
        Box::pin(async move {
            match self
                .handle
                .send_link_packet(LinkId::new(link), &complete_wire)
                .await
            {
                Ok(receipt) if matches!(receipt.evidence, DeliveryEvidence::Proof(_)) => Ok(()),
                Ok(_) => Err(DirectSendFailure::LinkFailed),
                Err(SendError::NodeStopped) => Err(DirectSendFailure::LocalNodeStopped),
                Err(SendError::Failed(SendToLinkFailure::Timeout)) => {
                    Err(DirectSendFailure::DeliveryTimedOut)
                }
                Err(_) => Err(DirectSendFailure::LinkFailed),
            }
        })
    }

    fn destination_public_key(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Option<[u8; 64]>> {
        Box::pin(async move {
            self.handle
                .destination_identity(DestinationIdentityQuery::Destination(DestinationHash::new(
                    destination,
                )))
                .await
                .map(|snapshot| *snapshot.public.as_bytes())
        })
    }

    fn announce(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectAnnounceFailure>> {
        Box::pin(async move {
            self.handle
                .announce_now(AnnounceNow {
                    destination: DestinationHash::new(destination),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                })
                .await
                .map_err(|_| DirectAnnounceFailure::NodeRejected)
        })
    }
}

fn classify_establish_link_failure(failure: SendError<EstablishLinkFailure>) -> DirectSendFailure {
    match failure {
        SendError::Failed(EstablishLinkFailure::Rejected(
            EstablishLinkRejection::NoRouteToDestination,
        ))
        | SendError::Failed(EstablishLinkFailure::WriteFailed(
            WriteEstablishLinkRejection::RouteVanished,
        )) => DirectSendFailure::NoRoute,
        SendError::NodeStopped => DirectSendFailure::LocalNodeStopped,
        SendError::PayloadTooLarge
        | SendError::Busy
        | SendError::Failed(
            EstablishLinkFailure::Rejected(EstablishLinkRejection::NotDirectlyReachable)
            | EstablishLinkFailure::WriteFailed(
                WriteEstablishLinkRejection::Serialize
                | WriteEstablishLinkRejection::LinkTableFull
                | WriteEstablishLinkRejection::DuplicateLinkId,
            )
            | EstablishLinkFailure::Timeout,
        ) => DirectSendFailure::LinkFailed,
    }
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

/// Retain a zeroizing signer and move a separate zeroizing copy into the exact
/// direct-only Prns destination configuration.
pub fn prepare_local_lxmf_destination<'a>(
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    announce_app_data: &'a [u8],
) -> Result<(LocalLxmfIdentity, PreConfiguredDestination<'a>), ExpandNameError> {
    let retained = LocalLxmfIdentity::from_secret_bytes(&identity)?;
    let destination = PreConfiguredDestination::Single {
        app_name: prns_lxmf_wire::LXMF_APP_NAME,
        aspects: prns_lxmf_wire::LXMF_DELIVERY_ASPECTS,
        identity,
        announce_app_data,
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: ByteLimit::Maximum(0),
        request_endpoints: ServeMyRequestEndpoints::No,
    };
    Ok((retained, destination))
}

/// One authenticated peer learned through the accepted-announce observer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LxmfPeer {
    pub destination: [u8; 16],
    pub announced_identity: [u8; 16],
    pub display_name: Option<String>,
    pub required_stamp_cost: Option<u64>,
    pub observed_at_millis: u64,
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

/// Terminal method result from one awaited LXM1 direct-send attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendDirectTextOutcome {
    /// The in-memory record was started and its one Link packet obtained proof.
    Started {
        local_record_id: u64,
    },
    NeedsResource {
        wire_bytes: usize,
    },
    UnsupportedRemoteStampRequirement,
    PeerIdentityUnavailable,
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
}

#[derive(Clone)]
pub struct LxmfCallbacks {
    local_destination: [u8; 16],
    jobs: mpsc::Sender<Job>,
    health: Arc<LaneHealth>,
}

impl LxmfCallbacks {
    /// Build the one authenticated accepted-announce callback installed on the node.
    pub fn accepted_announce_observer(
        &self,
    ) -> impl for<'a> FnMut(AnnounceObservation<'a>) + Send + 'static {
        let callbacks = self.clone();
        move |observation| {
            let _outcome = callbacks.on_accepted_announce(observation);
        }
    }

    /// Validate the authenticated identity/name association, then copy only bounded facts.
    pub fn on_accepted_announce(&self, observation: AnnounceObservation<'_>) -> CallbackOutcome {
        if self.health.state() == LxmfHealthState::Stopped {
            return CallbackOutcome::Stopped;
        }
        let Ok(expected) = derive_single_destination_hash(
            &observation.announced_identity,
            prns_lxmf_wire::LXMF_APP_NAME,
            prns_lxmf_wire::LXMF_DELIVERY_ASPECTS,
        ) else {
            return CallbackOutcome::InvalidDestinationAssociation;
        };
        if expected != observation.destination {
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
            is_path_response: observation.is_path_response,
        });
        self.try_enqueue(job, false)
    }

    /// Route only Link DATA into the LXMF lane; diagnostics never discover peers.
    pub fn on_prns_event(&self, event: &PrnsEvent<'_>) -> CallbackOutcome {
        let PrnsEvent::Message(Message::Delivered(Delivery::Link(delivery))) = event else {
            return CallbackOutcome::Ignored;
        };
        if delivery.plaintext.len() > MAX_BASIC_LXMF_WIRE_BYTES {
            return CallbackOutcome::Oversized;
        }
        let job = Job::Inbound(InboundJob {
            wire: delivery.plaintext.to_vec(),
            arrived_at_millis: delivery.arrived_at.0,
            source_interface: *delivery.source_interface.as_bytes(),
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

/// Cloneable owner of one in-memory LXM1 generation.
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
            state: Mutex::new(ServiceState::default()),
            health: callbacks.health.clone(),
            refresh,
        });
        let worker = runtime.spawn(run_worker(shared.clone(), receiver, shutdown_receiver));
        let lifecycle = Arc::new(Lifecycle {
            shutdown,
            worker: LifecycleMutex::new(Some(worker)),
            attempts: LifecycleMutex::new(BTreeMap::new()),
            stop_lock: Mutex::new(()),
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
            identity,
            callbacks: callbacks.clone(),
            receiver,
            refresh,
            shutdown,
            shutdown_receiver,
        };
        (pending, callbacks)
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
            peers: state.peers.values().cloned().collect(),
            messages: state.messages.clone(),
            health: self.shared.health.snapshot(),
        }
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
        let peer = {
            let state = self.shared.state.lock().await;
            state.peers.get(&destination).cloned()
        };
        let Some(peer) = peer else {
            return SendDirectTextOutcome::PeerIdentityUnavailable;
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
        let local_record_id = {
            let mut state = self.shared.state.lock().await;
            let local_record_id = state.allocate_record_id();
            state.messages.push(LxmfMessage {
                local_record_id,
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
            });
            local_record_id
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
                        Ok(()) => message.delivery_state = LxmfDeliveryState::Delivered,
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
            let _finished = attempt.await;
        }
        outcome
    }

    /// Cancel and join every service-owned task within a fixed bound.
    ///
    /// This generation's peers, messages, and dedup set remain queryable only
    /// through existing clones and are intentionally disposable on restart.
    pub async fn stop(&self) -> Result<(), DirectServiceStopError> {
        let _stop_guard = self.lifecycle.stop_lock.lock().await;
        self.shared.health.stop();
        let _changed = self.lifecycle.shutdown.send(true);

        let mut tasks: Vec<JoinHandle<()>> = mem::take(&mut *self.lifecycle.attempts())
            .into_values()
            .collect();
        let worker = self.lifecycle.worker().take();
        if let Some(worker) = worker {
            tasks.push(worker);
        }
        for task in &tasks {
            task.abort();
        }

        let mut state = self.shared.state.lock().await;
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

        tokio::time::timeout(STOP_JOIN_TIMEOUT, async move {
            for task in tasks {
                let _finished = task.await;
            }
        })
        .await
        .map_err(|_| DirectServiceStopError::JoinTimedOut)
    }
}

const fn send_outcome(
    local_record_id: u64,
    result: Result<(), DirectSendFailure>,
) -> SendDirectTextOutcome {
    match result {
        Ok(()) => SendDirectTextOutcome::Started { local_record_id },
        Err(DirectSendFailure::NoRoute) => SendDirectTextOutcome::NoRoute,
        Err(DirectSendFailure::LinkFailed) => SendDirectTextOutcome::LinkFailed,
        Err(DirectSendFailure::DeliveryTimedOut) => SendDirectTextOutcome::DeliveryTimedOut,
        Err(DirectSendFailure::LocalNodeStopped) => SendDirectTextOutcome::LocalNodeStopped,
    }
}

async fn run_single_attempt(
    network: &dyn DirectNetwork,
    destination: [u8; 16],
    exact_wire: Vec<u8>,
) -> Result<(), DirectSendFailure> {
    if !network.has_route(destination).await {
        network.request_path(destination).await?;
    }
    let link = network.establish_link(destination).await?;
    network.send_link_packet(link, exact_wire).await
}

struct Shared {
    local_destination: [u8; 16],
    identity: Arc<LocalLxmfIdentity>,
    network: Arc<dyn DirectNetwork>,
    state: Mutex<ServiceState>,
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

#[derive(Default)]
struct ServiceState {
    peers: BTreeMap<[u8; 16], LxmfPeer>,
    messages: Vec<LxmfMessage>,
    seen_message_ids: HashSet<[u8; 32]>,
    next_record_id: u64,
}

impl ServiceState {
    fn allocate_record_id(&mut self) -> u64 {
        self.next_record_id = self.next_record_id.saturating_add(1);
        self.next_record_id
    }
}

struct LaneHealth {
    state: AtomicU8,
    inbound_overflow_count: AtomicU64,
    refresh: watch::Sender<u64>,
}

impl LaneHealth {
    fn new(refresh: watch::Sender<u64>) -> Self {
        Self {
            state: AtomicU8::new(0),
            inbound_overflow_count: AtomicU64::new(0),
            refresh,
        }
    }

    fn state(&self) -> LxmfHealthState {
        match self.state.load(Ordering::Acquire) {
            0 => LxmfHealthState::Ready,
            1 => LxmfHealthState::Degraded,
            _ => LxmfHealthState::Stopped,
        }
    }

    fn snapshot(&self) -> LxmfHealth {
        LxmfHealth {
            state: self.state(),
            inbound_overflow_count: self.inbound_overflow_count.load(Ordering::Acquire),
        }
    }

    fn degrade(&self, inbound: bool) {
        if inbound {
            let _previous = self.inbound_overflow_count.fetch_update(
                Ordering::AcqRel,
                Ordering::Acquire,
                |count| Some(count.saturating_add(1)),
            );
        }
        if self
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.notify();
        }
    }

    fn recover(&self) {
        if self
            .state
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.notify();
        }
    }

    fn stop(&self) {
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

enum Job {
    Peer(PeerJob),
    Inbound(InboundJob),
}

struct PeerJob {
    destination: [u8; 16],
    announced_identity: [u8; 16],
    app_data: Vec<u8>,
    observed_at_millis: u64,
    is_path_response: bool,
}

struct InboundJob {
    wire: Vec<u8>,
    arrived_at_millis: u64,
    source_interface: [u8; 8],
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
    let Ok(announce) = parse_lxmf_announce(&job.app_data, MAX_DISPLAY_NAME_BYTES) else {
        return;
    };
    let mut normalized_display_name = [0u8; MAX_DISPLAY_NAME_BYTES];
    let display_name = announce.display_name.and_then(|bytes| {
        normalize_lxmf_display_name(bytes, &mut normalized_display_name)
            .ok()
            .map(String::from)
    });
    let peer = LxmfPeer {
        destination: job.destination,
        announced_identity: job.announced_identity,
        display_name,
        required_stamp_cost: announce.required_stamp_cost,
        observed_at_millis: job.observed_at_millis,
        is_path_response: job.is_path_response,
    };
    shared
        .state
        .lock()
        .await
        .peers
        .insert(job.destination, peer);
    shared.notify();
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
    let verification = match shared.network.destination_public_key(source).await {
        None => LxmfVerification::SourceUnknown,
        Some(public) => match message.bind_source_identity(&public) {
            Ok(bound) if message.verify_signature(&bound).is_ok() => LxmfVerification::Verified,
            Ok(_) | Err(_) => LxmfVerification::InvalidSignature,
        },
    };
    let timestamp_seconds = message.payload().timestamp();
    let timestamp_unix_ms = if timestamp_seconds.is_finite() && timestamp_seconds >= 0.0 {
        (timestamp_seconds * 1_000.0) as u64
    } else {
        0
    };
    let mut state = shared.state.lock().await;
    if !state.seen_message_ids.insert(message_id) {
        return;
    }
    let local_record_id = state.allocate_record_id();
    state.messages.push(LxmfMessage {
        local_record_id,
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
    });
    drop(state);
    shared.notify();
}

#[cfg(test)]
mod tests;
