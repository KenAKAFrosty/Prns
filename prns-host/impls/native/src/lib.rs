#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use personal_rns::engine::{
    AllowRequester, AllowRequesterFailure, AllowRequesterRejection, AnnounceAppData, AnnounceNow,
    AnnounceNowFailure, AnnounceNowRejection, AnnounceTarget,
    DeliveryEvidence as EngineDeliveryEvidence, DeliveryProof, EstablishLinkFailure,
    EstablishLinkRejection, IdentifyFailure, IdentifyRejection,
    LinkClosedReason as EngineLinkClosedReason, RequestPathFailure, RequestResponseTimeout,
    RouteRemovalCause, SendRequestFailure, SendRequestRejection, SendResourceFailure,
    SendResourceRejection, SendSinglePacketFailure, SendSinglePacketRejection,
    SendToChannelFailure, SendToChannelRejection, SendToLinkFailure, SendToLinkRejection,
    SetResourceStrategyFailure, SetResourceStrategyRejection,
};
use personal_rns::interfaces::BitrateBps;
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::channel::MessageType;
use personal_rns::routing::request_handlers::RequestPolicy as EngineRequestPolicy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_router::RespondToken;
use personal_rns::runtime::{
    Diagnostic, Message, PrnsEvent, RequestPathError, ResourceSendError, SegmentCompression,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::{TcpClientInterface, TcpServer};
use personal_rns::udp::UdpInterface;
use personal_rns::units::{DurationMillis, RttMillis};
use personal_rns::{
    load_or_create_identity_secret, routes, try_generate_identity_secret, AttachedInterface,
    AttachedSupervisor, Manual, PreConfiguredDestination, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe,
    RatchetPolicy, ResourceStrategy as EngineResourceStrategy, SendError, Zeroizing,
    IDENTITY_SECRET_KEY_LEN,
};
use prns_host::{
    ApplicationEvent, Bitrate, Capability, ChannelMessage, CommandFailure, CommandOutcome,
    DeliveryEvidence, DestinationConfig, DestinationHash, DestinationIdentityConfig,
    DiagnosticEvent, HostCommand, HostConfig, HostRole, IdentityConfig, IdentityHash, InterfaceId,
    LinkClosedReason, LinkId, PacketHash, RequestAvailable, RequestHandlerConfig, RequestId,
    RequestPathHash, RequestPolicy, ResourceAvailable, ResourceCompression, ResourceHash,
    ResourceNeedsDecompression, ResourceSegmentAvailable, ResourceStrategy, ResourceStreamId,
    ResponseAvailable, ResponseSegmentAvailable, ResponseTimeout, SingleDelivery,
};
use tokio::sync::{mpsc, watch};

const START_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const AUTO_BITRATE_BPS: u64 = 65_000_000;

static THREAD_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static RESOURCE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub trait NativeEventSink: Send + Sync + 'static {
    fn running(&self);
    fn publish_application(&self, event: ApplicationEvent) -> bool;
    fn publish_resource(&self, event: ResourceAvailable, body: Vec<u8>) -> bool;
    fn publish_diagnostic(&self, event: DiagnosticEvent);
    fn stopped(&self);
    fn failed(&self, detail: String);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeStartError {
    MissingCapabilities(Vec<Capability>),
    Identity(String),
    Destination(String),
    Runtime(String),
    Thread(String),
    TimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeSubmitError {
    Busy,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandWait {
    Completed(Result<CommandOutcome, CommandFailure>),
    TimedOut,
    Interrupted,
}

struct CompletionState {
    result: Option<Result<CommandOutcome, CommandFailure>>,
    interrupted: bool,
}

struct CommandCompletion {
    state: Mutex<CompletionState>,
    ready: Condvar,
}

impl CommandCompletion {
    fn new() -> Self {
        Self {
            state: Mutex::new(CompletionState {
                result: None,
                interrupted: false,
            }),
            ready: Condvar::new(),
        }
    }

    fn finish(&self, result: Result<CommandOutcome, CommandFailure>) {
        let mut state = lock(&self.state);
        if state.result.is_none() {
            state.result = Some(result);
        }
        drop(state);
        self.ready.notify_all();
    }
}

#[derive(Clone)]
pub struct CommandHandle {
    completion: Arc<CommandCompletion>,
}

impl CommandHandle {
    #[must_use]
    pub fn wait(&self, timeout: Option<Duration>) -> CommandWait {
        let mut state = lock(&self.completion.state);
        if let Some(result) = state.result.clone() {
            return CommandWait::Completed(result);
        }
        if state.interrupted {
            return CommandWait::Interrupted;
        }
        match timeout {
            None => {
                while state.result.is_none() && !state.interrupted {
                    state = self
                        .completion
                        .ready
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
            Some(timeout) => {
                let waited = self
                    .completion
                    .ready
                    .wait_timeout_while(state, timeout, |state| {
                        state.result.is_none() && !state.interrupted
                    })
                    .unwrap_or_else(PoisonError::into_inner);
                state = waited.0;
                if waited.1.timed_out() && state.result.is_none() && !state.interrupted {
                    return CommandWait::TimedOut;
                }
            }
        }
        if let Some(result) = state.result.clone() {
            CommandWait::Completed(result)
        } else {
            CommandWait::Interrupted
        }
    }

    pub fn interrupt_wait(&self) {
        lock(&self.completion.state).interrupted = true;
        self.completion.ready.notify_all();
    }
}

struct CommandJob {
    command: HostCommand,
    completion: Arc<CommandCompletion>,
}

impl Drop for CommandJob {
    fn drop(&mut self) {
        self.completion.finish(Err(CommandFailure::NodeStopped));
    }
}

pub struct NativeHost {
    commands: mpsc::Sender<CommandJob>,
    shutdown: watch::Sender<bool>,
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
    stopped: AtomicBool,
    identity_hash: IdentityHash,
    destination_hashes: Vec<DestinationHash>,
}

impl NativeHost {
    pub fn start(
        config: HostConfig,
        sink: Arc<dyn NativeEventSink>,
    ) -> Result<Self, NativeStartError> {
        let missing: Vec<Capability> = config
            .required_capabilities
            .iter()
            .copied()
            .filter(|capability| !native_capabilities().contains(capability))
            .collect();
        if !missing.is_empty() {
            return Err(NativeStartError::MissingCapabilities(missing));
        }
        let pending_commands = config.limits.pending_commands();
        let (command_tx, command_rx) = mpsc::channel(pending_commands);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let sequence = THREAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let join = std::thread::Builder::new()
            .name(format!("prns-host-{sequence}"))
            .spawn(move || worker(config, sink, command_rx, shutdown_rx, ready_tx))
            .map_err(|error| NativeStartError::Thread(error.to_string()))?;
        let ready = match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(ready) => ready,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let _ = shutdown_tx.send(true);
                let _ = join.join();
                return Err(NativeStartError::TimedOut);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = join.join();
                return Err(NativeStartError::Runtime(
                    "native host exited during startup".to_string(),
                ));
            }
        }?;
        Ok(Self {
            commands: command_tx,
            shutdown: shutdown_tx,
            join: Mutex::new(Some(join)),
            stopped: AtomicBool::new(false),
            identity_hash: ready.identity_hash,
            destination_hashes: ready.destination_hashes,
        })
    }

    #[must_use]
    pub const fn identity_hash(&self) -> IdentityHash {
        self.identity_hash
    }

    #[must_use]
    pub fn destination_hashes(&self) -> &[DestinationHash] {
        &self.destination_hashes
    }

    pub fn submit(&self, command: HostCommand) -> Result<CommandHandle, NativeSubmitError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(NativeSubmitError::Stopped);
        }
        let completion = Arc::new(CommandCompletion::new());
        let job = CommandJob {
            command,
            completion: Arc::clone(&completion),
        };
        self.commands.try_send(job).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => NativeSubmitError::Busy,
            mpsc::error::TrySendError::Closed(_) => NativeSubmitError::Stopped,
        })?;
        Ok(CommandHandle { completion })
    }

    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.shutdown.send(true);
        let join = lock(&self.join).take();
        if let Some(join) = join {
            let _ = join.join();
        }
    }
}

