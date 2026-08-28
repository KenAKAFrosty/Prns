use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use futures_util::FutureExt;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::engine::{
    CommandId, EstablishLinkFailure, Journaled, LinkEstablished, PacketReceiptDelivered,
    PersistenceFlushTarget, PrnsCommand, ProofRequest, SendGroupFailure, SendPlainPacketFailure,
    SendSinglePacketFailure, SetTransportIdentityError,
};
use crate::identity::held::HoldIdentityError;
use crate::identity::{IdentityHash, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::InterfaceId;
use crate::manifold::compression;
use crate::manifold::driver::{
    self as manifold_driver, CryptoPoolConfig, Egress, HostCommand, ProvideDecompressedHostCommand,
    TokioHost,
};
use crate::remote_control::{RemoteControlEndpoint, RemoteControlNodeIdentities};
use crate::routing::announce::AnnounceObservation;
use crate::routing::links::resources::ResourceMemoryLimits;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestHandlerError, RequestPolicy};
use crate::storage::{GrowableHeap, StorageLayout, TablePushError};
use crate::units::RttMillis;
use crate::wire::DestinationHash;

use prns_runtime::runtime::{
    assemble_node, configure_preconfigured_destination, AssembledNode,
    ConfigurePreconfiguredDestinationError, Diagnostic,
};

use super::super::remote_control_access::{
    remote_control_access_lane, RemoteControlAccessReceiver,
};
use super::super::request_endpoints::{RequestEndpoint, RequestEndpointSet, RespondToken};
use super::super::request_runner::{run_router, RunnerRequest, REQUEST_QUEUE_DEPTH};
use super::super::{
    InterfaceStore, Message, PreConfiguredDestination, PrnsEvent, PrnsNodeRecipe, SendError,
};
use super::interface_lifecycle::{drive_interfaces, DriverMsg};
use super::resource_transfer::resource_segment_decompression_bound;
use super::{persistence, AttachIntent, PrnsNodeHandle};

const INFLATE_QUEUE_PER_WORKER: usize = 4;
const MAX_INFLATE_PARALLELISM: usize = 8;

fn inflate_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .clamp(1, MAX_INFLATE_PARALLELISM)
}

type AcceptedAnnounceObserver = Box<dyn for<'a> FnMut(AnnounceObservation<'a>) + Send>;

fn notify_accepted_announce(
    observer: &mut Option<AcceptedAnnounceObserver>,
    journaled: &Journaled<'_>,
) {
    if let Journaled::AnnounceHeard { observation, .. } = journaled {
        if let Some(observer) = observer.as_mut() {
            observer(*observation);
        }
    }
}

