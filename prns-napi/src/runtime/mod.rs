use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use personal_rns::engine::RatchetPolicy;
use personal_rns::routing::request_handlers::RequestPolicy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Manual, PreConfiguredDestination, PrnsNodeRecipe, RequestHandlerRegistration,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::{
    routes, PlanAttachments, PrnsNode, PrnsNodeHandle, ResourceStrategy, Zeroizing,
    IDENTITY_SECRET_KEY_LEN,
};
use tokio::sync::{mpsc, oneshot};

use crate::errors::{code_err, CodeResult, ErrorCode};
use crate::events::bridge::EventSink;

const START_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(4);

static THREAD_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct SingleConfig {
    pub identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    pub announce_app_data: Vec<u8>,
    pub proof: ProofStrategy,
    pub link_requests: LinkRequestPolicy,
    pub ratchet: RatchetPolicy,
    pub resource_strategy: ResourceStrategy,
    pub request_paths: Vec<(String, RequestPolicy)>,
}

pub struct DestinationConfig {
    pub app_name: String,
    pub aspects: Vec<String>,
    pub single: Option<SingleConfig>,
}

pub struct NodeConfig {
    pub transport_identity: Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>,
    pub destinations: Vec<DestinationConfig>,
}

fn build_destinations<'a>(
    config: &'a NodeConfig,
    aspect_refs: &'a [Vec<&'a str>],
) -> Vec<PreConfiguredDestination<'a>> {
    config
        .destinations
        .iter()
        .zip(aspect_refs)
        .map(|(dest, aspects)| match &dest.single {
            None => PreConfiguredDestination::Plain {
                app_name: &dest.app_name,
                aspects: aspects.as_slice(),
            },
            Some(single) => PreConfiguredDestination::Single {
                app_name: &dest.app_name,
                aspects: aspects.as_slice(),
                identity: single.identity.clone(),
                announce_app_data: &single.announce_app_data,
                proof: single.proof,
                link_requests: single.link_requests,
                ratchet: single.ratchet,
                resource_strategy: single.resource_strategy,
                request_handlers: RequestHandlerRegistration::None,
            },
        })
        .collect()
}

fn aspect_refs(config: &NodeConfig) -> Vec<Vec<&str>> {
    config
        .destinations
        .iter()
        .map(|dest| dest.aspects.iter().map(String::as_str).collect())
        .collect()
}

pub fn destination_hashes(config: &NodeConfig) -> CodeResult<Vec<[u8; 16]>> {
    let aspects = aspect_refs(config);
    build_destinations(config, &aspects)
        .iter()
        .map(|dest| {
            dest.destination_hash()
                .map(|hash| *hash.as_bytes())
                .map_err(|error| {
                    code_err(
                        ErrorCode::InvalidArgument,
                        format!("invalid destination name: {error:?}"),
                    )
                })
        })
        .collect()
}

pub enum Exit {
    Stopped,
    Crashed(String),
}

pub type BoxedControlFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
pub type ControlJob = Box<dyn FnOnce(PrnsNodeHandle) -> BoxedControlFuture + Send>;

struct LiveState {
    ready: Option<oneshot::Receiver<Result<PrnsNodeHandle, String>>>,
    handle: Option<PrnsNodeHandle>,
    control: mpsc::Sender<ControlJob>,
    shutdown: oneshot::Sender<()>,
    stopped: oneshot::Receiver<Exit>,
    join: std::thread::JoinHandle<()>,
    plan_attachments: Vec<PlanAttachments>,
    sink: EventSink,
}

enum ManagerState {
    Live(Box<LiveState>),
    Waiting,
    Stopping,
    Stopped,
}

pub struct NodeManager {
    state: Mutex<ManagerState>,
}