impl Drop for NativeHost {
    fn drop(&mut self) {
        self.stop();
    }
}

#[must_use]
pub fn native_capabilities() -> &'static [Capability] {
    &[
        Capability::TcpClient,
        Capability::TcpServer,
        Capability::Udp,
    ]
}

struct Ready {
    identity_hash: IdentityHash,
    destination_hashes: Vec<DestinationHash>,
}

struct ResolvedSingle {
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    announce_app_data: Vec<u8>,
    request_handlers: Vec<RequestHandlerConfig>,
}

struct ResolvedDestination {
    app_name: String,
    aspects: Vec<String>,
    single: Option<ResolvedSingle>,
}

struct ResolvedConfig {
    host_identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    role: HostRole,
    destinations: Vec<ResolvedDestination>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn resolve_identity(
    identity: IdentityConfig,
) -> Result<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>, NativeStartError> {
    match identity {
        IdentityConfig::Existing(secret) => Ok(Zeroizing::new(*secret.expose())),
        IdentityConfig::GenerateEphemeral => try_generate_identity_secret()
            .map_err(|error| NativeStartError::Identity(error.to_string())),
        IdentityConfig::LoadOrCreate { path } => load_or_create_identity_secret(Path::new(&path))
            .map_err(|error| NativeStartError::Identity(format!("{path}: {error:?}"))),
    }
}

fn resolve_config(config: HostConfig) -> Result<ResolvedConfig, NativeStartError> {
    let host_identity = resolve_identity(config.identity)?;
    let mut destinations = Vec::with_capacity(config.destinations.len());
    for destination in config.destinations {
        match destination {
            DestinationConfig::Plain(name) => destinations.push(ResolvedDestination {
                app_name: name.app_name().to_string(),
                aspects: name.aspects().to_vec(),
                single: None,
            }),
            DestinationConfig::Single(single) => {
                let identity = match single.identity {
                    DestinationIdentityConfig::HostIdentity => host_identity.clone(),
                    DestinationIdentityConfig::Dedicated(identity) => resolve_identity(identity)?,
                };
                destinations.push(ResolvedDestination {
                    app_name: single.name.app_name().to_string(),
                    aspects: single.name.aspects().to_vec(),
                    single: Some(ResolvedSingle {
                        identity,
                        announce_app_data: single.announce_app_data,
                        request_handlers: single.request_handlers,
                    }),
                });
            }
        }
    }
    Ok(ResolvedConfig {
        host_identity,
        role: config.role,
        destinations,
    })
}

fn aspect_refs(config: &ResolvedConfig) -> Vec<Vec<&str>> {
    config
        .destinations
        .iter()
        .map(|destination| destination.aspects.iter().map(String::as_str).collect())
        .collect()
}

fn build_destinations<'a>(
    config: &'a ResolvedConfig,
    aspects: &'a [Vec<&'a str>],
) -> Vec<PreConfiguredDestination<'a>> {
    config
        .destinations
        .iter()
        .zip(aspects)
        .map(|(destination, aspects)| match &destination.single {
            None => PreConfiguredDestination::Plain {
                app_name: &destination.app_name,
                aspects,
            },
            Some(single) => PreConfiguredDestination::Single {
                app_name: &destination.app_name,
                aspects,
                identity: single.identity.clone(),
                announce_app_data: &single.announce_app_data,
                proof: ProofStrategy::ProveAll,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: EngineResourceStrategy::AcceptNone,
                request_handlers: personal_rns::runtime::RequestHandlerRegistration::None,
            },
        })
        .collect()
}

fn worker(
    config: HostConfig,
    sink: Arc<dyn NativeEventSink>,
    command_rx: mpsc::Receiver<CommandJob>,
    shutdown_rx: watch::Receiver<bool>,
    ready_tx: std::sync::mpsc::SyncSender<Result<Ready, NativeStartError>>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let failure = NativeStartError::Runtime(error.to_string());
            let _ = ready_tx.send(Err(failure.clone()));
            sink.failed(format!("{failure:?}"));
            return;
        }
    };
    runtime.block_on(run(config, sink, command_rx, shutdown_rx, ready_tx));
    runtime.shutdown_timeout(STOP_TIMEOUT);
}

