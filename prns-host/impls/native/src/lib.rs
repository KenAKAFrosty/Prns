#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use personal_rns::engine::{
    AllowRequester, AllowRequesterFailure, AllowRequesterRejection, AnnounceAppData, AnnounceNow,
    AnnounceNowFailure, AnnounceNowRejection, AnnounceTarget,
    DeliveryEvidence as EngineDeliveryEvidence, DeliveryProof, EstablishLinkFailure,
    EstablishLinkRejection, IdentifyFailure, IdentifyRejection,
    LinkClosedReason as EngineLinkClosedReason,
    PersistenceFlushCause as EnginePersistenceFlushCause,
    PersistenceFlushTarget as EnginePersistenceFlushTarget, RequestPathFailure,
    RequestResponseTimeout, RouteRemovalCause, SendRequestFailure, SendRequestRejection,
    SendResourceFailure, SendResourceRejection, SendSinglePacketFailure, SendSinglePacketRejection,
    SendToChannelFailure, SendToChannelRejection, SendToLinkFailure, SendToLinkRejection,
    SetResourceStrategyFailure, SetResourceStrategyRejection,
};
use personal_rns::interfaces::{BitrateBps, ConnectionState, InterfaceKind as EngineInterfaceKind};
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::node_introspection::logical_interface_inventory;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::channel::MessageType;
use personal_rns::routing::request_handlers::RequestPolicy as EngineRequestPolicy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::RespondToken;
use personal_rns::runtime::{
    Diagnostic, Message, NodePersistence, PersistenceIntent, PrnsEvent, RequestPathError,
    ResourceSendError, SegmentCompression,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::{TcpClientInterface, TcpServer};
use personal_rns::udp::UdpInterface;
use personal_rns::units::{DurationMillis, RttMillis};
use personal_rns::{
    load_or_create_identity_secret, request_endpoints, try_generate_identity_secret,
    AttachedInterface, AttachedSupervisor, IdentitySecretFileError, ManuallyAttached,
    PreConfiguredDestination, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe, RatchetPolicy,
    ResourceStrategy as EngineResourceStrategy, SendError, Zeroizing, IDENTITY_SECRET_KEY_LEN,
};
use prns_host::{
    ApplicationEvent, BackendInfo, BackendKind, Bitrate, Capability, ChannelMessage,
    CommandFailure, CommandOutcome, DeliveryEvidence, DestinationConfig, DestinationHash,
    DestinationIdentityConfig, DestinationIdentitySnapshot, DiagnosticEvent, HostCommand,
    HostConfig, HostRole, HostSnapshot, IdentityConfig, IdentityHash, InterfaceConfig,
    InterfaceHealth, InterfaceId, InterfaceKind, InterfaceSnapshot, LinkClosedReason, LinkId,
    PacketHash, PersistenceConfig, PersistenceFlushCause, PersistenceFlushTarget,
    PersistenceSnapshot, RequestAvailable, RequestHandlerConfig, RequestId, RequestPathHash,
    RequestPolicy, ResourceAvailable, ResourceCompression, ResourceHash,
    ResourceNeedsDecompression, ResourceSegmentAvailable, ResourceStrategy, ResourceStreamId,
    ResponseAvailable, ResponseSegmentAvailable, ResponseTimeout, RouteSnapshot,
    RuntimeHealthSnapshot, SingleDelivery,
};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::{mpsc, watch};

const START_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const AUTO_BITRATE_BPS: u64 = 65_000_000;
const UPLOAD_CHUNK_CAPACITY: usize = 4;
const MAX_UPLOAD_CHUNK_BYTES: usize = 256 * 1024;

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
    Identity(IdentityStartError),
    Destination(String),
    Persistence(PersistenceStartError),
    Runtime(String),
    Thread(String),
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityStartError {
    EntropyUnavailable,
    PermissionDenied { path: String },
    Malformed { path: String, length: u64 },
    Unavailable { path: String, detail: String },
    InvalidMaterial,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistenceStartError {
    PermissionDenied { path: String },
    NotDirectory { path: String },
    Unavailable { path: String, detail: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeSubmitError {
    Busy,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeSnapshotError {
    Busy,
    Stopped,
    TimedOut,
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

pub type CommandReadiness = Arc<dyn Fn() + Send + Sync>;

struct CommandCompletion {
    state: Mutex<CompletionState>,
    ready: Condvar,
    readiness: Option<CommandReadiness>,
}

impl CommandCompletion {
    fn new(readiness: Option<CommandReadiness>) -> Self {
        Self {
            state: Mutex::new(CompletionState {
                result: None,
                interrupted: false,
            }),
            ready: Condvar::new(),
            readiness,
        }
    }

    fn finish(&self, result: Result<CommandOutcome, CommandFailure>) {
        let mut state = lock(&self.state);
        if state.result.is_some() {
            return;
        }
        state.result = Some(result);
        drop(state);
        self.ready.notify_all();
        if let Some(readiness) = &self.readiness {
            readiness();
        }
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
        if let Some(readiness) = &self.completion.readiness {
            readiness();
        }
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
    uploads: mpsc::Sender<UploadJob>,
    snapshots: mpsc::Sender<SnapshotJob>,
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
        let (upload_tx, upload_rx) = mpsc::channel(pending_commands);
        let (snapshot_tx, snapshot_rx) = mpsc::channel(pending_commands);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let persistence_snapshot = Arc::new(Mutex::new(match &config.persistence {
            PersistenceConfig::Ephemeral => PersistenceSnapshot::ephemeral(),
            PersistenceConfig::Directory { .. } => PersistenceSnapshot::persistent(),
        }));
        let sequence = THREAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let worker_persistence = Arc::clone(&persistence_snapshot);
        let join = std::thread::Builder::new()
            .name(format!("prns-host-{sequence}"))
            .spawn(move || {
                worker(
                    config,
                    sink,
                    WorkerInputs {
                        commands: command_rx,
                        uploads: upload_rx,
                        snapshots: snapshot_rx,
                        shutdown: shutdown_rx,
                        persistence: worker_persistence,
                    },
                    ready_tx,
                )
            })
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
            uploads: upload_tx,
            snapshots: snapshot_tx,
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
        self.submit_with_readiness(command, None)
    }

    pub fn submit_with_readiness(
        &self,
        command: HostCommand,
        readiness: Option<CommandReadiness>,
    ) -> Result<CommandHandle, NativeSubmitError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(NativeSubmitError::Stopped);
        }
        let completion = Arc::new(CommandCompletion::new(readiness));
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

    pub fn begin_resource_upload(
        &self,
        link_id: LinkId,
        declared_length: u64,
        packed_metadata: Option<Vec<u8>>,
        compression: ResourceCompression,
    ) -> Result<NativeUpload, NativeSubmitError> {
        self.begin_resource_upload_with_readiness(
            link_id,
            declared_length,
            packed_metadata,
            compression,
            None,
        )
    }

    pub fn begin_resource_upload_with_readiness(
        &self,
        link_id: LinkId,
        declared_length: u64,
        packed_metadata: Option<Vec<u8>>,
        compression: ResourceCompression,
        readiness: Option<CommandReadiness>,
    ) -> Result<NativeUpload, NativeSubmitError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(NativeSubmitError::Stopped);
        }
        let completion = Arc::new(CommandCompletion::new(readiness));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (chunks, source) = mpsc::channel(UPLOAD_CHUNK_CAPACITY);
        self.uploads
            .try_send(UploadJob {
                link_id,
                declared_length,
                packed_metadata,
                compression,
                source: UploadSource::new(source),
                completion: PendingCompletion(Arc::clone(&completion)),
                cancelled: Arc::clone(&cancelled),
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => NativeSubmitError::Busy,
                mpsc::error::TrySendError::Closed(_) => NativeSubmitError::Stopped,
            })?;
        Ok(NativeUpload {
            chunks: Mutex::new(Some(chunks)),
            completion,
            cancelled,
            declared_length,
            written: AtomicU64::new(0),
            finished: AtomicBool::new(false),
        })
    }

    pub fn snapshot(&self, timeout: Option<Duration>) -> Result<HostSnapshot, NativeSnapshotError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(NativeSnapshotError::Stopped);
        }
        let (reply, result) = std::sync::mpsc::sync_channel(1);
        self.snapshots
            .try_send(SnapshotJob { reply })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => NativeSnapshotError::Busy,
                mpsc::error::TrySendError::Closed(_) => NativeSnapshotError::Stopped,
            })?;
        match timeout {
            Some(timeout) => result.recv_timeout(timeout).map_err(|error| match error {
                std::sync::mpsc::RecvTimeoutError::Timeout => NativeSnapshotError::TimedOut,
                std::sync::mpsc::RecvTimeoutError::Disconnected => NativeSnapshotError::Stopped,
            }),
            None => result.recv().map_err(|_| NativeSnapshotError::Stopped),
        }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadWriteError {
    WouldBlock,
    ChunkTooLarge,
    LengthOverrun,
    Finished,
    Stopped,
}

pub struct NativeUpload {
    chunks: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    completion: Arc<CommandCompletion>,
    cancelled: Arc<AtomicBool>,
    declared_length: u64,
    written: AtomicU64,
    finished: AtomicBool,
}

impl NativeUpload {
    pub fn write(&self, chunk: &[u8]) -> Result<(), UploadWriteError> {
        if self.finished.load(Ordering::Acquire) {
            return Err(UploadWriteError::Finished);
        }
        if chunk.len() > MAX_UPLOAD_CHUNK_BYTES {
            return Err(UploadWriteError::ChunkTooLarge);
        }
        let length = u64::try_from(chunk.len()).map_err(|_| UploadWriteError::LengthOverrun)?;
        let mut chunks = lock(&self.chunks);
        let written = self.written.load(Ordering::Acquire);
        if written.saturating_add(length) > self.declared_length {
            self.finished.store(true, Ordering::Release);
            self.cancelled.store(true, Ordering::Release);
            chunks.take();
            self.completion
                .finish(Err(CommandFailure::ResourceLengthOverrun));
            return Err(UploadWriteError::LengthOverrun);
        }
        let sender = chunks.as_ref().ok_or(UploadWriteError::Finished)?;
        sender
            .try_send(chunk.to_vec())
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => UploadWriteError::WouldBlock,
                mpsc::error::TrySendError::Closed(_) => UploadWriteError::Stopped,
            })?;
        self.written.store(written + length, Ordering::Release);
        Ok(())
    }

    #[must_use]
    pub fn is_writable(&self) -> bool {
        lock(&self.chunks)
            .as_ref()
            .is_some_and(|sender| sender.capacity() > 0 && !sender.is_closed())
    }

    pub fn finish(&self) -> CommandHandle {
        if !self.finished.swap(true, Ordering::AcqRel) {
            lock(&self.chunks).take();
            let written = self.written.load(Ordering::Acquire);
            if written != self.declared_length {
                self.cancelled.store(true, Ordering::Release);
                self.completion
                    .finish(Err(CommandFailure::ResourceEarlyEof));
            }
        }
        CommandHandle {
            completion: Arc::clone(&self.completion),
        }
    }

    pub fn abort(&self) {
        if !self.finished.swap(true, Ordering::AcqRel) {
            self.cancelled.store(true, Ordering::Release);
            lock(&self.chunks).take();
            self.completion
                .finish(Err(CommandFailure::ResourceUploadCancelled));
        }
    }
}