impl NodeManager {
    pub fn start(config: NodeConfig, sink: EventSink, pending_commands: usize) -> CodeResult<Self> {
        let (ready_tx, ready_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (stopped_tx, stopped_rx) = oneshot::channel();
        let (control_tx, control_rx) = mpsc::channel(pending_commands);
        let sequence = THREAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let live_sink = sink.clone();
        let join = std::thread::Builder::new()
            .name(format!("prns-node-{sequence}"))
            .spawn(move || worker(config, sink, ready_tx, shutdown_rx, control_rx, stopped_tx))
            .map_err(|error| {
                code_err(
                    ErrorCode::StartFailed,
                    format!("node thread spawn failed: {error}"),
                )
            })?;
        Ok(Self {
            state: Mutex::new(ManagerState::Live(Box::new(LiveState {
                ready: Some(ready_rx),
                handle: None,
                control: control_tx,
                shutdown: shutdown_tx,
                stopped: stopped_rx,
                join,
                plan_attachments: Vec::new(),
                sink: live_sink,
            }))),
        })
    }

    fn lock(&self) -> MutexGuard<'_, ManagerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub async fn ready(&self) -> CodeResult<()> {
        let mut live = {
            let mut state = self.lock();
            match core::mem::replace(&mut *state, ManagerState::Waiting) {
                ManagerState::Live(live) => {
                    if live.ready.is_none() {
                        *state = ManagerState::Live(live);
                        return Ok(());
                    }
                    live
                }
                ManagerState::Waiting => {
                    *state = ManagerState::Waiting;
                    return Err(code_err(ErrorCode::Busy, "ready() is already pending"));
                }
                other => {
                    *state = other;
                    return Err(code_err(ErrorCode::NodeStopped, "node stopped"));
                }
            }
        };
        let Some(ready) = live.ready.take() else {
            *self.lock() = ManagerState::Live(live);
            return Ok(());
        };
        match tokio::time::timeout(START_TIMEOUT, ready).await {
            Ok(Ok(Ok(handle))) => {
                live.handle = Some(handle);
                *self.lock() = ManagerState::Live(live);
                Ok(())
            }
            Ok(Ok(Err(reason))) => {
                *self.lock() = ManagerState::Stopped;
                Err(code_err(ErrorCode::StartFailed, reason))
            }
            Ok(Err(_)) => {
                *self.lock() = ManagerState::Stopped;
                Err(code_err(
                    ErrorCode::StartFailed,
                    "node thread exited during startup",
                ))
            }
            Err(_) => {
                *self.lock() = ManagerState::Stopped;
                Err(code_err(
                    ErrorCode::StartTimeout,
                    "node did not become ready within 5s",
                ))
            }
        }
    }

    pub fn handle(&self) -> CodeResult<PrnsNodeHandle> {
        let state = self.lock();
        match &*state {
            ManagerState::Live(live) => match &live.handle {
                Some(handle) => Ok(handle.clone()),
                None => Err(code_err(
                    ErrorCode::NotReady,
                    "node is not ready; await node.ready() first",
                )),
            },
            ManagerState::Waiting => Err(code_err(
                ErrorCode::NotReady,
                "node is not ready; await node.ready() first",
            )),
            _ => Err(code_err(ErrorCode::NodeStopped, "node stopped")),
        }
    }

    fn control(&self) -> CodeResult<mpsc::Sender<ControlJob>> {
        let state = self.lock();
        match &*state {
            ManagerState::Live(live) if live.handle.is_some() => Ok(live.control.clone()),
            ManagerState::Live(_) | ManagerState::Waiting => Err(code_err(
                ErrorCode::NotReady,
                "node is not ready; await node.ready() first",
            )),
            _ => Err(code_err(ErrorCode::NodeStopped, "node stopped")),
        }
    }

    pub async fn on_node_runtime<T, F, Fut>(&self, run: F) -> CodeResult<T>
    where
        T: Send + 'static,
        F: FnOnce(PrnsNodeHandle) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        let control = self.control()?;
        let (result_tx, result_rx) = oneshot::channel();
        let job: ControlJob = Box::new(move |handle| {
            Box::pin(async move {
                let value = run(handle).await;
                let _ = result_tx.send(value);
            })
        });
        control.try_send(job).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => code_err(ErrorCode::Busy, "engine busy"),
            mpsc::error::TrySendError::Closed(_) => {
                code_err(ErrorCode::NodeStopped, "node stopped")
            }
        })?;
        result_rx
            .await
            .map_err(|_| code_err(ErrorCode::NodeStopped, "node stopped"))
    }

    pub fn sink(&self) -> CodeResult<EventSink> {
        let state = self.lock();
        match &*state {
            ManagerState::Live(live) => Ok(live.sink.clone()),
            _ => Err(code_err(ErrorCode::NodeStopped, "node stopped")),
        }
    }

    pub fn store_plan_attachments(&self, attachments: PlanAttachments) {
        let mut state = self.lock();
        if let ManagerState::Live(live) = &mut *state {
            live.plan_attachments.push(attachments);
        }
    }

    pub async fn stop(&self) -> CodeResult<()> {
        let live = {
            let mut state = self.lock();
            match core::mem::replace(&mut *state, ManagerState::Stopping) {
                ManagerState::Live(live) => live,
                ManagerState::Waiting => {
                    *state = ManagerState::Waiting;
                    return Err(code_err(
                        ErrorCode::Busy,
                        "ready() is pending; await it before stop()",
                    ));
                }
                other => {
                    *state = other;
                    return Ok(());
                }
            }
        };
        let _ = live.shutdown.send(());
        match tokio::time::timeout(STOP_TIMEOUT, live.stopped).await {
            Ok(_) => {
                let join = live.join;
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = join.join();
                })
                .await;
                *self.lock() = ManagerState::Stopped;
                Ok(())
            }
            Err(_) => {
                *self.lock() = ManagerState::Stopped;
                Err(code_err(
                    ErrorCode::ShutdownTimeout,
                    "node did not stop within 5s",
                ))
            }
        }
    }
}