async fn run(
    config: HostConfig,
    sink: Arc<dyn NativeEventSink>,
    command_rx: mpsc::Receiver<CommandJob>,
    shutdown_rx: watch::Receiver<bool>,
    ready_tx: std::sync::mpsc::SyncSender<Result<Ready, NativeStartError>>,
) {
    let resolved = match resolve_config(config) {
        Ok(config) => config,
        Err(error) => {
            let _ = ready_tx.send(Err(error.clone()));
            sink.failed(format!("{error:?}"));
            return;
        }
    };
    let identity =
        personal_rns::identity::PrivateIdentityMaterial::from_slice(&resolved.host_identity[..])
            .map(|material| IdentityHash::new(*material.identity_hash().as_bytes()))
            .map_err(|error| NativeStartError::Identity(format!("{error:?}")));
    let identity_hash = match identity {
        Ok(identity) => identity,
        Err(error) => {
            let _ = ready_tx.send(Err(error.clone()));
            sink.failed(format!("{error:?}"));
            return;
        }
    };
    let aspects = aspect_refs(&resolved);
    let destinations = build_destinations(&resolved, &aspects);
    let mut destination_hashes = Vec::with_capacity(destinations.len());
    for destination in &destinations {
        match destination.destination_hash() {
            Ok(hash) => destination_hashes.push(DestinationHash::new(*hash.as_bytes())),
            Err(error) => {
                let failure = NativeStartError::Destination(format!("{error:?}"));
                let _ = ready_tx.send(Err(failure.clone()));
                sink.failed(format!("{failure:?}"));
                return;
            }
        }
    }
    let event_sink = Arc::clone(&sink);
    let backpressure = Arc::new(tokio::sync::Notify::new());
    let event_backpressure = Arc::clone(&backpressure);
    let mut node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: match resolved.role {
            HostRole::Endpoint => None,
            HostRole::Transport => Some(resolved.host_identity.clone()),
        },
        pre_configured_destinations: destinations,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
        on_event: move |event, _state: &()| {
            if !publish_event(event_sink.as_ref(), event) {
                event_backpressure.notify_waiters();
            }
        },
    });
    for (destination, hash) in resolved.destinations.iter().zip(&destination_hashes) {
        let Some(single) = &destination.single else {
            continue;
        };
        for handler in &single.request_handlers {
            if let Err(error) = node.register_request_path(
                &engine_destination(*hash),
                &handler.path,
                engine_request_policy(handler.policy),
            ) {
                let failure = NativeStartError::Destination(format!(
                    "request path {:?}: {error:?}",
                    handler.path
                ));
                let _ = ready_tx.send(Err(failure.clone()));
                sink.failed(format!("{failure:?}"));
                return;
            }
        }
    }
    let handle = node.handle();
    sink.running();
    if ready_tx
        .send(Ok(Ready {
            identity_hash,
            destination_hashes,
        }))
        .is_err()
    {
        return;
    }
    let commands = tokio::spawn(command_loop(command_rx, handle, shutdown_rx.clone()));
    let exit = tokio::select! {
        result = node.run() => result.map_err(|error| format!("{error}")),
        result = commands => result.map_err(|error| error.to_string()),
        () = backpressure.notified() => Err("application event backpressure exceeded".to_string()),
    };
    match exit {
        Ok(()) => sink.stopped(),
        Err(detail) => sink.failed(detail),
    }
}

enum Attachment {
    Interface(AttachedInterface),
    Supervisor(AttachedSupervisor),
}

impl Attachment {
    fn teardown(self) {
        match self {
            Self::Interface(interface) => interface.teardown(),
            Self::Supervisor(supervisor) => supervisor.teardown(),
        }
    }
}

async fn command_loop(
    mut commands: mpsc::Receiver<CommandJob>,
    handle: PrnsNodeHandle,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut attachments = BTreeMap::new();
    loop {
        let job = tokio::select! {
            job = commands.recv() => match job {
                Some(job) => job,
                None => break,
            },
            _ = shutdown.wait_for(|stopping| *stopping) => break,
        };
        let result = execute_command(&handle, &mut attachments, &job.command).await;
        job.completion.finish(result);
    }
    for (_, attachment) in attachments {
        attachment.teardown();
    }
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
}