impl Drop for NativeUpload {
    fn drop(&mut self) {
        self.abort();
    }
}

struct UploadJob {
    link_id: LinkId,
    declared_length: u64,
    packed_metadata: Option<Vec<u8>>,
    compression: ResourceCompression,
    source: UploadSource,
    completion: PendingCompletion,
    cancelled: Arc<AtomicBool>,
}

struct SnapshotJob {
    reply: std::sync::mpsc::SyncSender<HostSnapshot>,
}

struct PendingCompletion(Arc<CommandCompletion>);

impl PendingCompletion {
    fn finish(&self, result: Result<CommandOutcome, CommandFailure>) {
        self.0.finish(result);
    }
}

impl Drop for PendingCompletion {
    fn drop(&mut self) {
        self.0.finish(Err(CommandFailure::NodeStopped));
    }
}

struct UploadSource {
    chunks: mpsc::Receiver<Vec<u8>>,
    active: Vec<u8>,
    offset: usize,
}

impl UploadSource {
    fn new(chunks: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            chunks,
            active: Vec::new(),
            offset: 0,
        }
    }
}

impl AsyncRead for UploadSource {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            if self.offset < self.active.len() {
                let available = &self.active[self.offset..];
                let take = available.len().min(output.remaining());
                output.put_slice(&available[..take]);
                self.offset += take;
                return Poll::Ready(Ok(()));
            }
            match Pin::new(&mut self.chunks).poll_recv(context) {
                Poll::Ready(Some(chunk)) => {
                    self.active = chunk;
                    self.offset = 0;
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
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

#[must_use]
pub fn native_interface_kinds() -> &'static [InterfaceKind] {
    &[
        InterfaceKind::TcpClient,
        InterfaceKind::TcpServer,
        InterfaceKind::Udp,
    ]
}

#[must_use]
pub fn native_backend_info() -> BackendInfo {
    BackendInfo::new(
        BackendKind::Native,
        native_capabilities().iter().copied(),
        native_interface_kinds().iter().copied(),
    )
}

#[must_use]
pub fn persistent_endpoint(
    root: &Path,
    destinations: Vec<DestinationConfig>,
    required_capabilities: Vec<Capability>,
    limits: prns_host::PrnsLimits,
) -> HostConfig {
    HostConfig {
        identity: IdentityConfig::LoadOrCreate {
            path: root.join("identity").to_string_lossy().into_owned(),
        },
        persistence: PersistenceConfig::Directory {
            path: root.join("state").to_string_lossy().into_owned(),
        },
        role: HostRole::Endpoint,
        destinations,
        required_capabilities,
        limits,
    }
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
    persistence: ResolvedPersistence,
}

enum ResolvedPersistence {
    Ephemeral,
    Directory(NodePersistence),
}

impl PersistenceIntent for ResolvedPersistence {
    fn into_node_persistence(self) -> Option<NodePersistence> {
        match self {
            Self::Ephemeral => None,
            Self::Directory(persistence) => Some(persistence),
        }
    }
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
            .map_err(|_| NativeStartError::Identity(IdentityStartError::EntropyUnavailable)),
        IdentityConfig::LoadOrCreate { path } => load_or_create_identity_secret(Path::new(&path))
            .map_err(|error| {
                let failure = match error {
                    IdentitySecretFileError::Io(error)
                        if error.kind() == std::io::ErrorKind::PermissionDenied =>
                    {
                        IdentityStartError::PermissionDenied { path }
                    }
                    IdentitySecretFileError::Io(error) => IdentityStartError::Unavailable {
                        path,
                        detail: error.to_string(),
                    },
                    IdentitySecretFileError::Malformed { len } => {
                        IdentityStartError::Malformed { path, length: len }
                    }
                };
                NativeStartError::Identity(failure)
            }),
    }
}

fn resolve_persistence(
    persistence: PersistenceConfig,
) -> Result<ResolvedPersistence, NativeStartError> {
    match persistence {
        PersistenceConfig::Ephemeral => Ok(ResolvedPersistence::Ephemeral),
        PersistenceConfig::Directory { path } => {
            let directory = Path::new(&path);
            if directory.exists() && !directory.is_dir() {
                return Err(NativeStartError::Persistence(
                    PersistenceStartError::NotDirectory { path },
                ));
            }
            NodePersistence::custom_dir(directory)
                .map(ResolvedPersistence::Directory)
                .map_err(|error| {
                    let failure = match error.kind() {
                        std::io::ErrorKind::PermissionDenied => {
                            PersistenceStartError::PermissionDenied { path }
                        }
                        std::io::ErrorKind::NotADirectory => {
                            PersistenceStartError::NotDirectory { path }
                        }
                        _ => PersistenceStartError::Unavailable {
                            path,
                            detail: error.to_string(),
                        },
                    };
                    NativeStartError::Persistence(failure)
                })
        }
    }
}

fn resolve_config(config: HostConfig) -> Result<ResolvedConfig, NativeStartError> {
    let host_identity = resolve_identity(config.identity)?;
    let persistence = resolve_persistence(config.persistence)?;
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
        persistence,
    })
}