/// A node on the tokio host. Built from a [`PrnsNodeRecipe`] with [`new`](Self::new)
/// (synchronous: it wires the engine and spawns each interface), then driven by
/// [`run`](Self::run) or [`run_until`](Self::run_until). Hold [`handle`](Self::handle)
/// clones to drive it from other tasks or threads while either method owns the loop.
pub struct PrnsNode<St, R, F, S: StorageLayout> {
    handle: PrnsNodeHandle,
    pub(super) host: TokioHost,
    pub(super) node: AssembledNode<St, R, F, S>,
    notify_rx: UnboundedReceiver<InterfaceId>,
    command_rx: UnboundedReceiver<HostCommand>,
    remote_control_access_rx: RemoteControlAccessReceiver,
    iface_build_rx: UnboundedReceiver<DriverMsg>,
    accepted_announce_observer: Option<AcceptedAnnounceObserver>,
    pub(super) crypto_pool: CryptoPoolConfig,
    persistence: Option<persistence::NodePersistence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonRoutingIdentityError {
    Hold(HoldIdentityError),
    Configure(SetTransportIdentityError),
}

pub type SharedInstanceIdentityError = NonRoutingIdentityError;

impl<St, R, F> PrnsNode<St, R, F, GrowableHeap>
where
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
{
    /// Applies independent incoming and outgoing active Resource buffer budgets
    /// before the node is run.
    #[must_use]
    pub fn with_resource_memory_limits(mut self, limits: ResourceMemoryLimits) -> Self {
        self.node.engine.set_resource_memory_limits(limits);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterRequestEndpointError {
    Registration(TablePushError),
    Seed(RequestHandlerError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRunError {
    ManifoldPanicked,
    RequestEndpointrPanicked,
    InterfaceDriverPanicked,
    PersistenceFailed,
    PersistenceWorkerStopped,
}

impl fmt::Display for NodeRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManifoldPanicked => formatter.write_str("the node manifold panicked"),
            Self::RequestEndpointrPanicked => formatter.write_str("the request router panicked"),
            Self::InterfaceDriverPanicked => formatter.write_str("the interface driver panicked"),
            Self::PersistenceFailed => {
                formatter.write_str("the recipe-managed persistence worker could not save")
            }
            Self::PersistenceWorkerStopped => formatter.write_str(
                "the recipe-managed persistence worker stopped before completing its contract",
            ),
        }
    }
}

impl std::error::Error for NodeRunError {}

async fn run_node_tasks(
    manifold: impl Future<Output = ()>,
    request_endpoints: impl Future<Output = ()>,
    interface_driver: impl Future<Output = ()>,
) -> Result<(), NodeRunError> {
    let manifold = AssertUnwindSafe(manifold).catch_unwind();
    let request_endpoints = AssertUnwindSafe(request_endpoints).catch_unwind();
    let interface_driver = AssertUnwindSafe(interface_driver).catch_unwind();
    tokio::pin!(manifold, request_endpoints, interface_driver);
    tokio::select! {
        result = &mut manifold => result.map_err(|_| NodeRunError::ManifoldPanicked),
        result = &mut request_endpoints => result.map_err(|_| NodeRunError::RequestEndpointrPanicked),
        result = &mut interface_driver => result.map_err(|_| NodeRunError::InterfaceDriverPanicked),
    }
}

fn persistence_restored_diagnostic(
    report: &persistence::PersistenceRestoreReport,
) -> Diagnostic<'static> {
    Diagnostic::PersistenceRestored {
        routes: report.routes.seeded_count,
        destination_identities: report.destination_identities.seeded_count,
        tunnels: report.tunnels.seeded_count,
        ratchets: report.ratchets.seeded_count,
        refused: report.refused_total(),
        dropped: report.dropped_total(),
    }
}

fn note_landed_persistence_flush(
    commands: &UnboundedSender<HostCommand>,
    observations: &mut Vec<oneshot::Receiver<()>>,
    trigger: persistence::PersistenceTrigger,
    target: PersistenceFlushTarget,
) {
    let observed = if matches!(trigger, persistence::PersistenceTrigger::Shutdown) {
        let (observed, observation) = oneshot::channel();
        observations.push(observation);
        Some(observed)
    } else {
        None
    };
    let _ignored = commands.send(HostCommand::NotePersistenceFlush {
        cause: trigger.into(),
        target,
        observed,
    });
}

fn note_failed_persistence_flush(
    commands: &UnboundedSender<HostCommand>,
    observations: &mut Vec<oneshot::Receiver<()>>,
    trigger: persistence::PersistenceTrigger,
    target: PersistenceFlushTarget,
    error: &dyn fmt::Display,
) {
    #[cfg(feature = "tracing")]
    tracing::error!(
        target: "prns.runtime",
        event = "persistence_flush_failed",
        cause = trigger.name(),
        persistence_target = target.name(),
        error = %error,
    );
    #[cfg(not(feature = "tracing"))]
    let _ = error;

    let (observed, observation) = oneshot::channel();
    observations.push(observation);
    let _ignored = commands.send(HostCommand::NotePersistenceFlushFailure {
        cause: trigger.into(),
        target,
        observed,
    });
}

fn note_persistence_event(
    commands: &UnboundedSender<HostCommand>,
    observations: &mut Vec<oneshot::Receiver<()>>,
    event: persistence::PersistenceEvent<'_>,
) {
    match event {
        persistence::PersistenceEvent::Flushed { trigger, .. } => note_landed_persistence_flush(
            commands,
            observations,
            trigger,
            PersistenceFlushTarget::RoutingState,
        ),
        persistence::PersistenceEvent::RatchetsFlushed { trigger, .. } => {
            note_landed_persistence_flush(
                commands,
                observations,
                trigger,
                PersistenceFlushTarget::Ratchets,
            );
        }
        persistence::PersistenceEvent::FlushFailed { trigger, error } => {
            note_failed_persistence_flush(
                commands,
                observations,
                trigger,
                PersistenceFlushTarget::RoutingState,
                error,
            );
        }
        persistence::PersistenceEvent::RatchetFlushFailed { trigger, error } => {
            note_failed_persistence_flush(
                commands,
                observations,
                trigger,
                PersistenceFlushTarget::Ratchets,
                error,
            );
        }
    }
}

async fn run_recipe_persistence(
    worker: persistence::PersistenceWorker,
    shutdown: impl Future<Output = ()>,
    commands: UnboundedSender<HostCommand>,
) -> Result<(), NodeRunError> {
    let mut observations = Vec::new();
    let status = worker
        .run(shutdown, |event| {
            note_persistence_event(&commands, &mut observations, event);
        })
        .await;
    for observation in observations {
        if observation.await.is_err() {
            return Err(NodeRunError::PersistenceWorkerStopped);
        }
    }
    match status {
        persistence::PersistenceFlushStatus::Landed => Ok(()),
        persistence::PersistenceFlushStatus::Failed => Err(NodeRunError::PersistenceFailed),
        persistence::PersistenceFlushStatus::NodeStopped => {
            Err(NodeRunError::PersistenceWorkerStopped)
        }
    }
}

impl<St, R, F, S: StorageLayout> PrnsNode<St, R, F, S>
where
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
{
    /// Stand a node up from `recipe` on the storage layout it names: assemble the engine (transport role, destinations, the request endpoints), then let the recipe's `interfaces` intent attach the node's edges through its own handle. Only [`run`](Self::run) awaits.
    pub fn new<'a, D, I, P>(recipe: PrnsNodeRecipe<'a, D, St, R, F, I, S, P>) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'a>>,
        I: AttachIntent,
        P: persistence::PersistenceIntent,
    {
        Self::new_with_handle(|_| recipe)
    }