async fn execute_command(
    handle: &PrnsNodeHandle,
    attachments: &mut BTreeMap<InterfaceId, Attachment>,
    command: &HostCommand,
) -> Result<CommandOutcome, CommandFailure> {
    match command {
        HostCommand::Announce {
            destination,
            interface,
        } => {
            let target = match interface {
                Some(interface) => AnnounceTarget::Interface(engine_interface(*interface)),
                None => AnnounceTarget::AllInterfaces,
            };
            handle
                .announce_now(AnnounceNow {
                    destination: engine_destination(*destination),
                    target,
                    app_data: AnnounceAppData::Registered,
                })
                .await
                .map_err(announce_failure)?;
            Ok(CommandOutcome::Announced)
        }
        HostCommand::SendSinglePacket {
            destination,
            payload,
        } => {
            let receipt = handle
                .send_single_packet(engine_destination(*destination), payload)
                .await
                .map_err(send_failure)?;
            let evidence = match receipt.evidence {
                EngineDeliveryEvidence::Proof(DeliveryProof::Explicit(hash)) => {
                    DeliveryEvidence::ExplicitProof(PacketHash::new(*hash.as_bytes()))
                }
                EngineDeliveryEvidence::Proof(DeliveryProof::Implicit(hash)) => {
                    DeliveryEvidence::ImplicitProof(PacketHash::new(*hash.as_bytes()))
                }
                EngineDeliveryEvidence::Response => DeliveryEvidence::Response,
            };
            Ok(CommandOutcome::PacketDelivered {
                rtt_millis: receipt.rtt.millis(),
                evidence,
            })
        }
        HostCommand::CloseLink { link_id } => {
            if handle.close_link(engine_link(*link_id)) {
                Ok(CommandOutcome::LinkCloseQueued)
            } else {
                Err(CommandFailure::NodeStopped)
            }
        }
        HostCommand::AttachTcpServer { bind, bitrate } => {
            let server = TcpServer::bind(bind.as_str(), engine_bitrate(*bitrate)?)
                .await
                .map_err(|error| CommandFailure::BindFailed {
                    detail: error.to_string(),
                })?;
            let attached = handle.supervise(server);
            let interface = host_interface(attached.id());
            attachments.insert(interface, Attachment::Supervisor(attached));
            Ok(CommandOutcome::InterfaceAttached { interface })
        }
        HostCommand::AttachTcpClient { target, bitrate } => {
            let client = TcpClientInterface::new(
                target.clone(),
                engine_bitrate(*bitrate)?,
                ReconnectPolicy::STANDARD,
            );
            let attached = handle.add_interface(client);
            let interface = host_interface(attached.id());
            attachments.insert(interface, Attachment::Interface(attached));
            Ok(CommandOutcome::InterfaceAttached { interface })
        }
        HostCommand::AttachUdp {
            local,
            peer,
            bitrate,
        } => {
            let udp = UdpInterface::bind(local.as_str(), peer.as_str(), engine_bitrate(*bitrate)?)
                .await
                .map_err(|error| CommandFailure::BindFailed {
                    detail: error.to_string(),
                })?;
            let attached = handle.add_interface(udp);
            let interface = host_interface(attached.id());
            attachments.insert(interface, Attachment::Interface(attached));
            Ok(CommandOutcome::InterfaceAttached { interface })
        }
        HostCommand::DetachInterface { interface } => {
            let Some(attachment) = attachments.remove(interface) else {
                return Err(CommandFailure::UnknownInterface);
            };
            attachment.teardown();
            Ok(CommandOutcome::InterfaceDetached {
                interface: *interface,
            })
        }
        HostCommand::EstablishLink { destination } => {
            let established = handle
                .establish_link_with_rtt(engine_destination(*destination))
                .await
                .map_err(establish_link_failure)?;
            Ok(CommandOutcome::LinkEstablished {
                link_id: host_link(established.link_id),
                rtt_millis: established.rtt_millis,
            })
        }
        HostCommand::RequestPath { destination } => {
            let found = handle
                .request_path(engine_destination(*destination))
                .await
                .map_err(request_path_failure)?;
            Ok(CommandOutcome::PathDiscovered { hops: found.hops.0 })
        }
        HostCommand::Identify { link_id, identity } => {
            handle
                .identify(engine_link(*link_id), engine_identity(*identity))
                .await
                .map_err(identify_failure)?;
            Ok(CommandOutcome::Identified)
        }
        HostCommand::SendLinkPacket { link_id, payload } => {
            let receipt = handle
                .send_link_packet(engine_link(*link_id), payload)
                .await
                .map_err(send_link_failure)?;
            Ok(delivered_outcome(receipt))
        }
        HostCommand::Request {
            link_id,
            path_hash,
            payload,
            timeout,
        } => {
            let timeout = match timeout {
                ResponseTimeout::LinkDefault => RequestResponseTimeout::LinkDefault,
                ResponseTimeout::Exact { millis } => {
                    RequestResponseTimeout::Exact(DurationMillis(*millis))
                }
            };
            let (data, rtt) = handle
                .request_with_response_timeout(
                    engine_link(*link_id),
                    engine_request_path(*path_hash),
                    payload,
                    timeout,
                )
                .await
                .map_err(request_failure)?;
            Ok(CommandOutcome::ResponseReceived {
                data: unpack_binary(&data).unwrap_or(&data).to_vec(),
                rtt_millis: rtt.millis(),
            })
        }
        HostCommand::Respond {
            link_id,
            request_id,
            request_rtt_millis,
            payload,
        } => {
            let rtt = handle
                .respond_owned_bytes(
                    RespondToken {
                        link_id: engine_link(*link_id),
                        request_id: personal_rns::routing::links::request::RequestId(
                            request_id.into_bytes(),
                        ),
                        rtt: RttMillis::new(*request_rtt_millis),
                    },
                    payload.clone(),
                )
                .ok_or(CommandFailure::NodeStopped)?;
            Ok(CommandOutcome::ResponseSent {
                rtt_millis: rtt.millis(),
            })
        }
        HostCommand::SendResource {
            link_id,
            payload,
            packed_metadata,
            compression,
        } => {
            let total_len =
                u64::try_from(payload.len()).map_err(|_| CommandFailure::PayloadTooLarge)?;
            let source = std::io::Cursor::new(payload);
            let compression = match compression {
                ResourceCompression::Auto => SegmentCompression::AUTO,
                ResourceCompression::Never => SegmentCompression::Never,
            };
            let result = match packed_metadata {
                Some(metadata) => {
                    let (progress, _) = mpsc::unbounded_channel();
                    handle
                        .send_resource_with_options(
                            engine_link(*link_id),
                            total_len,
                            source,
                            metadata,
                            compression,
                            progress,
                        )
                        .await
                }
                None => {
                    handle
                        .send_resource_with_compression(
                            engine_link(*link_id),
                            total_len,
                            source,
                            compression,
                        )
                        .await
                }
            };
            result.map_err(resource_send_failure)?;
            Ok(CommandOutcome::ResourceSent)
        }
        HostCommand::SetLinkResourceStrategy { link_id, strategy } => {
            handle
                .set_link_resource_strategy(
                    engine_link(*link_id),
                    engine_resource_strategy(*strategy),
                )
                .await
                .map_err(set_resource_strategy_failure)?;
            Ok(CommandOutcome::ResourceStrategySet)
        }
        HostCommand::SetDestinationResourceStrategy {
            destination,
            strategy,
        } => {
            if !handle
                .set_resource_strategy(
                    engine_destination(*destination),
                    engine_resource_strategy(*strategy),
                )
                .await
            {
                return Err(CommandFailure::UnknownDestination);
            }
            Ok(CommandOutcome::ResourceStrategySet)
        }
        HostCommand::SendChannelMessage {
            link_id,
            message_type,
            payload,
        } => {
            let message_type = MessageType(*message_type);
            if message_type.is_system_reserved() {
                return Err(CommandFailure::InvalidChannelMessageType);
            }
            let receipt = handle
                .send_channel_message(engine_link(*link_id), message_type, payload)
                .await
                .map_err(send_channel_failure)?;
            Ok(delivered_outcome(receipt))
        }
        HostCommand::AllowRequester {
            destination,
            path_hash,
            identity,
        } => {
            handle
                .allow_requester(AllowRequester {
                    destination: engine_destination(*destination),
                    path_hash: engine_request_path(*path_hash),
                    identity: engine_identity(*identity),
                })
                .await
                .map_err(allow_requester_failure)?;
            Ok(CommandOutcome::RequesterAllowed)
        }
    }
}

fn engine_bitrate(bitrate: Bitrate) -> Result<BitrateBps, CommandFailure> {
    match bitrate {
        Bitrate::Auto => Ok(BitrateBps::guess(AUTO_BITRATE_BPS)),
        Bitrate::BitsPerSecond(value) => {
            BitrateBps::new(value).ok_or(CommandFailure::InvalidBitrate)
        }
    }
}