fn aspect_refs(destinations: &[ResolvedDestination]) -> Vec<Vec<&str>> {
    destinations
        .iter()
        .map(|destination| destination.aspects.iter().map(String::as_str).collect())
        .collect()
}

fn build_destinations<'a>(
    destinations: &'a [ResolvedDestination],
    aspects: &'a [Vec<&'a str>],
) -> Vec<PreConfiguredDestination<'a>> {
    destinations
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
                request_endpoints: personal_rns::runtime::ServeMyRequestEndpoints::No,
            },
        })
        .collect()
}

struct WorkerInputs {
    commands: mpsc::Receiver<CommandJob>,
    uploads: mpsc::Receiver<UploadJob>,
    snapshots: mpsc::Receiver<SnapshotJob>,
    shutdown: watch::Receiver<bool>,
    persistence: Arc<Mutex<PersistenceSnapshot>>,
}

fn worker(
    config: HostConfig,
    sink: Arc<dyn NativeEventSink>,
    inputs: WorkerInputs,
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
    runtime.block_on(run(config, sink, inputs, ready_tx));
    runtime.shutdown_timeout(STOP_TIMEOUT);
}

async fn run(
    config: HostConfig,
    sink: Arc<dyn NativeEventSink>,
    inputs: WorkerInputs,
    ready_tx: std::sync::mpsc::SyncSender<Result<Ready, NativeStartError>>,
) {
    let WorkerInputs {
        commands: command_rx,
        uploads: upload_rx,
        snapshots: snapshot_rx,
        shutdown: shutdown_rx,
        persistence: persistence_snapshot,
    } = inputs;
    let resolved = match resolve_config(config) {
        Ok(config) => config,
        Err(error) => {
            let _ = ready_tx.send(Err(error.clone()));
            sink.failed(format!("{error:?}"));
            return;
        }
    };
    let ResolvedConfig {
        host_identity,
        role,
        destinations: resolved_destinations,
        persistence,
    } = resolved;
    let identity = personal_rns::identity::PrivateIdentityMaterial::from_slice(&host_identity[..])
        .map(|material| IdentityHash::new(*material.identity_hash().as_bytes()))
        .map_err(|_| NativeStartError::Identity(IdentityStartError::InvalidMaterial));
    let identity_hash = match identity {
        Ok(identity) => identity,
        Err(error) => {
            let _ = ready_tx.send(Err(error.clone()));
            sink.failed(format!("{error:?}"));
            return;
        }
    };
    let aspects = aspect_refs(&resolved_destinations);
    let destinations = build_destinations(&resolved_destinations, &aspects);
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
    let event_persistence = Arc::clone(&persistence_snapshot);
    let mut node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: match role {
            HostRole::Endpoint => None,
            HostRole::Transport => Some(host_identity.clone()),
        },
        pre_configured_destinations: destinations,
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence,
        on_event: move |event, _state: &()| {
            if !publish_event(event_sink.as_ref(), event, &event_persistence) {
                event_backpressure.notify_waiters();
            }
        },
    });
    for (destination, hash) in resolved_destinations.iter().zip(&destination_hashes) {
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
    let commands = tokio::spawn(command_loop(
        command_rx,
        snapshot_rx,
        handle.clone(),
        shutdown_rx.clone(),
        persistence_snapshot,
        Instant::now(),
    ));
    let uploads = tokio::spawn(upload_loop(upload_rx, handle, shutdown_rx.clone()));
    let exit = tokio::select! {
        result = node.run_until(shutdown_requested(shutdown_rx)) => {
            result.map_err(|error| format!("{error}"))
        },
        () = backpressure.notified() => Err("application event backpressure exceeded".to_string()),
    };
    commands.abort();
    let _ = commands.await;
    uploads.abort();
    let _ = uploads.await;
    match exit {
        Ok(()) => sink.stopped(),
        Err(detail) => sink.failed(detail),
    }
}

