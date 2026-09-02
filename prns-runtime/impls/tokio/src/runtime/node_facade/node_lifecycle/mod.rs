use std::collections::HashMap;
use std::fmt;
use std::future::{poll_fn, Future};
use std::marker::PhantomData;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use futures_util::task::AtomicWaker;
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
use crate::manifold::driver::{
    self as manifold_driver, CryptoPoolConfig, Egress, HostCommand, TokioClock, TokioHost,
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

use super::super::remote_control_controller_grants::{
    remote_control_controller_grant_lane, RemoteControlControllerGrantReceiver,
};
use super::super::remote_control_pairing_persistence::remote_control_pairing_persistence_lane;
use super::super::remote_control_pairing_persistence::RemoteControlAuthorizationPersistenceFailure;
use super::super::remote_control_target_accesses::{
    remote_control_target_access_lane, RemoteControlTargetAccessReceiver,
};
use super::super::request_endpoints::{RequestEndpoint, RequestEndpointSet, RespondToken};
use super::super::request_runner::{
    run_router, RemoteControlAuthorizationRuntime, RunnerRequest, REQUEST_QUEUE_DEPTH,
};
use super::super::{
    InterfaceStore, Message, PreConfiguredDestination, PrnsEvent, PrnsNodeRecipe, SendError,
};
use super::interface_lifecycle::{drive_interfaces, DriverMsg};
use super::{persistence, AttachIntent, PrnsNodeHandle, PrnsNodeLocalHandle};

const LOCAL_COMMAND_DEPTH: usize = 128;

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
    local_commands: Option<manifold_driver::LocalCommandProducer>,
    pub(super) host: TokioHost,
    pub(super) node: AssembledNode<St, R, F, S>,
    notify_rx: UnboundedReceiver<InterfaceId>,
    command_rx: UnboundedReceiver<HostCommand>,
    local_command_rx: manifold_driver::LocalCommandConsumer,
    remote_control_controller_grants_rx: RemoteControlControllerGrantReceiver,
    remote_control_target_accesses_rx: RemoteControlTargetAccessReceiver,
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
    RemoteControlAuthorizationPersistenceFailed(RemoteControlAuthorizationPersistenceFailure),
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
            Self::RemoteControlAuthorizationPersistenceFailed(failure) => write!(
                formatter,
                "remote-control authorization persistence could not be reconciled: {failure}"
            ),
        }
    }
}

impl std::error::Error for NodeRunError {}

/// A zero-sized ownership marker that keeps the whole node execution unit on the executor thread
/// which polls it. The engine lives inside `manifold`, while the interface and request futures are
/// sibling children of the same outer future. Making that outer future deliberately `!Send` locks
/// in their co-location without introducing a dedicated runtime, thread, queue, or wake boundary.
struct ExecutorThreadAnchor(PhantomData<Rc<()>>);

impl ExecutorThreadAnchor {
    const fn new() -> Self {
        Self(PhantomData)
    }

    fn finish<T>(self, result: T) -> T {
        result
    }
}

struct RequestTaskWake {
    ready: AtomicBool,
    polling: AtomicBool,
    parent: AtomicWaker,
}

impl RequestTaskWake {
    fn new() -> Self {
        Self {
            ready: AtomicBool::new(true),
            polling: AtomicBool::new(false),
            parent: AtomicWaker::new(),
        }
    }

    fn take_ready(&self) -> bool {
        self.ready.swap(false, Ordering::AcqRel)
    }

    fn finish_poll(&self) {
        self.polling.store(false, Ordering::Release);
        if self.ready.load(Ordering::Acquire) {
            self.parent.wake();
        }
    }
}

impl Wake for RequestTaskWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if !self.ready.swap(true, Ordering::AcqRel) && !self.polling.load(Ordering::Acquire) {
            self.parent.wake();
        }
    }
}