fn announce_failure(error: SendError<AnnounceNowFailure>) -> CommandFailure {
    match error {
        SendError::NodeStopped => CommandFailure::NodeStopped,
        SendError::Busy => CommandFailure::Busy,
        SendError::PayloadTooLarge => CommandFailure::AnnounceAppDataTooLong,
        SendError::Failed(AnnounceNowFailure::Rejected(rejection)) => match rejection {
            AnnounceNowRejection::UnknownDestination => CommandFailure::UnknownDestination,
            AnnounceNowRejection::NotASingleDestination => CommandFailure::NotSingleDestination,
            AnnounceNowRejection::AppDataTooLong => CommandFailure::AnnounceAppDataTooLong,
            AnnounceNowRejection::UnknownInterface => CommandFailure::UnknownInterface,
        },
        SendError::Failed(AnnounceNowFailure::WriteFailed(error)) => CommandFailure::WriteFailed {
            detail: format!("{error:?}"),
        },
    }
}

fn send_failure(error: SendError<SendSinglePacketFailure>) -> CommandFailure {
    match error {
        SendError::NodeStopped => CommandFailure::NodeStopped,
        SendError::Busy => CommandFailure::Busy,
        SendError::PayloadTooLarge => CommandFailure::PayloadTooLarge,
        SendError::Failed(SendSinglePacketFailure::Rejected(rejection)) => match rejection {
            SendSinglePacketRejection::NoRouteToDestination => CommandFailure::NoRouteToDestination,
            SendSinglePacketRejection::NotDirectlyReachable => CommandFailure::NotDirectlyReachable,
        },
        SendError::Failed(SendSinglePacketFailure::WriteFailed(error)) => {
            CommandFailure::WriteFailed {
                detail: format!("{error:?}"),
            }
        }
        SendError::Failed(SendSinglePacketFailure::Culled) => CommandFailure::PacketCulled,
        SendError::Failed(SendSinglePacketFailure::Timeout) => CommandFailure::DeliveryTimedOut,
    }
}

fn delivered_outcome(receipt: personal_rns::PacketReceiptDelivered) -> CommandOutcome {
    let evidence = match receipt.evidence {
        EngineDeliveryEvidence::Proof(DeliveryProof::Explicit(hash)) => {
            DeliveryEvidence::ExplicitProof(PacketHash::new(*hash.as_bytes()))
        }
        EngineDeliveryEvidence::Proof(DeliveryProof::Implicit(hash)) => {
            DeliveryEvidence::ImplicitProof(PacketHash::new(*hash.as_bytes()))
        }
        EngineDeliveryEvidence::Response => DeliveryEvidence::Response,
    };
    CommandOutcome::PacketDelivered {
        rtt_millis: receipt.rtt.millis(),
        evidence,
    }
}

fn establish_link_failure(error: SendError<EstablishLinkFailure>) -> CommandFailure {
    match error {
        SendError::NodeStopped => CommandFailure::NodeStopped,
        SendError::Busy => CommandFailure::Busy,
        SendError::PayloadTooLarge => CommandFailure::PayloadTooLarge,
        SendError::Failed(EstablishLinkFailure::Rejected(rejection)) => match rejection {
            EstablishLinkRejection::NoRouteToDestination => CommandFailure::NoRouteToDestination,
            EstablishLinkRejection::NotDirectlyReachable => CommandFailure::NotDirectlyReachable,
        },
        SendError::Failed(EstablishLinkFailure::WriteFailed(error)) => {
            CommandFailure::WriteFailed {
                detail: format!("{error:?}"),
            }
        }
        SendError::Failed(EstablishLinkFailure::Timeout) => CommandFailure::DeliveryTimedOut,
    }
}

fn request_path_failure(error: RequestPathError) -> CommandFailure {
    match error {
        RequestPathError::EntropyUnavailable => CommandFailure::EntropyUnavailable,
        RequestPathError::NodeStopped => CommandFailure::NodeStopped,
        RequestPathError::Failed(RequestPathFailure::Timeout) => CommandFailure::DeliveryTimedOut,
        RequestPathError::Failed(RequestPathFailure::Culled) => CommandFailure::PacketCulled,
        RequestPathError::Failed(RequestPathFailure::WriteFailed(error)) => {
            CommandFailure::WriteFailed {
                detail: format!("{error:?}"),
            }
        }
    }
}

fn identify_failure(error: SendError<IdentifyFailure>) -> CommandFailure {
    match error {
        SendError::NodeStopped => CommandFailure::NodeStopped,
        SendError::Busy => CommandFailure::Busy,
        SendError::PayloadTooLarge => CommandFailure::PayloadTooLarge,
        SendError::Failed(IdentifyFailure::Rejected(rejection)) => match rejection {
            IdentifyRejection::NoSuchLink => CommandFailure::UnknownLink,
            IdentifyRejection::LinkNotActive => CommandFailure::LinkNotActive,
            IdentifyRejection::NotInitiator => CommandFailure::NotLinkInitiator,
            IdentifyRejection::IdentityNotHeld => CommandFailure::IdentityNotHeld,
        },
        SendError::Failed(IdentifyFailure::WriteFailed) => CommandFailure::WriteFailed {
            detail: "identity write failed".to_string(),
        },
    }
}

fn send_link_failure(error: SendError<SendToLinkFailure>) -> CommandFailure {
    match error {
        SendError::NodeStopped => CommandFailure::NodeStopped,
        SendError::Busy => CommandFailure::Busy,
        SendError::PayloadTooLarge => CommandFailure::PayloadTooLarge,
        SendError::Failed(SendToLinkFailure::Rejected(rejection)) => match rejection {
            SendToLinkRejection::NoSuchLink => CommandFailure::UnknownLink,
            SendToLinkRejection::LinkNotActive => CommandFailure::LinkNotActive,
        },
        SendError::Failed(SendToLinkFailure::WriteFailed(error)) => CommandFailure::WriteFailed {
            detail: format!("{error:?}"),
        },
        SendError::Failed(SendToLinkFailure::Culled) => CommandFailure::PacketCulled,
        SendError::Failed(SendToLinkFailure::Timeout) => CommandFailure::DeliveryTimedOut,
    }
}