async fn shutdown_requested(mut shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    let _ = shutdown.wait_for(|stopping| *stopping).await;
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
    mut snapshots: mpsc::Receiver<SnapshotJob>,
    handle: PrnsNodeHandle,
    mut shutdown: watch::Receiver<bool>,
    persistence: Arc<Mutex<PersistenceSnapshot>>,
    started_at: Instant,
) {
    let mut attachments = BTreeMap::new();
    let mut snapshot_revision = 0u64;
    loop {
        let work = tokio::select! {
            command = commands.recv() => match command {
                Some(command) => HostWork::Command(command),
                None => break,
            },
            snapshot = snapshots.recv() => match snapshot {
                Some(snapshot) => HostWork::Snapshot(snapshot),
                None => break,
            },
            _ = shutdown.wait_for(|stopping| *stopping) => break,
        };
        match work {
            HostWork::Command(job) => {
                let result = execute_command(&handle, &mut attachments, &job.command).await;
                job.completion.finish(result);
            }
            HostWork::Snapshot(job) => {
                snapshot_revision = snapshot_revision.saturating_add(1);
                if let Some(snapshot) = collect_snapshot(
                    &handle,
                    &attachments,
                    snapshot_revision,
                    started_at,
                    &persistence,
                )
                .await
                {
                    let _ = job.reply.send(snapshot);
                }
            }
        }
    }
    for (_, attachment) in attachments {
        attachment.teardown();
    }
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
}

enum HostWork {
    Command(CommandJob),
    Snapshot(SnapshotJob),
}

