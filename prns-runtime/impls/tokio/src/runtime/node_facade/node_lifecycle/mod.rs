use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use futures_util::FutureExt;
use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::engine::{
    CommandId, EngineCommand, EstablishLinkFailure, Journaled, LinkEstablished,
    PacketReceiptDelivered, SendSinglePacketFailure, SetTransportIdentityError,
};
use crate::identity::held::HoldIdentityError;
use crate::identity::{IdentityHash, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::InterfaceId;
use crate::manifold::compression;
use crate::manifold::driver::{
    self as manifold_driver, CryptoPoolConfig, Egress, HostCommand, ProvideDecompressedHostCommand,
    TokioHost,
};
use crate::routing::announce::AnnounceObservation;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestHandlerError, RequestPolicy};
use crate::storage::{StorageLayout, TablePushError};
use crate::units::RttMillis;
use crate::wire::DestinationHash;

use prns_runtime::runtime::{
    assemble_node, configure_preconfigured_destination, AssembledNode,
    ConfigurePreconfiguredDestinationError,
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

/// A node on the tokio host. Built from a [`PrnsNodeRecipe`] with [`new`](Self::new) (synchronous: it wires the engine and spawns each interface), then driven by [`run`](Self::run). Hold [`handle`](Self::handle) clones to drive it from other tasks/threads while `run` owns the loop.
pub struct PrnsNode<St, R, F, S: StorageLayout> {
    handle: PrnsNodeHandle,
    pub(super) host: TokioHost,
    pub(super) node: AssembledNode<St, R, F, S>,
    notify_rx: UnboundedReceiver<InterfaceId>,
    command_rx: UnboundedReceiver<HostCommand>,
    iface_build_rx: UnboundedReceiver<DriverMsg>,
    accepted_announce_observer: Option<AcceptedAnnounceObserver>,
    pub(super) crypto_pool: CryptoPoolConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonRoutingIdentityError {
    Hold(HoldIdentityError),
    Configure(SetTransportIdentityError),
}

pub type SharedInstanceIdentityError = NonRoutingIdentityError;

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
}

impl fmt::Display for NodeRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManifoldPanicked => formatter.write_str("the node manifold panicked"),
            Self::RequestEndpointrPanicked => formatter.write_str("the request router panicked"),
            Self::InterfaceDriverPanicked => formatter.write_str("the interface driver panicked"),
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

impl<St, R, F, S: StorageLayout> PrnsNode<St, R, F, S>
where
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
{
    /// Stand a node up from `recipe` on the storage layout it names: assemble the engine (transport role, destinations, the request endpoints), then let the recipe's `interfaces` intent attach the node's edges through its own handle. Only [`run`](Self::run) awaits.
    pub fn new<'a, D, I>(recipe: PrnsNodeRecipe<D, St, R, F, I, S>) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'a>>,
        I: AttachIntent,
    {
        Self::new_with_handle(|_| recipe)
    }

    pub fn new_with_handle<'a, D, I, B>(build_recipe: B) -> Self
    where
        D: IntoIterator<Item = PreConfiguredDestination<'a>>,
        I: AttachIntent,
        B: FnOnce(PrnsNodeHandle) -> PrnsNodeRecipe<D, St, R, F, I, S>,
    {
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (iface_build_tx, iface_build_rx) = mpsc::unbounded_channel();

        let handle = PrnsNodeHandle {
            commands: command_tx,
            ids: Arc::new(AtomicU64::new(0)),
            notify_tx,
            iface_build: iface_build_tx,
            interfaces: Arc::new(Mutex::new(HashMap::new())),
            store: InterfaceStore::new(),
            resource_admission: super::resource_admission::ResourceAdmissionRegistry::default(),
            entropy: crate::manifold::driver::TokioEntropy,
        };
        let (node, interfaces) = assemble_node(build_recipe(handle.clone()));
        interfaces.attach(&handle);

        PrnsNode {
            handle,
            host: TokioHost::start_at(persistence::wall_clock_timeline_origin()),
            node,
            notify_rx,
            command_rx,
            iface_build_rx,
            accepted_announce_observer: None,
            crypto_pool: CryptoPoolConfig::host_default(),
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

    /// A `Send + Clone` handle for other tasks/threads to drive the node while [`run`](Self::run) owns the loop.
    #[must_use]
    pub fn handle(&self) -> PrnsNodeHandle {
        self.handle.clone()
    }

    pub fn issue(&self, command: EngineCommand) -> Option<CommandId> {
        self.handle.issue(command)
    }

    pub async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        self.handle.send_single_packet(destination, data).await
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

    /// Drive the node until it stops (in practice forever). The manifold and the request runner run joined: every inbound request forks to the runner, while that event, and every other, reaches the recipe's `on_event` with shared `&state`, zero-copy.
    pub async fn run(self) -> Result<(), NodeRunError> {
        let PrnsNode {
            handle,
            host,
            node,
            notify_rx,
            command_rx,
            iface_build_rx,
            mut accepted_announce_observer,
            crypto_pool,
        } = self;
        let AssembledNode {
            engine,
            state,
            mut on_event,
            request_endpoints: _,
        } = node;
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
        let manifold = manifold_driver::run_with_store_and_deciders(
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
                should_prove: |_| false,
                should_accept_resource: move |offer| admission_decider.permits(offer),
            },
        );
        let driver_commands = handle.commands.clone();
        let driver_interfaces = handle.interfaces.clone();
        run_node_tasks(
            manifold,
            run_router::<St, R>(&state, req_rx, handle),
            drive_interfaces(
                std::vec::Vec::new(),
                iface_build_rx,
                driver_commands,
                driver_interfaces,
            ),
        )
        .await
    }
}

#[cfg(test)]
mod tests;