fn worker(
    config: NodeConfig,
    sink: EventSink,
    ready_tx: oneshot::Sender<Result<PrnsNodeHandle, String>>,
    shutdown_rx: oneshot::Receiver<()>,
    control_rx: mpsc::Receiver<ControlJob>,
    stopped_tx: oneshot::Sender<Exit>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready_tx.send(Err(format!("tokio runtime build failed: {error}")));
            let _ = stopped_tx.send(Exit::Crashed("runtimeBuildFailed".to_string()));
            return;
        }
    };
    let closing_sink = sink.clone();
    let exit = runtime.block_on(run_node(config, sink, ready_tx, shutdown_rx, control_rx));
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    match &exit {
        Exit::Stopped => closing_sink.node_stopped("stopped"),
        Exit::Crashed(reason) => closing_sink.node_stopped(reason),
    }
    let _ = stopped_tx.send(exit);
}

async fn run_node(
    config: NodeConfig,
    sink: EventSink,
    ready_tx: oneshot::Sender<Result<PrnsNodeHandle, String>>,
    mut shutdown_rx: oneshot::Receiver<()>,
    control_rx: mpsc::Receiver<ControlJob>,
) -> Exit {
    let aspects = aspect_refs(&config);
    let destinations = build_destinations(&config, &aspects);
    let mut hashes = Vec::with_capacity(destinations.len());
    for dest in &destinations {
        match dest.destination_hash() {
            Ok(hash) => hashes.push(hash),
            Err(error) => {
                let _ = ready_tx.send(Err(format!("invalid destination name: {error:?}")));
                return Exit::Stopped;
            }
        }
    }
    let failure_sink = sink.clone();
    let mut node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: config.transport_identity.clone(),
        pre_configured_destinations: destinations,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
        on_event: move |event, _state: &()| sink.dispatch(event),
    });
    let mut registration_failure: Option<String> = None;
    'register: for (dest, hash) in config.destinations.iter().zip(&hashes) {
        let Some(single) = &dest.single else {
            continue;
        };
        for (path, policy) in &single.request_paths {
            if let Err(error) = node.register_request_path(hash, path, *policy) {
                registration_failure = Some(format!(
                    "request path {path:?} registration failed: {error:?}"
                ));
                break 'register;
            }
        }
    }
    if let Some(reason) = registration_failure {
        let _ = ready_tx.send(Err(reason));
        return Exit::Stopped;
    }
    let handle = node.handle();
    tokio::spawn(control_loop(control_rx, handle.clone()));
    if ready_tx.send(Ok(handle)).is_err() {
        return Exit::Stopped;
    }
    tokio::select! {
        _ = &mut shutdown_rx => Exit::Stopped,
        () = failure_sink.wait_failed() => Exit::Crashed("eventBackpressureExceeded".to_string()),
        result = node.run() => match result {
            Ok(()) => Exit::Crashed("engineStopped".to_string()),
            Err(error) => Exit::Crashed(format!("{error}")),
        },
    }
}

async fn control_loop(mut control_rx: mpsc::Receiver<ControlJob>, handle: PrnsNodeHandle) {
    while let Some(job) = control_rx.recv().await {
        tokio::spawn(job(handle.clone()));
    }
}