async fn collect_snapshot(
    handle: &PrnsNodeHandle,
    attachments: &BTreeMap<InterfaceId, Attachment>,
    revision: u64,
    started_at: Instant,
    persistence: &Mutex<PersistenceSnapshot>,
) -> Option<HostSnapshot> {
    let inventory = logical_interface_inventory(handle.interface_inventory());
    let interfaces: Vec<InterfaceSnapshot> = inventory
        .into_iter()
        .filter(|entry| attachments.contains_key(&host_interface(entry.snapshot.id)))
        .map(|entry| {
            let rates = entry.snapshot.transfer_rates;
            InterfaceSnapshot {
                interface_id: host_interface(entry.snapshot.id),
                name: entry.name,
                kind: entry.snapshot.id.kind().and_then(host_interface_kind),
                health: host_interface_health(entry.snapshot.connection),
                failure_detail: entry.snapshot.failure_reason.map(str::to_string),
                rx_bytes: entry.snapshot.rx_bytes,
                tx_bytes: entry.snapshot.tx_bytes,
                rx_bps: rates.map(|rates| u64::from(rates.rx_bps)),
                tx_bps: rates.map(|rates| u64::from(rates.tx_bps)),
                route_count: entry.snapshot.destinations,
                link_count: entry.snapshot.links,
                transported_link_count: entry.snapshot.transported_links,
            }
        })
        .collect();
    let engine = handle.engine_inspection_snapshot().await?;
    let routes: Vec<RouteSnapshot> = engine
        .routes
        .into_iter()
        .map(|route| RouteSnapshot {
            destination: host_destination(route.destination),
            hops: route.hops,
            via_identity: match route.via {
                personal_rns::routing::routes::NextHop::Direct => None,
                personal_rns::routing::routes::NextHop::Via(identity) => {
                    Some(IdentityHash::new(*identity.as_bytes()))
                }
            },
            interface_id: host_interface(route.interface),
            learned_at_millis: route.learned_at.0,
            last_relayed_at_millis: route.last_relayed_at.0,
            expires_at_millis: route.expires_at.0,
        })
        .collect();
    let destination_identities = engine
        .destination_identities
        .into_iter()
        .map(|identity| DestinationIdentitySnapshot {
            destination: host_destination(identity.destination),
            identity: IdentityHash::new(*identity.identity.as_bytes()),
        })
        .collect();
    let interface_count = u32::try_from(interfaces.len()).unwrap_or(u32::MAX);
    let online_interface_count = u32::try_from(
        interfaces
            .iter()
            .filter(|interface| {
                matches!(
                    interface.health,
                    InterfaceHealth::Connected | InterfaceHealth::Degraded
                )
            })
            .count(),
    )
    .unwrap_or(u32::MAX);
    let runtime = RuntimeHealthSnapshot {
        running: true,
        uptime_millis: u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        interface_count,
        online_interface_count,
        route_count: u32::try_from(routes.len()).unwrap_or(u32::MAX),
        link_count: engine.link_count,
        transported_link_count: interfaces.iter().fold(0u32, |sum, entry| {
            sum.saturating_add(entry.transported_link_count)
        }),
        rx_bytes: interfaces
            .iter()
            .fold(0u64, |sum, entry| sum.saturating_add(entry.rx_bytes)),
        tx_bytes: interfaces
            .iter()
            .fold(0u64, |sum, entry| sum.saturating_add(entry.tx_bytes)),
        rx_bps: interfaces.iter().fold(0u64, |sum, entry| {
            sum.saturating_add(entry.rx_bps.unwrap_or_default())
        }),
        tx_bps: interfaces.iter().fold(0u64, |sum, entry| {
            sum.saturating_add(entry.tx_bps.unwrap_or_default())
        }),
    };
    Some(HostSnapshot {
        revision,
        backend: native_backend_info(),
        interfaces,
        routes,
        active_link_count: engine.link_count,
        destination_identities,
        runtime,
        persistence: lock(persistence).clone(),
    })
}

fn host_interface_health(health: ConnectionState) -> InterfaceHealth {
    match health {
        ConnectionState::Initializing => InterfaceHealth::Initializing,
        ConnectionState::Connected => InterfaceHealth::Connected,
        ConnectionState::Degraded => InterfaceHealth::Degraded,
        ConnectionState::Reconnecting => InterfaceHealth::Reconnecting,
        ConnectionState::Failed => InterfaceHealth::Failed,
        ConnectionState::Disconnected => InterfaceHealth::Disconnected,
        ConnectionState::Disabled => InterfaceHealth::Disabled,
        ConnectionState::Unknown => InterfaceHealth::Unknown,
    }
}

fn host_interface_kind(kind: EngineInterfaceKind) -> Option<InterfaceKind> {
    match kind {
        EngineInterfaceKind::TcpClient => Some(InterfaceKind::TcpClient),
        EngineInterfaceKind::TcpServer | EngineInterfaceKind::TcpServerPeer => {
            Some(InterfaceKind::TcpServer)
        }
        EngineInterfaceKind::Udp => Some(InterfaceKind::Udp),
        EngineInterfaceKind::Serial => Some(InterfaceKind::Serial),
        EngineInterfaceKind::UsbAutoHost | EngineInterfaceKind::UsbAutoDevice => {
            Some(InterfaceKind::AutomaticUsb)
        }
        EngineInterfaceKind::AutoWifi
        | EngineInterfaceKind::WifiPeer
        | EngineInterfaceKind::WifiDirect
        | EngineInterfaceKind::WifiDirectPeer
        | EngineInterfaceKind::WifiAware
        | EngineInterfaceKind::WifiAwarePeer => Some(InterfaceKind::AutoLan),
        EngineInterfaceKind::BluetoothAuto | EngineInterfaceKind::BluetoothPeer => {
            Some(InterfaceKind::AutomaticBluetoothLe)
        }
        EngineInterfaceKind::Kiss => Some(InterfaceKind::Kiss),
        EngineInterfaceKind::Ax25Kiss => Some(InterfaceKind::Ax25Kiss),
        EngineInterfaceKind::Pipe => Some(InterfaceKind::Pipe),
        EngineInterfaceKind::Rnode | EngineInterfaceKind::LoRa => Some(InterfaceKind::RNode),
        EngineInterfaceKind::BackboneServer | EngineInterfaceKind::BackboneServerPeer => {
            Some(InterfaceKind::BackboneServer)
        }
        EngineInterfaceKind::BackboneClient => Some(InterfaceKind::BackboneClient),
        EngineInterfaceKind::WebSocketClient => Some(InterfaceKind::WebSocketClient),
        EngineInterfaceKind::WebSocketServer | EngineInterfaceKind::WebSocketServerPeer => {
            Some(InterfaceKind::WebSocketServer)
        }
        EngineInterfaceKind::I2p | EngineInterfaceKind::I2pPeer => Some(InterfaceKind::I2p),
        EngineInterfaceKind::Weave | EngineInterfaceKind::WeavePeer => Some(InterfaceKind::Weave),
        EngineInterfaceKind::Loopback
        | EngineInterfaceKind::LocalServer
        | EngineInterfaceKind::LocalClient
        | EngineInterfaceKind::EspNow => None,
    }
}