    pub fn new_with_handle<'a, D, I, P, B>(build_recipe: B) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'a>>,
        I: AttachIntent,
        P: persistence::PersistenceIntent,
        B: FnOnce(PrnsNodeHandle) -> PrnsNodeRecipe<'a, D, St, R, F, I, S, P>,
    {
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (iface_build_tx, iface_build_rx) = mpsc::unbounded_channel();
        let (remote_control_access, remote_control_access_rx) = remote_control_access_lane();

        let handle = PrnsNodeHandle {
            commands: command_tx,
            ids: Arc::new(AtomicU64::new(0)),
            attachment_epochs: Arc::new(AtomicU64::new(0)),
            notify_tx,
            iface_build: iface_build_tx,
            interfaces: Arc::new(Mutex::new(HashMap::new())),
            store: InterfaceStore::new(),
            resource_admission: super::resource_admission::ResourceAdmissionRegistry::default(),
            entropy: crate::manifold::driver::TokioEntropy,
            timing_oracle: Arc::new(Mutex::new(None)),
            remote_control_access,
        };
        let (node, interfaces, persistence_intent) = assemble_node(build_recipe(handle.clone()));
        let node_persistence =
            persistence::PersistenceIntent::into_node_persistence(persistence_intent);
        interfaces.attach(&handle);

        PrnsNode {
            handle,
            host: TokioHost::start_at(
                node_persistence
                    .as_ref()
                    .map(persistence::NodePersistence::timeline_origin)
                    .unwrap_or_else(persistence::wall_clock_timeline_origin),
            ),
            node,
            notify_rx,
            command_rx,
            remote_control_access_rx,
            iface_build_rx,
            accepted_announce_observer: None,
            crypto_pool: CryptoPoolConfig::host_default(),
            persistence: node_persistence,
        }
    }

    pub fn with_non_routing_identity(
        mut self,
        secret: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    ) -> Result<Self, NonRoutingIdentityError> {
        let identity = self
            .node
            .engine
            .hold_identity(secret)
            .map_err(NonRoutingIdentityError::Hold)?;
        self.node
            .engine
            .set_non_routing_identity(&identity)
            .map_err(NonRoutingIdentityError::Configure)?;
        Ok(self)
    }