fn request_failure(error: SendError<SendRequestFailure>) -> CommandFailure {
    match error {
        SendError::NodeStopped => CommandFailure::NodeStopped,
        SendError::Busy => CommandFailure::Busy,
        SendError::PayloadTooLarge => CommandFailure::PayloadTooLarge,
        SendError::Failed(SendRequestFailure::Rejected(rejection)) => match rejection {
            SendRequestRejection::NoSuchLink => CommandFailure::UnknownLink,
            SendRequestRejection::LinkNotActive => CommandFailure::LinkNotActive,
        },
        SendError::Failed(SendRequestFailure::WriteFailed) => CommandFailure::WriteFailed {
            detail: "request write failed".to_string(),
        },
        SendError::Failed(SendRequestFailure::Culled) => CommandFailure::PacketCulled,
        SendError::Failed(SendRequestFailure::Timeout) => CommandFailure::DeliveryTimedOut,
    }
}

fn resource_send_failure(error: ResourceSendError) -> CommandFailure {
    match error {
        ResourceSendError::Source(error) => CommandFailure::WriteFailed {
            detail: error.to_string(),
        },
        ResourceSendError::UnrepresentableLength => CommandFailure::PayloadTooLarge,
        ResourceSendError::NodeStopped => CommandFailure::NodeStopped,
        ResourceSendError::Rejected(SendResourceFailure::Rejected(rejection)) => match rejection {
            SendResourceRejection::NoSuchLink => CommandFailure::UnknownLink,
            SendResourceRejection::LinkNotActive => CommandFailure::LinkNotActive,
            SendResourceRejection::LinkBusy => CommandFailure::LinkBusy,
            SendResourceRejection::TableFull => CommandFailure::ResourceTableFull,
            SendResourceRejection::Build(
                personal_rns::routing::links::resources::build_outgoing::BuildOutgoingResourceError::DataTooLarge,
            ) => CommandFailure::PayloadTooLarge,
            SendResourceRejection::Build(
                personal_rns::routing::links::resources::build_outgoing::BuildOutgoingResourceError::MetadataTooLarge,
            )
            | SendResourceRejection::MetadataMisplaced => {
                CommandFailure::ResourceMetadataTooLarge
            }
            SendResourceRejection::Build(error) => CommandFailure::WriteFailed {
                detail: format!("{error:?}"),
            },
        },
        ResourceSendError::Rejected(SendResourceFailure::WriteFailed) => {
            CommandFailure::WriteFailed {
                detail: "resource write failed".to_string(),
            }
        }
        ResourceSendError::Rejected(SendResourceFailure::RejectedByPeer) => {
            CommandFailure::ResourceRejectedByPeer
        }
        ResourceSendError::Rejected(SendResourceFailure::Sequencing) => {
            CommandFailure::ResourceSequencingFailed
        }
        ResourceSendError::Rejected(SendResourceFailure::Timeout) => {
            CommandFailure::DeliveryTimedOut
        }
        ResourceSendError::Rejected(SendResourceFailure::PredecessorFailed) => {
            CommandFailure::ResourcePredecessorFailed
        }
    }
}

fn set_resource_strategy_failure(error: SendError<SetResourceStrategyFailure>) -> CommandFailure {
    match error {
        SendError::NodeStopped => CommandFailure::NodeStopped,
        SendError::Busy => CommandFailure::Busy,
        SendError::PayloadTooLarge => CommandFailure::PayloadTooLarge,
        SendError::Failed(SetResourceStrategyFailure::Rejected(rejection)) => match rejection {
            SetResourceStrategyRejection::NoSuchLink => CommandFailure::UnknownLink,
            SetResourceStrategyRejection::LinkNotActive => CommandFailure::LinkNotActive,
        },
    }
}

fn send_channel_failure(error: SendError<SendToChannelFailure>) -> CommandFailure {
    match error {
        SendError::NodeStopped => CommandFailure::NodeStopped,
        SendError::Busy => CommandFailure::Busy,
        SendError::PayloadTooLarge => CommandFailure::PayloadTooLarge,
        SendError::Failed(SendToChannelFailure::Rejected(rejection)) => match rejection {
            SendToChannelRejection::NoSuchLink => CommandFailure::UnknownLink,
            SendToChannelRejection::LinkNotActive => CommandFailure::LinkNotActive,
        },
        SendError::Failed(SendToChannelFailure::WriteFailed(error)) => {
            CommandFailure::WriteFailed {
                detail: format!("{error:?}"),
            }
        }
        SendError::Failed(SendToChannelFailure::WindowFull) => CommandFailure::ChannelWindowFull,
        SendError::Failed(SendToChannelFailure::Untrackable) => CommandFailure::ChannelUntrackable,
        SendError::Failed(SendToChannelFailure::Timeout) => CommandFailure::DeliveryTimedOut,
    }
}

fn allow_requester_failure(error: SendError<AllowRequesterFailure>) -> CommandFailure {
    match error {
        SendError::NodeStopped => CommandFailure::NodeStopped,
        SendError::Busy => CommandFailure::Busy,
        SendError::PayloadTooLarge => CommandFailure::PayloadTooLarge,
        SendError::Failed(AllowRequesterFailure::Rejected(rejection)) => match rejection {
            AllowRequesterRejection::NoSuchHandler => CommandFailure::UnknownRequestHandler,
            AllowRequesterRejection::NoAllowList => CommandFailure::RequestPolicyNotAllowList,
            AllowRequesterRejection::AllowListFull => CommandFailure::RequestAllowListFull,
        },
    }
}

fn engine_resource_strategy(strategy: ResourceStrategy) -> EngineResourceStrategy {
    match strategy {
        ResourceStrategy::Refuse => EngineResourceStrategy::AcceptNone,
        ResourceStrategy::Accept {
            maximum_uncompressed_bytes,
            accept_compressed,
        } => EngineResourceStrategy::Accept {
            max_uncompressed_bytes: maximum_uncompressed_bytes,
            accept_compressed,
        },
    }
}

fn engine_request_policy(policy: RequestPolicy) -> EngineRequestPolicy {
    match policy {
        RequestPolicy::AllowNone => EngineRequestPolicy::AllowNone,
        RequestPolicy::AllowAll => EngineRequestPolicy::AllowAll,
        RequestPolicy::AllowList => EngineRequestPolicy::AllowList,
    }
}