async fn upload_loop(
    mut uploads: mpsc::Receiver<UploadJob>,
    handle: PrnsNodeHandle,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        let job = tokio::select! {
            job = uploads.recv() => match job {
                Some(job) => job,
                None => break,
            },
            _ = shutdown.wait_for(|stopping| *stopping) => break,
        };
        let UploadJob {
            link_id,
            declared_length,
            packed_metadata,
            compression,
            source,
            completion,
            cancelled,
        } = job;
        let compression = match compression {
            ResourceCompression::Auto => SegmentCompression::AUTO,
            ResourceCompression::Never => SegmentCompression::Never,
        };
        let result = match packed_metadata {
            Some(metadata) => {
                let (progress, _) = mpsc::unbounded_channel();
                handle
                    .send_resource_with_options(
                        engine_link(link_id),
                        declared_length,
                        source,
                        &metadata,
                        compression,
                        progress,
                    )
                    .await
            }
            None => {
                handle
                    .send_resource_with_compression(
                        engine_link(link_id),
                        declared_length,
                        source,
                        compression,
                    )
                    .await
            }
        };
        let result = match result {
            Ok(()) => Ok(CommandOutcome::ResourceSent),
            Err(ResourceSendError::Source(error))
                if cancelled.load(Ordering::Acquire)
                    && error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                Err(CommandFailure::ResourceUploadCancelled)
            }
            Err(ResourceSendError::Source(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                Err(CommandFailure::ResourceEarlyEof)
            }
            Err(error) => Err(resource_send_failure(error)),
        };
        completion.finish(result);
    }
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
            let server = TcpServer::bind_with_bitrate(bind.as_str(), engine_bitrate(*bitrate)?)
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
            let client = TcpClientInterface::new_with_bitrate(
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
        HostCommand::AttachInterface { config } => {
            config
                .validate()
                .map_err(|error| CommandFailure::InvalidConfiguration {
                    detail: format!("{error:?}"),
                })?;
            match config {
                InterfaceConfig::TcpServer { bind, bitrate } => {
                    let server =
                        TcpServer::bind_with_bitrate(bind.as_str(), engine_bitrate(*bitrate)?)
                            .await
                            .map_err(|error| CommandFailure::BindFailed {
                                detail: error.to_string(),
                            })?;
                    let attached = handle.supervise(server);
                    let interface = host_interface(attached.id());
                    attachments.insert(interface, Attachment::Supervisor(attached));
                    Ok(CommandOutcome::InterfaceAttached { interface })
                }
                InterfaceConfig::TcpClient { target, bitrate } => {
                    let client = TcpClientInterface::new_with_bitrate(
                        target.clone(),
                        engine_bitrate(*bitrate)?,
                        ReconnectPolicy::STANDARD,
                    );
                    let attached = handle.add_interface(client);
                    let interface = host_interface(attached.id());
                    attachments.insert(interface, Attachment::Interface(attached));
                    Ok(CommandOutcome::InterfaceAttached { interface })
                }
                InterfaceConfig::Udp {
                    local,
                    peer,
                    bitrate,
                } => {
                    let udp = UdpInterface::bind(
                        local.as_str(),
                        peer.as_str(),
                        engine_bitrate(*bitrate)?,
                    )
                    .await
                    .map_err(|error| CommandFailure::BindFailed {
                        detail: error.to_string(),
                    })?;
                    let attached = handle.add_interface(udp);
                    let interface = host_interface(attached.id());
                    attachments.insert(interface, Attachment::Interface(attached));
                    Ok(CommandOutcome::InterfaceAttached { interface })
                }
                _ => Err(CommandFailure::UnsupportedByBackend),
            }
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

fn publish_event(
    sink: &dyn NativeEventSink,
    event: PrnsEvent<'_>,
    persistence: &Mutex<PersistenceSnapshot>,
) -> bool {
    match event {
        PrnsEvent::Message(message) => publish_message(sink, message),
        PrnsEvent::Diagnostic(diagnostic) => {
            if let Some(diagnostic) = translate_diagnostic(diagnostic) {
                update_persistence_snapshot(persistence, &diagnostic);
                sink.publish_diagnostic(diagnostic);
            }
            true
        }
    }
}

fn update_persistence_snapshot(
    persistence: &Mutex<PersistenceSnapshot>,
    diagnostic: &DiagnosticEvent,
) {
    let mut persistence = lock(persistence);
    match diagnostic {
        DiagnosticEvent::PersistenceRestored { .. } => {
            persistence.restored = true;
            persistence.last_failure_detail = None;
        }
        DiagnosticEvent::PersistenceFlushed { cause, .. } => {
            persistence.last_flush_cause = Some(*cause);
            persistence.last_failure_detail = None;
        }
        DiagnosticEvent::PersistenceFlushFailed { cause, target } => {
            persistence.last_failure_detail = Some(format!("{cause:?}:{target:?}"));
        }
        _ => {}
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

fn translate_diagnostic(diagnostic: Diagnostic) -> Option<DiagnosticEvent> {
    Some(match diagnostic {
        Diagnostic::PersistenceRestored {
            routes,
            destination_identities,
            tunnels,
            ratchets,
            refused,
            dropped,
        } => DiagnosticEvent::PersistenceRestored {
            routes: routes.into(),
            destination_identities: destination_identities.into(),
            tunnels: tunnels.into(),
            ratchets: ratchets.into(),
            refused: refused.into(),
            dropped: dropped.into(),
        },
        Diagnostic::PersistenceFlushed { cause, target } => DiagnosticEvent::PersistenceFlushed {
            cause: persistence_flush_cause(cause),
            target: persistence_flush_target(target),
        },
        Diagnostic::PersistenceFlushFailed { cause, target } => {
            DiagnosticEvent::PersistenceFlushFailed {
                cause: persistence_flush_cause(cause),
                target: persistence_flush_target(target),
            }
        }
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
    })
}

fn persistence_flush_cause(cause: EnginePersistenceFlushCause) -> PersistenceFlushCause {
    match cause {
        EnginePersistenceFlushCause::Startup => PersistenceFlushCause::Startup,
        EnginePersistenceFlushCause::Interval => PersistenceFlushCause::Interval,
        EnginePersistenceFlushCause::RouteChange => PersistenceFlushCause::RouteChange,
        EnginePersistenceFlushCause::RatchetRotation => PersistenceFlushCause::RatchetRotation,
        EnginePersistenceFlushCause::Shutdown => PersistenceFlushCause::Shutdown,
    }
}

fn persistence_flush_target(target: EnginePersistenceFlushTarget) -> PersistenceFlushTarget {
    match target {
        EnginePersistenceFlushTarget::RoutingState => PersistenceFlushTarget::RoutingState,
        EnginePersistenceFlushTarget::Ratchets => PersistenceFlushTarget::Ratchets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_host::{DestinationName, PrnsLimits, SingleDestinationConfig};
    use std::fs;
    use std::sync::atomic::AtomicUsize;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    struct RecordingSink {
        diagnostics: Mutex<Vec<DiagnosticEvent>>,
        changed: Condvar,
    }

    impl RecordingSink {
        fn new() -> Self {
            Self {
                diagnostics: Mutex::new(Vec::new()),
                changed: Condvar::new(),
            }
        }

        fn wait_for(&self, predicate: impl Fn(&DiagnosticEvent) -> bool) -> bool {
            let diagnostics = lock(&self.diagnostics);
            let waited = self
                .changed
                .wait_timeout_while(diagnostics, Duration::from_secs(2), |events| {
                    !events.iter().any(&predicate)
                })
                .unwrap_or_else(PoisonError::into_inner);
            waited.0.iter().any(predicate)
        }

        fn diagnostics(&self) -> Vec<DiagnosticEvent> {
            lock(&self.diagnostics).clone()
        }
    }

    impl NativeEventSink for RecordingSink {
        fn running(&self) {}

        fn publish_application(&self, _event: ApplicationEvent) -> bool {
            true
        }

        fn publish_resource(&self, _event: ResourceAvailable, _body: Vec<u8>) -> bool {
            true
        }

        fn publish_diagnostic(&self, event: DiagnosticEvent) {
            lock(&self.diagnostics).push(event);
            self.changed.notify_all();
        }

        fn stopped(&self) {}

        fn failed(&self, _detail: String) {}
    }

    fn config() -> HostConfig {
        HostConfig {
            identity: IdentityConfig::GenerateEphemeral,
            persistence: PersistenceConfig::Ephemeral,
            role: HostRole::Endpoint,
            destinations: Vec::new(),
            required_capabilities: Vec::new(),
            limits: PrnsLimits::balanced(),
        }
    }

    fn temporary_root(label: &str) -> Result<std::path::PathBuf, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?;
        Ok(std::env::temp_dir().join(format!(
            "prns-host-native-{label}-{}-{}",
            std::process::id(),
            now.as_nanos()
        )))
    }

    fn persistent_config(root: &Path) -> HostConfig {
        persistent_endpoint(root, Vec::new(), Vec::new(), PrnsLimits::balanced())
    }

    #[test]
    fn command_completion_and_interruption_signal_readiness() {
        let readiness_count = Arc::new(AtomicUsize::new(0));
        let callback_count = Arc::clone(&readiness_count);
        let completion = Arc::new(CommandCompletion::new(Some(Arc::new(move || {
            callback_count.fetch_add(1, Ordering::AcqRel);
        }))));
        let command = CommandHandle {
            completion: Arc::clone(&completion),
        };
        completion.finish(Ok(CommandOutcome::Announced));
        assert_eq!(readiness_count.load(Ordering::Acquire), 1);
        command.interrupt_wait();
        assert_eq!(readiness_count.load(Ordering::Acquire), 2);
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
    fn snapshots_track_interface_changes_consistently() -> Result<(), String> {
        let host =
            NativeHost::start(config(), Arc::new(Sink)).map_err(|error| format!("{error:?}"))?;
        let initial = host
            .snapshot(Some(Duration::from_secs(2)))
            .map_err(|error| format!("{error:?}"))?;
        if initial.revision != 1
            || !initial.interfaces.is_empty()
            || !initial.routes.is_empty()
            || initial.active_link_count != 0
            || initial.runtime.interface_count != 0
        {
            return Err("initial snapshot was inconsistent".to_string());
        }
        let attached = host
            .submit(HostCommand::AttachTcpClient {
                target: "127.0.0.1:9".to_string(),
                bitrate: Bitrate::Auto,
            })
            .map_err(|error| format!("{error:?}"))?;
        let interface = match attached.wait(Some(Duration::from_secs(2))) {
            CommandWait::Completed(Ok(CommandOutcome::InterfaceAttached { interface })) => {
                interface
            }
            other => return Err(format!("{other:?}")),
        };
        let attached_snapshot = host
            .snapshot(Some(Duration::from_secs(2)))
            .map_err(|error| format!("{error:?}"))?;
        if attached_snapshot.revision != 2
            || attached_snapshot.interfaces.len() != 1
            || attached_snapshot.interfaces[0].interface_id != interface
            || attached_snapshot.interfaces[0].kind != Some(InterfaceKind::TcpClient)
            || attached_snapshot.runtime.interface_count != 1
        {
            return Err("attached interface snapshot was inconsistent".to_string());
        }
        let detached = host
            .submit(HostCommand::DetachInterface { interface })
            .map_err(|error| format!("{error:?}"))?;
        if !matches!(
            detached.wait(Some(Duration::from_secs(2))),
            CommandWait::Completed(Ok(CommandOutcome::InterfaceDetached { .. }))
        ) {
            return Err("interface did not detach".to_string());
        }
        let detached_snapshot = host
            .snapshot(Some(Duration::from_secs(2)))
            .map_err(|error| format!("{error:?}"))?;
        if detached_snapshot.revision != 3
            || !detached_snapshot.interfaces.is_empty()
            || detached_snapshot.runtime.interface_count != 0
        {
            return Err("detached interface snapshot was inconsistent".to_string());
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

    #[test]
    fn persistent_host_restores_identity_and_flushes_on_shutdown() -> Result<(), String> {
        let root = temporary_root("restart")?;
        let first_sink = Arc::new(RecordingSink::new());
        let first = NativeHost::start(persistent_config(&root), first_sink.clone())
            .map_err(|error| format!("{error:?}"))?;
        let identity = first.identity_hash();
        if !first_sink
            .wait_for(|event| matches!(event, DiagnosticEvent::PersistenceRestored { .. }))
        {
            return Err("first host did not report persistence restoration".to_string());
        }
        first.stop();
        if !first_sink.diagnostics().iter().any(|event| {
            matches!(
                event,
                DiagnosticEvent::PersistenceFlushed {
                    cause: PersistenceFlushCause::Shutdown,
                    ..
                }
            )
        }) {
            return Err("first host did not flush persistence during shutdown".to_string());
        }

        let second_sink = Arc::new(RecordingSink::new());
        let second = NativeHost::start(persistent_config(&root), second_sink.clone())
            .map_err(|error| format!("{error:?}"))?;
        if second.identity_hash() != identity {
            return Err("persistent host identity changed across restart".to_string());
        }
        if !second_sink
            .wait_for(|event| matches!(event, DiagnosticEvent::PersistenceRestored { .. }))
        {
            return Err("second host did not report persistence restoration".to_string());
        }
        second.stop();
        fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn persistence_path_must_be_a_directory() -> Result<(), String> {
        let root = temporary_root("not-directory")?;
        fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let state = root.join("state");
        fs::write(&state, b"not a directory").map_err(|error| error.to_string())?;
        let result = NativeHost::start(persistent_config(&root), Arc::new(Sink));
        let expected = NativeStartError::Persistence(PersistenceStartError::NotDirectory {
            path: state.to_string_lossy().into_owned(),
        });
        if result.err() != Some(expected) {
            return Err("non-directory persistence path was not typed".to_string());
        }
        fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
        Ok(())
    }

    fn isolated_upload(declared_length: u64) -> (NativeUpload, UploadSource) {
        let completion = Arc::new(CommandCompletion::new(None));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (chunks, source) = mpsc::channel(UPLOAD_CHUNK_CAPACITY);
        (
            NativeUpload {
                chunks: Mutex::new(Some(chunks)),
                completion,
                cancelled,
                declared_length,
                written: AtomicU64::new(0),
                finished: AtomicBool::new(false),
            },
            UploadSource::new(source),
        )
    }

    #[test]
    fn upload_reports_overrun_and_early_eof() {
        let (overrun, _source) = isolated_upload(1);
        assert_eq!(overrun.write(&[1, 2]), Err(UploadWriteError::LengthOverrun));
        assert!(matches!(
            overrun.finish().wait(Some(Duration::ZERO)),
            CommandWait::Completed(Err(CommandFailure::ResourceLengthOverrun))
        ));

        let (early, _source) = isolated_upload(2);
        assert_eq!(early.write(&[1]), Ok(()));
        assert!(matches!(
            early.finish().wait(Some(Duration::ZERO)),
            CommandWait::Completed(Err(CommandFailure::ResourceEarlyEof))
        ));
    }

    #[test]
    fn upload_is_bounded_and_release_cancels() {
        let (upload, _source) = isolated_upload(16);
        for _ in 0..UPLOAD_CHUNK_CAPACITY {
            assert_eq!(upload.write(&[1]), Ok(()));
        }
        assert_eq!(upload.write(&[1]), Err(UploadWriteError::WouldBlock));
        let command = CommandHandle {
            completion: Arc::clone(&upload.completion),
        };
        drop(upload);
        assert!(matches!(
            command.wait(Some(Duration::ZERO)),
            CommandWait::Completed(Err(CommandFailure::ResourceUploadCancelled))
        ));
    }

    #[test]
    fn upload_rejects_oversized_chunks() {
        let (upload, _source) = isolated_upload((MAX_UPLOAD_CHUNK_BYTES + 1) as u64);
        let chunk = vec![0; MAX_UPLOAD_CHUNK_BYTES + 1];
        assert_eq!(upload.write(&chunk), Err(UploadWriteError::ChunkTooLarge));
    }
}