async fn run_executor_local_node_tasks(
    manifold: impl Future<Output = ()>,
    request_endpoints: impl Future<Output = Result<(), RemoteControlAuthorizationPersistenceFailure>>,
    interface_driver: impl Future<Output = ()>,
) -> Result<(), NodeRunError> {
    // This marker is consumed after the await so the returned future remains `!Send` even if all
    // of its child futures become `Send` in a future refactor.
    let executor_thread = ExecutorThreadAnchor::new();
    let manifold = AssertUnwindSafe(manifold).catch_unwind();
    let request_endpoints = AssertUnwindSafe(request_endpoints).catch_unwind();
    let interface_driver = AssertUnwindSafe(interface_driver).catch_unwind();
    tokio::pin!(manifold, request_endpoints, interface_driver);
    let request_wake = Arc::new(RequestTaskWake::new());
    let request_waker = Waker::from(request_wake.clone());
    // Interface I/O and manifold work form one latency-sensitive pipeline, so every executor turn
    // advances both directly with Tokio's waker. The request runner is independent and commonly
    // dormant; its tagged waker keeps unrelated hot-pipeline wakes from polling it.
    let mut interface_first = true;
    let result = poll_fn(|parent_context| {
        request_wake.parent.register(parent_context.waker());
        request_wake.polling.store(true, Ordering::Release);
        let mut request_ready = request_wake.take_ready();

        macro_rules! poll_child {
            ($child:ident, $context:expr, $panic:expr) => {
                if let Poll::Ready(result) = $child.as_mut().poll($context) {
                    request_wake.polling.store(false, Ordering::Release);
                    return Poll::Ready(result.map_err(|_| $panic));
                }
            };
        }

        if interface_first {
            poll_child!(
                interface_driver,
                parent_context,
                NodeRunError::InterfaceDriverPanicked
            );
            request_ready |= request_wake.take_ready();
            poll_child!(manifold, parent_context, NodeRunError::ManifoldPanicked);
        } else {
            poll_child!(manifold, parent_context, NodeRunError::ManifoldPanicked);
            request_ready |= request_wake.take_ready();
            poll_child!(
                interface_driver,
                parent_context,
                NodeRunError::InterfaceDriverPanicked
            );
        }
        interface_first = !interface_first;
        request_ready |= request_wake.take_ready();
        if request_ready {
            let mut request_context = Context::from_waker(&request_waker);
            if let Poll::Ready(result) = request_endpoints.as_mut().poll(&mut request_context) {
                request_wake.polling.store(false, Ordering::Release);
                return Poll::Ready(match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(failure)) => Err(
                        NodeRunError::RemoteControlAuthorizationPersistenceFailed(failure),
                    ),
                    Err(_) => Err(NodeRunError::RequestEndpointrPanicked),
                });
            }
        }
        request_wake.finish_poll();
        Poll::Pending
    })
    .await;
    executor_thread.finish(result)
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
        let (local_commands, local_command_rx) =
            manifold_driver::local_command_lane(LOCAL_COMMAND_DEPTH);
        let (iface_build_tx, iface_build_rx) = mpsc::unbounded_channel();
        let (remote_control_controller_grants, remote_control_controller_grants_rx) =
            remote_control_controller_grant_lane();
        let (remote_control_target_accesses, remote_control_target_accesses_rx) =
            remote_control_target_access_lane();

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
            remote_control_controller_grants,
            remote_control_target_accesses,
        };
        let (node, interfaces, persistence_intent) = assemble_node(build_recipe(handle.clone()));
        let node_persistence =
            persistence::PersistenceIntent::into_node_persistence(persistence_intent);
        interfaces.attach(&handle);

        PrnsNode {
            local_commands: Some(local_commands),
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
            local_command_rx,
            remote_control_controller_grants_rx,
            remote_control_target_accesses_rx,
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
    pub fn clock(&self) -> TokioClock {
        self.host.clock()
    }

    /// Override how this node runs its asymmetric crypto. Defaults to `CryptoPoolConfig::host_default` (pooled on capable hosts, inline on mobile).
    #[must_use]
    pub fn with_crypto_pool(mut self, crypto_pool: CryptoPoolConfig) -> Self {
        self.crypto_pool = crypto_pool;
        self
    }

    /// A `Send + Clone` handle for other tasks or threads to drive the node while
    /// [`run`](Self::run) or [`run_until`](Self::run_until) owns the executor-local loop. The run
    /// future deliberately keeps its engine, manifold, interface driver, and request runner on one
    /// executor thread; only this handle and explicit worker jobs cross thread boundaries.
    #[must_use]
    pub fn handle(&self) -> PrnsNodeHandle {
        self.handle.clone()
    }

    /// Takes the node's one executor-local command producer.
    ///
    /// The returned handle must remain in the same task as [`run`](Self::run) or
    /// [`run_until`](Self::run_until). A node exposes exactly one so the lane remains SPSC.
    pub fn take_local_handle(&mut self) -> Option<PrnsNodeLocalHandle> {
        self.local_commands
            .take()
            .map(|commands| PrnsNodeLocalHandle {
                commands: std::cell::RefCell::new(commands),
                ids: self.handle.ids.clone(),
                next_id: std::cell::Cell::new(0),
                end_id: std::cell::Cell::new(0),
                local: std::marker::PhantomData,
            })
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
            local_commands: _,
            host,
            node,
            notify_rx,
            command_rx,
            local_command_rx,
            mut remote_control_controller_grants_rx,
            mut remote_control_target_accesses_rx,
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
        let (save_on_learn, persistence_worker, authorization_persistence, restore_diagnostic) =
            match restored {
                Some((node_persistence, report)) => {
                    let (save_on_learn, wiring) = persistence::SaveOnLearn::channel();
                    let worker = node_persistence
                        .worker(handle.clone())
                        .with_save_on_learn(wiring)
                        .with_flush_failure_policy(persistence::FlushFailurePolicy::Exit);
                    let authorization_persistence =
                        worker.remote_control_authorization_persistence();
                    (
                        Some(save_on_learn),
                        Some(worker),
                        Some(authorization_persistence),
                        Some(persistence_restored_diagnostic(&report)),
                    )
                }
                None => (None, None, None, None),
            };
        let (remote_control_pairing_persistence, mut remote_control_pairing_persistence_rx) =
            remote_control_pairing_persistence_lane();
        let egress = Egress::new(std::vec::Vec::new());
        let store = handle.store.clone();
        let (req_tx, req_rx) = mpsc::channel(REQUEST_QUEUE_DEPTH);
        let admission_decider = handle.resource_admission.clone();
        let admission_cleanup = handle.resource_admission.clone();
        let manifold = async {
            if let Some(diagnostic) = restore_diagnostic {
                let event = PrnsEvent::Diagnostic(diagnostic);
                #[cfg(feature = "tracing")]
                super::super::tracing_events::emit(&event);
                on_event(event, &state);
            }
            manifold_driver::run_executor_local_with_store_and_deciders(
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
                local_command_rx,
                |journaled| {
                    remote_control_pairing_persistence.observe(&journaled);
                    if let Journaled::LinkClosed { link_id, .. } = &journaled {
                        admission_cleanup.remove(*link_id);
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
        let node_tasks = run_executor_local_node_tasks(
            manifold,
            run_router::<St, R>(
                &state,
                &mut remote_control,
                req_rx,
                RemoteControlAuthorizationRuntime {
                    controller_grants: &mut remote_control_controller_grants_rx,
                    target_accesses: &mut remote_control_target_accesses_rx,
                    pairing_persistence: &mut remote_control_pairing_persistence_rx,
                    persistence: authorization_persistence.as_ref(),
                },
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