fn unpack_binary(packed: &[u8]) -> Option<&[u8]> {
    match packed {
        [0xc4, len, rest @ ..] if rest.len() == *len as usize => Some(rest),
        [0xc5, a, b, rest @ ..] if rest.len() == u16::from_be_bytes([*a, *b]) as usize => {
            Some(rest)
        }
        [0xc6, a, b, c, d, rest @ ..]
            if u32::try_from(rest.len()) == Ok(u32::from_be_bytes([*a, *b, *c, *d])) =>
        {
            Some(rest)
        }
        _ => None,
    }
}

fn engine_destination(value: DestinationHash) -> personal_rns::wire::DestinationHash {
    personal_rns::wire::DestinationHash::new(value.into_bytes())
}

fn engine_identity(value: IdentityHash) -> personal_rns::identity::IdentityHash {
    personal_rns::identity::IdentityHash::new(value.into_bytes())
}

fn engine_request_path(
    value: RequestPathHash,
) -> personal_rns::routing::request_handlers::RequestPathHash {
    personal_rns::routing::request_handlers::RequestPathHash::new(value.into_bytes())
}

fn engine_interface(value: InterfaceId) -> personal_rns::interfaces::InterfaceId {
    personal_rns::interfaces::InterfaceId::new(value.into_bytes())
}

fn engine_link(value: LinkId) -> personal_rns::routing::links::LinkId {
    personal_rns::routing::links::LinkId::new(value.into_bytes())
}

fn host_destination(value: personal_rns::wire::DestinationHash) -> DestinationHash {
    DestinationHash::new(*value.as_bytes())
}

fn host_interface(value: personal_rns::interfaces::InterfaceId) -> InterfaceId {
    InterfaceId::new(*value.as_bytes())
}

fn host_link(value: personal_rns::routing::links::LinkId) -> LinkId {
    LinkId::new(*value.as_bytes())
}

fn host_resource_hash(
    value: personal_rns::routing::links::resources::ResourceHash,
) -> ResourceHash {
    ResourceHash::new(*value.as_bytes())
}

fn publish_event(sink: &dyn NativeEventSink, event: PrnsEvent<'_>) -> bool {
    match event {
        PrnsEvent::Message(message) => publish_message(sink, message),
        PrnsEvent::Diagnostic(diagnostic) => {
            sink.publish_diagnostic(translate_diagnostic(diagnostic));
            true
        }
    }
}

fn publish_message(sink: &dyn NativeEventSink, message: Message<'_>) -> bool {
    let event = match message {
        Message::Delivered(Delivery::Single(delivery)) => {
            ApplicationEvent::SingleDelivery(SingleDelivery {
                destination: host_destination(delivery.destination),
                source_interface: host_interface(delivery.source_interface),
                plaintext: delivery.plaintext.to_vec(),
            })
        }
        Message::Delivered(other) => {
            sink.publish_diagnostic(DiagnosticEvent::Delivered {
                detail: format!("{other:?}"),
            });
            return true;
        }
        Message::Request {
            destination,
            link_id,
            request_id,
            requester,
            path_hash,
            rtt,
            data,
            ..
        } => ApplicationEvent::Request(RequestAvailable {
            destination: host_destination(destination),
            link_id: host_link(link_id),
            request_id: RequestId::new(request_id.0),
            requester: requester.map(|identity| IdentityHash::new(*identity.as_bytes())),
            path_hash: RequestPathHash::new(*path_hash.as_bytes()),
            rtt_millis: rtt.millis(),
            data: data.to_vec(),
        }),
        Message::Response {
            link_id,
            request_id,
            data,
        } => ApplicationEvent::Response(ResponseAvailable {
            link_id: host_link(link_id),
            request_id: RequestId::new(request_id.0),
            data: data.to_vec(),
        }),
        Message::ResponseSegment {
            link_id,
            request_id,
            segment_index,
            total_segments,
            data,
        } => ApplicationEvent::ResponseSegment(ResponseSegmentAvailable {
            link_id: host_link(link_id),
            request_id: RequestId::new(request_id.0),
            segment_index,
            total_segments,
            data: data.to_vec(),
        }),
        Message::Resource {
            link_id,
            hash,
            metadata,
            data,
        } => {
            let stream_id =
                ResourceStreamId::new(RESOURCE_SEQUENCE.fetch_add(1, Ordering::Relaxed));
            return sink.publish_resource(
                ResourceAvailable {
                    stream_id,
                    link_id: host_link(link_id),
                    hash: host_resource_hash(hash),
                    metadata: metadata.map(<[u8]>::to_vec),
                    total_bytes: u64::try_from(data.len()).unwrap_or(u64::MAX),
                },
                data.to_vec(),
            );
        }
        Message::ResourceNeedsDecompression {
            link_id,
            hash,
            stream,
            uncompressed_data_bytes,
        } => ApplicationEvent::ResourceNeedsDecompression(ResourceNeedsDecompression {
            link_id: host_link(link_id),
            hash: host_resource_hash(hash),
            stream: stream.to_vec(),
            uncompressed_data_bytes,
        }),
        Message::ResourceSegment {
            link_id,
            original_hash,
            segment_index,
            total_segments,
            metadata,
            data,
        } => ApplicationEvent::ResourceSegment(ResourceSegmentAvailable {
            link_id: host_link(link_id),
            original_hash: host_resource_hash(original_hash),
            segment_index,
            total_segments,
            metadata: metadata.map(<[u8]>::to_vec),
            data: data.to_vec(),
        }),
        Message::ChannelMessage {
            link_id,
            message_type,
            data,
        } => ApplicationEvent::ChannelMessage(ChannelMessage {
            link_id: host_link(link_id),
            message_type: message_type.0,
            data: data.to_vec(),
        }),
    };
    sink.publish_application(event)
}