    pub fn with_shared_instance_identity(
        self,
        secret: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    ) -> Result<Self, SharedInstanceIdentityError> {
        self.with_non_routing_identity(secret)
    }

    #[must_use]
    pub fn with_protocol_policy(mut self, policy: crate::engine::EngineProtocolPolicy) -> Self {
        self.node.engine.set_protocol_policy(policy);
        self
    }

    pub fn register_preconfigured_destination<'a>(
        &mut self,
        destination: PreConfiguredDestination<'a>,
    ) -> Result<DestinationHash, ConfigurePreconfiguredDestinationError> {
        configure_preconfigured_destination::<St, R, S>(&mut self.node.engine, destination)
    }

    pub fn allow_requester(
        &mut self,
        destination: &DestinationHash,
        path: &str,
        identity: IdentityHash,
    ) -> Result<(), RequestHandlerError> {
        self.node
            .engine
            .allow_requester(destination, path, identity)
    }

    pub fn register_request_route<Route>(
        &mut self,
        destination: &DestinationHash,
    ) -> Result<(), RegisterRequestEndpointError>
    where
        Route: RequestEndpoint<St>,
    {
        self.node
            .engine
            .register_request_handler(
                destination,
                Route::ENDPOINT_ID,
                Route::POLICY.engine_policy(),
            )
            .map_err(RegisterRequestEndpointError::Registration)?;
        for identity in Route::POLICY.seed_list() {
            self.node
                .engine
                .allow_requester(destination, Route::ENDPOINT_ID, *identity)
                .map_err(RegisterRequestEndpointError::Seed)?;
        }
        Ok(())
    }

    pub fn register_request_path(
        &mut self,
        destination: &DestinationHash,
        path: &str,
        policy: RequestPolicy,
    ) -> Result<(), TablePushError> {
        self.node
            .engine
            .register_request_handler(destination, path, policy)
    }

    #[must_use]
    pub fn with_accepted_announce_observer(
        mut self,
        observer: impl for<'a> FnMut(AnnounceObservation<'a>) + Send + 'static,
    ) -> Self {
        self.accepted_announce_observer = Some(Box::new(observer));
        self
    }

    #[must_use]
    pub fn clock(&self) -> TokioHost {
        self.host.clone()
    }

    /// Override how this node runs its asymmetric crypto. Defaults to `CryptoPoolConfig::host_default` (pooled on capable hosts, inline on mobile).
    #[must_use]
    pub fn with_crypto_pool(mut self, crypto_pool: CryptoPoolConfig) -> Self {
        self.crypto_pool = crypto_pool;
        self
    }

    /// A `Send + Clone` handle for other tasks or threads to drive the node while
    /// [`run`](Self::run) or [`run_until`](Self::run_until) owns the loop.
    #[must_use]
    pub fn handle(&self) -> PrnsNodeHandle {
        self.handle.clone()
    }

    #[must_use]
    pub const fn remote_control_identities(&self) -> Option<&RemoteControlNodeIdentities> {
        self.node.remote_control.identities()
    }

    #[must_use]
    pub const fn remote_control_target_endpoint(&self) -> Option<RemoteControlEndpoint> {
        self.node.remote_control.target_endpoint()
    }

    pub fn issue(&self, command: PrnsCommand) -> Option<CommandId> {
        self.handle.issue(command)
    }

    pub async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        self.handle.send_single_packet(destination, data).await
    }

    pub async fn send_plain_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendPlainPacketFailure>> {
        self.handle.send_plain_packet(destination, data).await
    }

    pub async fn send_group_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendGroupFailure>> {
        self.handle.send_group_packet(destination, data).await
    }

    pub async fn establish_link(
        &self,
        destination: DestinationHash,
    ) -> Result<LinkId, SendError<EstablishLinkFailure>> {
        self.handle.establish_link(destination).await
    }

    pub async fn establish_link_with_rtt(
        &self,
        destination: DestinationHash,
    ) -> Result<LinkEstablished, SendError<EstablishLinkFailure>> {
        self.handle.establish_link_with_rtt(destination).await
    }

    pub fn respond_packed(&self, responder: RespondToken, packed: &[u8]) -> Option<RttMillis> {
        self.handle.respond_packed(responder, packed)
    }

    pub fn close_link(&self, link_id: LinkId) -> bool {
        self.handle.close_link(link_id)
    }

    /// Drive the node until it stops (in practice forever).
    ///
    /// Dropping or cancelling this future cannot perform asynchronous cleanup. Call
    /// [`run_until`](Self::run_until) when recipe-managed persistence must land a final
    /// state and ratchet flush.
    pub async fn run(self) -> Result<(), NodeRunError> {
        self.run_with_proof_decider(|_| false).await
    }

    /// Drive the node with the synchronous application decision used by destinations
    /// configured with [`ProofStrategy::ProveIf`](crate::routing::ProofStrategy::ProveIf).
    ///
    /// The closure is kept in the run future and consulted inline after delivery is
    /// journaled. Prns allocates no policy table or per-packet decision state; capture a
    /// shared handle to caller-owned state when proof policy must change at runtime.
    /// [`run`](Self::run) keeps the default posture of declining every request.
    pub async fn run_with_proof_decider<P>(self, should_prove: P) -> Result<(), NodeRunError>
    where
        P: FnMut(&ProofRequest) -> bool,
    {
        self.run_until_with_proof_decider(core::future::pending::<()>(), should_prove)
            .await
    }

    /// Drive the node until `shutdown` resolves, then finish recipe-managed persistence
    /// before returning.
    ///
    /// The manifold and request runner stay active while the persistence worker takes
    /// and commits its final state and ratchet snapshots. Successful shutdown flushes,
    /// or a terminal persistence failure, reach the recipe's `on_event` callback before
    /// this method returns.
    pub async fn run_until(self, shutdown: impl Future<Output = ()>) -> Result<(), NodeRunError> {
        self.run_until_with_proof_decider(shutdown, |_| false).await
    }

    /// [`run_until`](Self::run_until) with a synchronous application proof decision.
    pub async fn run_until_with_proof_decider<P>(
        mut self,
        shutdown: impl Future<Output = ()>,
        should_prove: P,
    ) -> Result<(), NodeRunError>
    where
        P: FnMut(&ProofRequest) -> bool,
    {
        let restored = match self.persistence.take() {
            Some(node_persistence) => {
                let report = node_persistence.restore(&mut self);
                Some((node_persistence, report))
            }
            None => None,
        };
        let PrnsNode {
            handle,
            host,
            node,
            notify_rx,
            command_rx,
            mut remote_control_access_rx,
            iface_build_rx,
            mut accepted_announce_observer,
            crypto_pool,
            persistence: _,
        } = self;
        let AssembledNode {
            engine,
            mut remote_control,
            state,
            mut on_event,
            request_endpoints: _,
        } = node;
        let (save_on_learn, persistence_worker, restore_diagnostic) = match restored {
            Some((node_persistence, report)) => {
                let (save_on_learn, wiring) = persistence::SaveOnLearn::channel();
                let worker = node_persistence
                    .worker(handle.clone())
                    .with_save_on_learn(wiring)
                    .with_flush_failure_policy(persistence::FlushFailurePolicy::Exit);
                (
                    Some(save_on_learn),
                    Some(worker),
                    Some(persistence_restored_diagnostic(&report)),
                )
            }
            None => (None, None, None),
        };
        let egress = Egress::new(std::vec::Vec::new());
        let store = handle.store.clone();
        let (req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE_DEPTH);
        let inflate_commands = handle.commands.clone();
        let inflate_workers = inflate_parallelism();
        let inflate_admission = Arc::new(tokio::sync::Semaphore::new(
            inflate_workers.saturating_mul(INFLATE_QUEUE_PER_WORKER),
        ));
        let inflate_execution = Arc::new(tokio::sync::Semaphore::new(inflate_workers));
        let admission_decider = handle.resource_admission.clone();
        let admission_cleanup = handle.resource_admission.clone();
        let manifold = async {
            if let Some(diagnostic) = restore_diagnostic {
                let event = PrnsEvent::Diagnostic(diagnostic);
                #[cfg(feature = "tracing")]
                super::super::tracing_events::emit(&event);
                on_event(event, &state);
            }
            manifold_driver::run_with_store_and_deciders(
                engine,
                host,
                manifold_driver::ManifoldWiring {
                    interfaces: std::vec::Vec::new(),
                    ifacs: std::vec::Vec::new(),
                    notify: notify_rx,
                    inbound_lanes: std::vec::Vec::new(),
                    commands: command_rx,
                    egress,
                },
                |journaled| {
                    if let Journaled::LinkClosed { link_id, .. } = &journaled {
                        admission_cleanup.remove(*link_id);
                    }
                    if let Journaled::ResourceNeedsDecompression {
                        link_id,
                        hash,
                        stream,
                        uncompressed_data_bytes,
                    } = &journaled
                    {
                        let (link_id, hash, uncompressed_data_bytes) =
                            (*link_id, *hash, *uncompressed_data_bytes);
                        let stream = stream.to_vec();
                        let commands = inflate_commands.clone();
                        let admission = inflate_admission.clone().try_acquire_owned();
                        let execution = inflate_execution.clone();
                        let Ok(admission) = admission else {
                            let _ = commands.send(HostCommand::ProvideDecompressed(
                                ProvideDecompressedHostCommand {
                                    link_id,
                                    hash,
                                    plaintext: std::vec::Vec::new().into(),
                                },
                            ));
                            return;
                        };
                        tokio::spawn(async move {
                            let Ok(execution) = execution.acquire_owned().await else {
                                return;
                            };
                            let plaintext = tokio::task::spawn_blocking(move || {
                                compression::decompress_bounded(
                                    &stream,
                                    resource_segment_decompression_bound(uncompressed_data_bytes),
                                )
                                .unwrap_or_default()
                            })
                            .await
                            .unwrap_or_default();
                            drop(execution);
                            drop(admission);
                            let _ = commands.send(HostCommand::ProvideDecompressed(
                                ProvideDecompressedHostCommand {
                                    link_id,
                                    hash,
                                    plaintext: plaintext.into(),
                                },
                            ));
                        });
                        return;
                    }
                    notify_accepted_announce(&mut accepted_announce_observer, &journaled);
                    let event = PrnsEvent::from(journaled);
                    if let Some(save_on_learn) = &save_on_learn {
                        save_on_learn.observe(&event);
                    }
                    #[cfg(feature = "tracing")]
                    super::super::tracing_events::emit(&event);
                    if let PrnsEvent::Message(Message::Request {
                        destination,
                        link_id,
                        request_id,
                        requester,
                        path_hash,
                        requested_at,
                        rtt,
                        data,
                    }) = &event
                    {
                        let _ = req_tx.try_send(RunnerRequest {
                            destination: *destination,
                            link_id: *link_id,
                            request_id: *request_id,
                            requester: *requester,
                            path_hash: *path_hash,
                            requested_at: *requested_at,
                            rtt: *rtt,
                            data: data.to_vec(),
                        });
                    }
                    on_event(event, &state);
                },
                store,
                crypto_pool,
                crate::manifold::AppDeciders {
                    should_prove,
                    should_accept_resource: move |offer| admission_decider.permits(offer),
                },
            )
            .await;
        };
        let driver_commands = handle.commands.clone();
        let driver_interfaces = handle.interfaces.clone();
        let node_tasks = run_node_tasks(
            manifold,
            run_router::<St, R>(
                &state,
                &mut remote_control,
                req_rx,
                &mut remote_control_access_rx,
                handle.clone(),
            ),
            drive_interfaces(
                std::vec::Vec::new(),
                iface_build_rx,
                driver_commands,
                driver_interfaces,
            ),
        );
        match persistence_worker {
            None => {
                tokio::pin!(node_tasks, shutdown);
                tokio::select! {
                    biased;
                    result = &mut node_tasks => result,
                    () = &mut shutdown => Ok(()),
                }
            }
            Some(worker) => {
                let persistence = run_recipe_persistence(worker, shutdown, handle.commands.clone());
                tokio::pin!(node_tasks, persistence);
                tokio::select! {
                    biased;
                    result = &mut node_tasks => result,
                    result = &mut persistence => result,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