fn translate_diagnostic(diagnostic: Diagnostic) -> DiagnosticEvent {
    match diagnostic {
        Diagnostic::AnnounceHeard {
            destination,
            hops,
            source_interface,
        } => DiagnosticEvent::AnnounceHeard {
            destination: host_destination(destination),
            hops,
            source_interface: host_interface(source_interface),
        },
        Diagnostic::LinkEstablished(established) => DiagnosticEvent::LinkEstablished {
            link_id: host_link(established.link_id),
            rtt_millis: established.rtt_millis,
        },
        Diagnostic::PeerIdentified { link_id, identity } => DiagnosticEvent::PeerIdentified {
            link_id: host_link(link_id),
            identity: IdentityHash::new(*identity.as_bytes()),
        },
        Diagnostic::LinkClosed { link_id, reason } => DiagnosticEvent::LinkClosed {
            link_id: host_link(link_id),
            reason: match reason {
                EngineLinkClosedReason::Timeout => LinkClosedReason::Timeout,
                EngineLinkClosedReason::PeerClosed => LinkClosedReason::PeerClosed,
                EngineLinkClosedReason::MalformedRtt => LinkClosedReason::MalformedRtt,
            },
        },
        Diagnostic::CommandSettled { id, settlement } => DiagnosticEvent::BackendDiagnostic {
            kind: "commandSettled".to_string(),
            detail: format!("{}:{settlement:?}", id.0),
        },
        Diagnostic::SelfRatchetRotated { destination } => DiagnosticEvent::SelfRatchetRotated {
            destination: host_destination(destination),
        },
        Diagnostic::AnnounceHeldDropped {
            destination,
            source_interface,
            cause,
        } => DiagnosticEvent::AnnounceHeldDropped {
            destination: host_destination(destination),
            source_interface: host_interface(source_interface),
            cause: format!("{cause:?}"),
        },
        Diagnostic::LinkInterfaceMismatch {
            link_id,
            attached_interface,
            arrived_on,
        } => DiagnosticEvent::LinkInterfaceMismatch {
            link_id: host_link(link_id),
            attached_interface: host_interface(attached_interface),
            arrived_on: host_interface(arrived_on),
        },
        Diagnostic::ResourceAssembled {
            link_id,
            original_hash,
            total_size_bytes,
        } => DiagnosticEvent::ResourceAssembled {
            link_id: host_link(link_id),
            original_hash: host_resource_hash(original_hash),
            total_size_bytes,
        },
        Diagnostic::ResourceFailed {
            link_id,
            hash,
            cause,
        } => DiagnosticEvent::ResourceFailed {
            link_id: host_link(link_id),
            hash: host_resource_hash(hash),
            cause: format!("{cause:?}"),
        },
        Diagnostic::RouteRemoved { destination, cause } => match cause {
            RouteRemovalCause::Expired => DiagnosticEvent::RouteExpired {
                destination: host_destination(destination),
            },
            RouteRemovalCause::Evicted => DiagnosticEvent::RouteEvicted {
                destination: host_destination(destination),
            },
            RouteRemovalCause::InterfaceGone => DiagnosticEvent::RouteInterfaceGone {
                destination: host_destination(destination),
            },
            RouteRemovalCause::Dropped => DiagnosticEvent::RouteDropped {
                destination: host_destination(destination),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_host::{DestinationName, PrnsLimits, SingleDestinationConfig};

    struct Sink;

    impl NativeEventSink for Sink {
        fn running(&self) {}

        fn publish_application(&self, _event: ApplicationEvent) -> bool {
            true
        }

        fn publish_resource(&self, _event: ResourceAvailable, _body: Vec<u8>) -> bool {
            true
        }

        fn publish_diagnostic(&self, _event: DiagnosticEvent) {}

        fn stopped(&self) {}

        fn failed(&self, _detail: String) {}
    }

    fn config() -> HostConfig {
        HostConfig {
            identity: IdentityConfig::GenerateEphemeral,
            role: HostRole::Endpoint,
            destinations: Vec::new(),
            required_capabilities: Vec::new(),
            limits: PrnsLimits::balanced(),
        }
    }

    #[test]
    fn native_host_executes_interface_commands() -> Result<(), String> {
        let host =
            NativeHost::start(config(), Arc::new(Sink)).map_err(|error| format!("{error:?}"))?;
        let command = host
            .submit(HostCommand::AttachTcpClient {
                target: "127.0.0.1:9".to_string(),
                bitrate: Bitrate::Auto,
            })
            .map_err(|error| format!("{error:?}"))?;
        let attached = match command.wait(Some(Duration::from_secs(2))) {
            CommandWait::Completed(Ok(CommandOutcome::InterfaceAttached { interface })) => {
                interface
            }
            other => return Err(format!("{other:?}")),
        };
        let command = host
            .submit(HostCommand::DetachInterface {
                interface: attached,
            })
            .map_err(|error| format!("{error:?}"))?;
        if !matches!(
            command.wait(Some(Duration::from_secs(2))),
            CommandWait::Completed(Ok(CommandOutcome::InterfaceDetached { .. }))
        ) {
            return Err("interface did not detach".to_string());
        }
        let command = host
            .submit(HostCommand::SendChannelMessage {
                link_id: LinkId::new([0; 16]),
                message_type: 0xf000,
                payload: Vec::new(),
            })
            .map_err(|error| format!("{error:?}"))?;
        if !matches!(
            command.wait(Some(Duration::from_secs(2))),
            CommandWait::Completed(Err(CommandFailure::InvalidChannelMessageType))
        ) {
            return Err("reserved channel type did not settle as invalid".to_string());
        }
        host.stop();
        Ok(())
    }

    #[test]
    fn configured_host_registers_request_handlers() -> Result<(), String> {
        let mut config = config();
        config
            .destinations
            .push(DestinationConfig::Single(SingleDestinationConfig {
                name: DestinationName::try_new("host-test", ["request".to_string()])
                    .map_err(|error| format!("{error:?}"))?,
                identity: DestinationIdentityConfig::HostIdentity,
                announce_app_data: Vec::new(),
                request_handlers: vec![RequestHandlerConfig {
                    path: "/echo".to_string(),
                    policy: RequestPolicy::AllowList,
                }],
            }));
        let host =
            NativeHost::start(config, Arc::new(Sink)).map_err(|error| format!("{error:?}"))?;
        let destination = host.destination_hashes()[0];
        let engine_path = personal_rns::routing::request_handlers::RequestPathHash::of("/echo");
        let command = host
            .submit(HostCommand::AllowRequester {
                destination,
                path_hash: RequestPathHash::new(*engine_path.as_bytes()),
                identity: IdentityHash::new([0x44; 16]),
            })
            .map_err(|error| format!("{error:?}"))?;
        if !matches!(
            command.wait(Some(Duration::from_secs(2))),
            CommandWait::Completed(Ok(CommandOutcome::RequesterAllowed))
        ) {
            return Err("requester was not admitted".to_string());
        }
        host.stop();
        Ok(())
    }
}
