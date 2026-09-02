use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use personal_rns::prelude::{
    GrowableHeap, InitiateRemoteControlControllerPairing, ManuallyAttached,
    PreConfiguredDestination, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe,
    RemoteControlControllerPairingInitiationControl, RemoteControlPairingControl,
};
use personal_rns::remote_control::{
    RemoteControlInitialControllerGrants, RemoteControlPairingInvitationCode,
    RemoteControlSelfAnnouncement, RemoteControlService,
};
use personal_rns::runtime::{
    NodePersistence, RemoteControlIdentityDirectory, RemoteControlPairingControlError,
};
use tokio::sync::{mpsc, oneshot};

use crate::contract::{
    BluetoothState, DescribeRemoteControlTargetInput, DevelopmentNodeFailure,
    DevelopmentNodeFailureStage, DevelopmentNodeOperation, DevelopmentNodeOperationKind,
    DevelopmentNodeRuntime, DevelopmentNodeSnapshot, DevelopmentNodeStartOutcome,
    DevelopmentNodeStopOutcome, DevelopmentNodeStopStage, InitiateRemoteControlPairingInput,
    RemoteControlDescribeFailureStage, RemoteControlDescribeOutcome,
    RemoteControlPairingCommandOutcome, RemoteControlPairingDecisionInput,
    RemoteControlPairingFailureStage, RemoteControlPairingState, U64String,
};
use crate::node::{prepare_storage, reset_storage, NodeStoragePaths};
use crate::pairing::{
    apply_event, apply_overflow_failure, attempt_id_string, expire_candidate, send_event,
    AppliedNodeEvent, OwnedNodeEvent, PairingControls, EVENT_LANE_CAPACITY,
};
use crate::snapshot::SnapshotStore;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const STOP_TIMEOUT: Duration = Duration::from_secs(25);
const COMMAND_LANE_CAPACITY: usize = 8;

type ReadyResult = Result<DevelopmentNodeSnapshot, (DevelopmentNodeFailureStage, String)>;
type WorkerResult = Result<(), (DevelopmentNodeStopStage, String)>;

struct Supervisor {
    snapshots: Arc<SnapshotStore>,
    operation_admitted: Arc<AtomicBool>,
    state: Mutex<SupervisorState>,
}

#[derive(Default)]
struct SupervisorState {
    worker: Option<Worker>,
}

struct Worker {
    commands: mpsc::Sender<Command>,
    done: std_mpsc::Receiver<WorkerResult>,
    join: Option<JoinHandle<()>>,
    storage_root: PathBuf,
    stop_requested: bool,
    terminal_failure: Option<(DevelopmentNodeStopStage, String)>,
}

enum Command {
    Initiate(
        InitiateRemoteControlPairingInput,
        std_mpsc::SyncSender<RemoteControlPairingCommandOutcome>,
    ),
    Approve(
        RemoteControlPairingDecisionInput,
        std_mpsc::SyncSender<RemoteControlPairingCommandOutcome>,
    ),
    Reject(
        RemoteControlPairingDecisionInput,
        std_mpsc::SyncSender<RemoteControlPairingCommandOutcome>,
    ),
    Describe(
        DescribeRemoteControlTargetInput,
        std_mpsc::SyncSender<RemoteControlDescribeOutcome>,
    ),
    Shutdown,
}

enum BluetoothMonitor {
    #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
    Apple {
        status: personal_rns::bluetooth_auto::BluetoothAutoStatus,
        preparation_failure: Option<String>,
    },
    #[cfg(not(all(feature = "apple", any(target_os = "ios", target_os = "macos"))))]
    NotCompiled,
}

pub fn start(storage_root: &Path) -> DevelopmentNodeStartOutcome {
    let supervisor = supervisor();
    let mut state = supervisor.lock_state();
    if state.worker.is_some() {
        let snapshot = supervisor.snapshots.read();
        return match snapshot.runtime {
            DevelopmentNodeRuntime::Running => {
                DevelopmentNodeStartOutcome::AlreadyRunning { snapshot }
            }
            DevelopmentNodeRuntime::Starting
            | DevelopmentNodeRuntime::Stopping
            | DevelopmentNodeRuntime::Failed
            | DevelopmentNodeRuntime::Stopped => DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::Runtime,
                detail: "A native node generation still owns the process lifecycle.".to_owned(),
            },
        };
    }

    let paths = match prepare_storage(storage_root) {
        Ok(paths) => paths,
        Err(detail) => {
            supervisor.snapshots.fail(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Storage,
                detail: detail.clone(),
            });
            return DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::Storage,
                detail,
            };
        }
    };

    let initial_bluetooth = if cfg!(all(
        feature = "apple",
        any(target_os = "ios", target_os = "macos")
    )) {
        BluetoothState::Preparing
    } else {
        BluetoothState::NotCompiled
    };
    supervisor.snapshots.begin_generation(initial_bluetooth);
    supervisor
        .operation_admitted
        .store(false, Ordering::Release);

    let (ready_tx, ready_rx) = std_mpsc::sync_channel(1);
    let (done_tx, done_rx) = std_mpsc::sync_channel(1);
    let (command_tx, command_rx) = mpsc::channel(COMMAND_LANE_CAPACITY);
    let snapshots = Arc::clone(&supervisor.snapshots);
    let operation_admitted = Arc::clone(&supervisor.operation_admitted);
    let storage_for_worker = paths.root.clone();
    let join = std::thread::Builder::new()
        .name("prns-app-native".to_owned())
        .spawn(move || {
            let result = run_worker(
                paths,
                command_rx,
                snapshots,
                Arc::clone(&operation_admitted),
                ready_tx,
            );
            operation_admitted.store(false, Ordering::Release);
            let _ = done_tx.send(result);
        });
    let join = match join {
        Ok(join) => join,
        Err(error) => {
            let detail = format!("could not start the native node worker: {error}");
            supervisor.snapshots.fail(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Runtime,
                detail: detail.clone(),
            });
            return DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::Runtime,
                detail,
            };
        }
    };
    state.worker = Some(Worker {
        commands: command_tx,
        done: done_rx,
        join: Some(join),
        storage_root: storage_for_worker,
        stop_requested: false,
        terminal_failure: None,
    });

    match ready_rx.recv_timeout(STARTUP_TIMEOUT + Duration::from_secs(2)) {
        Ok(Ok(snapshot)) => DevelopmentNodeStartOutcome::Started { snapshot },
        Ok(Err((stage, detail))) => {
            finish_failed_start(&mut state, stage, detail.clone());
            DevelopmentNodeStartOutcome::Failed { stage, detail }
        }
        Err(std_mpsc::RecvTimeoutError::Timeout) => {
            let detail = "The native node did not restore persistence before the startup deadline."
                .to_owned();
            supervisor.snapshots.fail(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::PersistenceRestore,
                detail: detail.clone(),
            });
            if let Some(worker) = state.worker.as_mut() {
                let _ = worker.commands.try_send(Command::Shutdown);
                worker.stop_requested = true;
            }
            DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::PersistenceRestore,
                detail,
            }
        }
        Err(std_mpsc::RecvTimeoutError::Disconnected) => {
            let detail = "The native node worker exited before publishing readiness.".to_owned();
            finish_failed_start(
                &mut state,
                DevelopmentNodeFailureStage::Node,
                detail.clone(),
            );
            DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::Node,
                detail,
            }
        }
    }
}

#[must_use]
pub fn snapshot() -> DevelopmentNodeSnapshot {
    supervisor().snapshots.read()
}

pub fn initiate(input: InitiateRemoteControlPairingInput) -> RemoteControlPairingCommandOutcome {
    call_pairing(|response| Command::Initiate(input, response))
}

pub fn approve(input: RemoteControlPairingDecisionInput) -> RemoteControlPairingCommandOutcome {
    call_pairing(|response| Command::Approve(input, response))
}

pub fn reject(input: RemoteControlPairingDecisionInput) -> RemoteControlPairingCommandOutcome {
    call_pairing(|response| Command::Reject(input, response))
}

pub fn describe(input: DescribeRemoteControlTargetInput) -> RemoteControlDescribeOutcome {
    let Some(commands) = running_commands() else {
        return RemoteControlDescribeOutcome::Failed {
            stage: RemoteControlDescribeFailureStage::Node,
            detail: "The native Prns node is not running.".to_owned(),
        };
    };
    if supervisor()
        .operation_admitted
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return RemoteControlDescribeOutcome::Busy;
    }
    let (response_tx, response_rx) = std_mpsc::sync_channel(1);
    match commands.try_send(Command::Describe(input, response_tx)) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            supervisor()
                .operation_admitted
                .store(false, Ordering::Release);
            return RemoteControlDescribeOutcome::Busy;
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            supervisor()
                .operation_admitted
                .store(false, Ordering::Release);
            return RemoteControlDescribeOutcome::Failed {
                stage: RemoteControlDescribeFailureStage::Node,
                detail: "The native Prns node stopped before Describe was admitted.".to_owned(),
            };
        }
    }
    response_rx
        .recv_timeout(COMMAND_TIMEOUT)
        .unwrap_or_else(|_| RemoteControlDescribeOutcome::Failed {
            stage: RemoteControlDescribeFailureStage::Timeout,
            detail: "The native Describe command exceeded its bounded wait.".to_owned(),
        })
}

pub fn stop() -> DevelopmentNodeStopOutcome {
    let supervisor = supervisor();
    let mut state = supervisor.lock_state();
    let Some(worker) = state.worker.as_mut() else {
        return DevelopmentNodeStopOutcome::AlreadyStopped;
    };
    if let Some((stage, detail)) = &worker.terminal_failure {
        return DevelopmentNodeStopOutcome::Failed {
            stage: *stage,
            detail: detail.clone(),
        };
    }

    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Stopping);
    supervisor.snapshots.update(|snapshot| {
        snapshot.active_operation = Some(DevelopmentNodeOperation {
            kind: DevelopmentNodeOperationKind::Shutdown,
            started_at_millis: U64String::from(wall_clock_millis()),
        });
    });
    if !worker.stop_requested {
        match worker.commands.try_send(Command::Shutdown) {
            Ok(()) => worker.stop_requested = true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                return DevelopmentNodeStopOutcome::Failed {
                    stage: DevelopmentNodeStopStage::CommandAdmission,
                    detail: "The native command lane is busy; shutdown was not admitted."
                        .to_owned(),
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => worker.stop_requested = true,
        }
    }

    let result = worker.done.recv_timeout(STOP_TIMEOUT);
    match result {
        Ok(Ok(())) => {
            join_finished_worker(worker);
            state.worker = None;
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            supervisor.snapshots.stopped();
            DevelopmentNodeStopOutcome::Stopped
        }
        Ok(Err((stage, detail))) => {
            join_finished_worker(worker);
            worker.terminal_failure = Some((stage, detail.clone()));
            DevelopmentNodeStopOutcome::Failed { stage, detail }
        }
        Err(std_mpsc::RecvTimeoutError::Timeout) => DevelopmentNodeStopOutcome::Failed {
            stage: DevelopmentNodeStopStage::Worker,
            detail: "The native worker did not finish bounded shutdown.".to_owned(),
        },
        Err(std_mpsc::RecvTimeoutError::Disconnected) => {
            join_finished_worker(worker);
            let detail = "The native worker exited without a shutdown result.".to_owned();
            worker.terminal_failure = Some((DevelopmentNodeStopStage::Worker, detail.clone()));
            DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Worker,
                detail,
            }
        }
    }
}

pub fn reset(storage_root: &Path) -> DevelopmentNodeStopOutcome {
    let active_root = {
        let state = supervisor().lock_state();
        state
            .worker
            .as_ref()
            .map(|worker| worker.storage_root.clone())
    };
    if let Some(active_root) = active_root {
        let requested_root = storage_root
            .canonicalize()
            .unwrap_or_else(|_| storage_root.to_path_buf());
        if requested_root != active_root {
            return DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Persistence,
                detail: "Reset must name the private directory owned by the active generation."
                    .to_owned(),
            };
        }
    }
    let terminal_worker_reaped = {
        let mut state = supervisor().lock_state();
        let safe_to_clear = state
            .worker
            .as_ref()
            .is_some_and(|worker| worker.join.is_none() && worker.terminal_failure.is_some());
        if safe_to_clear {
            state.worker = None;
        }
        safe_to_clear
    };
    let stop_outcome = if terminal_worker_reaped {
        supervisor()
            .operation_admitted
            .store(false, Ordering::Release);
        DevelopmentNodeStopOutcome::Stopped
    } else {
        stop()
    };
    if !matches!(
        stop_outcome,
        DevelopmentNodeStopOutcome::Stopped | DevelopmentNodeStopOutcome::AlreadyStopped
    ) {
        return stop_outcome;
    }
    match reset_storage(storage_root) {
        Ok(()) => {
            supervisor().snapshots.stopped();
            stop_outcome
        }
        Err(detail) => DevelopmentNodeStopOutcome::Failed {
            stage: DevelopmentNodeStopStage::Persistence,
            detail,
        },
    }
}

fn supervisor() -> &'static Supervisor {
    static SUPERVISOR: OnceLock<Supervisor> = OnceLock::new();
    SUPERVISOR.get_or_init(|| Supervisor {
        snapshots: Arc::new(SnapshotStore::new()),
        operation_admitted: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(SupervisorState::default()),
    })
}

impl Supervisor {
    fn lock_state(&self) -> MutexGuard<'_, SupervisorState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn running_commands() -> Option<mpsc::Sender<Command>> {
    let supervisor = supervisor();
    if supervisor.snapshots.read().runtime != DevelopmentNodeRuntime::Running {
        return None;
    }
    supervisor
        .lock_state()
        .worker
        .as_ref()
        .map(|worker| worker.commands.clone())
}

fn call_pairing(
    command: impl FnOnce(std_mpsc::SyncSender<RemoteControlPairingCommandOutcome>) -> Command,
) -> RemoteControlPairingCommandOutcome {
    let Some(commands) = running_commands() else {
        return pairing_failed(
            RemoteControlPairingFailureStage::Node,
            "The native Prns node is not running.",
        );
    };
    if supervisor()
        .operation_admitted
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return RemoteControlPairingCommandOutcome::Busy;
    }
    let (response_tx, response_rx) = std_mpsc::sync_channel(1);
    match commands.try_send(command(response_tx)) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            supervisor()
                .operation_admitted
                .store(false, Ordering::Release);
            return RemoteControlPairingCommandOutcome::Busy;
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            supervisor()
                .operation_admitted
                .store(false, Ordering::Release);
            return pairing_failed(
                RemoteControlPairingFailureStage::Node,
                "The native Prns node stopped before the command was admitted.",
            );
        }
    }
    response_rx
        .recv_timeout(COMMAND_TIMEOUT)
        .unwrap_or_else(|_| {
            pairing_failed(
                RemoteControlPairingFailureStage::Node,
                "The native pairing command exceeded its bounded wait.",
            )
        })
}

fn finish_failed_start(
    state: &mut SupervisorState,
    stage: DevelopmentNodeFailureStage,
    detail: String,
) {
    let mut worker_finished = false;
    if let Some(worker) = state.worker.as_mut() {
        let _ = worker.commands.try_send(Command::Shutdown);
        worker.stop_requested = true;
        match worker.done.recv_timeout(STOP_TIMEOUT) {
            Ok(_) | Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                join_finished_worker(worker);
                worker_finished = true;
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
        }
    } else {
        worker_finished = true;
    }
    if worker_finished {
        state.worker = None;
    }
    let supervisor = supervisor();
    supervisor
        .operation_admitted
        .store(false, Ordering::Release);
    supervisor
        .snapshots
        .fail(DevelopmentNodeFailure { stage, detail });
}

fn join_finished_worker(worker: &mut Worker) {
    if let Some(join) = worker.join.take() {
        let _ = join.join();
    }
}

fn run_worker(
    paths: NodeStoragePaths,
    commands: mpsc::Receiver<Command>,
    snapshots: Arc<SnapshotStore>,
    operation_admitted: Arc<AtomicBool>,
    ready: std_mpsc::SyncSender<ReadyResult>,
) -> WorkerResult {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            let detail = format!("could not create the Tokio runtime: {error}");
            let _ = ready.send(Err((DevelopmentNodeFailureStage::Runtime, detail.clone())));
            (DevelopmentNodeStopStage::Worker, detail)
        })?;
    runtime.block_on(run_generation(
        paths,
        commands,
        snapshots,
        operation_admitted,
        ready,
    ))
}

async fn run_generation(
    paths: NodeStoragePaths,
    commands: mpsc::Receiver<Command>,
    snapshots: Arc<SnapshotStore>,
    operation_admitted: Arc<AtomicBool>,
    ready: std_mpsc::SyncSender<ReadyResult>,
) -> WorkerResult {
    let bootstrap = RemoteControlIdentityDirectory::new(&paths.remote_control_identities)
        .load_or_generate()
        .map_err(|error| {
            boot_failure(
                &ready,
                &snapshots,
                DevelopmentNodeFailureStage::Identity,
                format!("could not load the installation RemoteControl identities: {error}"),
            )
        })?;
    let controller_identity_fingerprint = bootstrap
        .secrets()
        .identities()
        .controller()
        .identity_hash()
        .as_bytes()
        .to_vec();
    let (identity_secrets, _) = bootstrap.into_parts();
    let bluetooth_identity = personal_rns::load_or_create_ble_identity(&paths.bluetooth_identity)
        .map_err(|error| {
        boot_failure(
            &ready,
            &snapshots,
            DevelopmentNodeFailureStage::Identity,
            format!("could not load the installation Bluetooth identity: {error}"),
        )
    })?;

    #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
    let (prepared_bluetooth, preparation_failure) =
        match personal_rns::bluetooth_auto::AutoBle::prepare(bluetooth_identity).await {
            Ok(prepared) => (prepared, None),
            Err(error) => (
                personal_rns::bluetooth_auto::AutoBle::unavailable(bluetooth_identity),
                Some(format!(
                    "CoreBluetooth manager preparation failed: {error:?}"
                )),
            ),
        };
    #[cfg(not(all(feature = "apple", any(target_os = "ios", target_os = "macos"))))]
    let _ = bluetooth_identity;

    let persistence = NodePersistence::custom_dir(&paths.network).map_err(|error| {
        boot_failure(
            &ready,
            &snapshots,
            DevelopmentNodeFailureStage::Storage,
            format!("could not open the upstream persistence owner: {error}"),
        )
    })?;
    let remote_control = RemoteControlService::new(
        identity_secrets,
        RemoteControlInitialControllerGrants::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    );
    let (event_tx, event_rx) = mpsc::channel(EVENT_LANE_CAPACITY);
    let overflowed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let event_overflowed = Arc::clone(&overflowed);
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: personal_rns::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence,
        on_event: move |event, _state: &()| send_event(&event_tx, &event_overflowed, event),
    });
    let handle = node.handle();
    let clock = node.clock();

    #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
    let bluetooth = {
        let attached = handle.attach(prepared_bluetooth);
        BluetoothMonitor::Apple {
            status: attached.status(),
            preparation_failure,
        }
    };
    #[cfg(not(all(feature = "apple", any(target_os = "ios", target_os = "macos"))))]
    let bluetooth = BluetoothMonitor::NotCompiled;

    snapshots.update(|snapshot| {
        snapshot.controller_identity_fingerprint = Some(controller_identity_fingerprint);
        snapshot.bluetooth = bluetooth.snapshot();
        if bluetooth.is_not_compiled() {
            snapshot.pairing = RemoteControlPairingState::BluetoothUnavailable;
        }
    });

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let node_run = node.run_until(async {
        let _ = shutdown_rx.await;
    });
    let actor = run_actor(
        handle,
        commands,
        event_rx,
        overflowed,
        bluetooth,
        clock,
        Arc::clone(&snapshots),
        Arc::clone(&operation_admitted),
        Arc::clone(&shutdown_requested),
        ready,
        shutdown_tx,
    );
    tokio::pin!(node_run);
    tokio::pin!(actor);
    let result = tokio::select! {
        actor_result = &mut actor => {
            let node_result = node_run.await.map_err(|error| map_node_run_error(
                error,
                "the Prns node failed during shutdown",
            ));
            actor_result.and(node_result)
        }
        node_result = &mut node_run => {
            node_result.map_err(|error| map_node_run_error(
                error,
                "the Prns node stopped unexpectedly",
            ))?;
            if shutdown_requested.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err((
                    DevelopmentNodeStopStage::Node,
                    "The Prns node stopped before lifecycle shutdown completed.".to_owned(),
                ))
            }
        }
    };
    settle_worker_result(&operation_admitted, &snapshots, &result);
    result
}

fn settle_worker_result(
    operation_admitted: &AtomicBool,
    snapshots: &SnapshotStore,
    result: &WorkerResult,
) {
    operation_admitted.store(false, Ordering::Release);
    if let Err((stage, detail)) = result {
        snapshots.update(|snapshot| {
            if *stage == DevelopmentNodeStopStage::Persistence
                && matches!(
                    snapshot.pairing,
                    RemoteControlPairingState::Persisting { .. }
                )
            {
                snapshot.pairing = RemoteControlPairingState::Failed {
                    stage: RemoteControlPairingFailureStage::Persistence,
                    detail: "The paired target authorization could not be persisted.".to_owned(),
                };
            }
            snapshot.runtime = DevelopmentNodeRuntime::Failed;
            snapshot.failure = Some(DevelopmentNodeFailure {
                stage: failure_stage_for_stop(*stage),
                detail: detail.clone(),
            });
            snapshot.active_operation = None;
        });
    }
}

const fn failure_stage_for_stop(stage: DevelopmentNodeStopStage) -> DevelopmentNodeFailureStage {
    match stage {
        DevelopmentNodeStopStage::Persistence => DevelopmentNodeFailureStage::PersistenceRestore,
        DevelopmentNodeStopStage::Worker => DevelopmentNodeFailureStage::Runtime,
        DevelopmentNodeStopStage::CommandAdmission
        | DevelopmentNodeStopStage::TargetConnection
        | DevelopmentNodeStopStage::Node => DevelopmentNodeFailureStage::Node,
    }
}

fn map_node_run_error(
    error: personal_rns::runtime::NodeRunError,
    context: &str,
) -> (DevelopmentNodeStopStage, String) {
    let stage = match error {
        personal_rns::runtime::NodeRunError::PersistenceFailed
        | personal_rns::runtime::NodeRunError::PersistenceWorkerStopped
        | personal_rns::runtime::NodeRunError::RemoteControlAuthorizationPersistenceFailed(_) => {
            DevelopmentNodeStopStage::Persistence
        }
        personal_rns::runtime::NodeRunError::ManifoldPanicked
        | personal_rns::runtime::NodeRunError::RequestEndpointrPanicked
        | personal_rns::runtime::NodeRunError::InterfaceDriverPanicked => {
            DevelopmentNodeStopStage::Node
        }
    };
    (stage, format!("{context}: {error}"))
}

#[allow(clippy::too_many_arguments)]
async fn run_actor(
    handle: PrnsNodeHandle,
    mut commands: mpsc::Receiver<Command>,
    mut events: mpsc::Receiver<OwnedNodeEvent>,
    overflowed: Arc<std::sync::atomic::AtomicBool>,
    bluetooth: BluetoothMonitor,
    clock: personal_rns::manifold::tokio::TokioClock,
    snapshots: Arc<SnapshotStore>,
    operation_admitted: Arc<AtomicBool>,
    shutdown_requested: Arc<AtomicBool>,
    ready: std_mpsc::SyncSender<ReadyResult>,
    shutdown: oneshot::Sender<()>,
) -> WorkerResult {
    let mut controls = PairingControls::default();
    let startup = async {
        loop {
            let Some(event) = events.recv().await else {
                return Err("The native event lane closed before persistence restoration.");
            };
            if apply_event(event, &mut controls, &snapshots)
                == AppliedNodeEvent::PersistenceRestored
            {
                return Ok(());
            }
        }
    };
    match tokio::time::timeout(STARTUP_TIMEOUT, startup).await {
        Ok(Ok(())) => {}
        Ok(Err(detail)) => {
            let detail = detail.to_owned();
            let _ = ready.send(Err((
                DevelopmentNodeFailureStage::PersistenceRestore,
                detail.clone(),
            )));
            shutdown_requested.store(true, Ordering::Release);
            let _ = shutdown.send(());
            return Err((DevelopmentNodeStopStage::Persistence, detail));
        }
        Err(_) => {
            let detail =
                "Prns did not publish PersistenceRestored before the startup deadline.".to_owned();
            let _ = ready.send(Err((
                DevelopmentNodeFailureStage::PersistenceRestore,
                detail.clone(),
            )));
            shutdown_requested.store(true, Ordering::Release);
            let _ = shutdown.send(());
            return Err((DevelopmentNodeStopStage::Persistence, detail));
        }
    }

    let _ = crate::remote_control::refresh_targets(&handle, &snapshots).await;
    snapshots.update(|snapshot| {
        snapshot.runtime = DevelopmentNodeRuntime::Running;
        snapshot.bluetooth = bluetooth.snapshot();
        snapshot.failure = None;
    });
    let _ = ready.send(Ok(snapshots.read()));

    let mut bluetooth_refresh = tokio::time::interval(Duration::from_millis(500));
    loop {
        if overflowed.swap(false, std::sync::atomic::Ordering::AcqRel) {
            apply_overflow_failure(&snapshots);
        }
        tokio::select! {
            _ = bluetooth_refresh.tick() => {
                snapshots.set_bluetooth(bluetooth.snapshot());
                expire_candidate(&mut controls, &snapshots, clock.now());
            }
            event = events.recv() => {
                match event {
                    Some(event) => {
                        if apply_event(event, &mut controls, &snapshots)
                            == AppliedNodeEvent::TargetInventoryChanged
                        {
                            if let Err(detail) = crate::remote_control::refresh_targets(&handle, &snapshots).await {
                                snapshots.fail(DevelopmentNodeFailure {
                                    stage: DevelopmentNodeFailureStage::Node,
                                    detail: detail.clone(),
                                });
                                shutdown_requested.store(true, Ordering::Release);
                                let _ = shutdown.send(());
                                return Err((DevelopmentNodeStopStage::Node, detail));
                            }
                        }
                    }
                    None => {
                        operation_admitted.store(false, Ordering::Release);
                        snapshots.fail(DevelopmentNodeFailure {
                            stage: DevelopmentNodeFailureStage::Node,
                            detail: "The native event lane closed unexpectedly.".to_owned(),
                        });
                        shutdown_requested.store(true, Ordering::Release);
                        let _ = shutdown.send(());
                        return Err((
                            DevelopmentNodeStopStage::Node,
                            "The native actor lost its Prns event producer.".to_owned(),
                        ));
                    }
                }
            }
            command = commands.recv() => match command {
                Some(Command::Initiate(input, response)) => {
                    let outcome = initiate_pairing(&handle, &mut controls, &snapshots, input).await;
                    operation_admitted.store(false, Ordering::Release);
                    let _ = response.send(outcome);
                }
                Some(Command::Approve(input, response)) => {
                    let outcome = approve_pairing(&handle, &mut controls, &snapshots, input).await;
                    operation_admitted.store(false, Ordering::Release);
                    let _ = response.send(outcome);
                }
                Some(Command::Reject(input, response)) => {
                    let outcome = reject_pairing(&handle, &mut controls, &snapshots, input).await;
                    operation_admitted.store(false, Ordering::Release);
                    let _ = response.send(outcome);
                }
                Some(Command::Describe(input, response)) => {
                    let outcome = crate::remote_control::describe(&handle, &snapshots, input).await;
                    operation_admitted.store(false, Ordering::Release);
                    let _ = response.send(outcome);
                }
                Some(Command::Shutdown) | None => {
                    commands.close();
                    snapshots.set_runtime(DevelopmentNodeRuntime::Stopping);
                    shutdown_requested.store(true, Ordering::Release);
                    let _ = shutdown.send(());
                    return Ok(());
                }
            }
        }
    }
}

async fn initiate_pairing(
    handle: &PrnsNodeHandle,
    controls: &mut PairingControls,
    snapshots: &SnapshotStore,
    input: InitiateRemoteControlPairingInput,
) -> RemoteControlPairingCommandOutcome {
    let invitation_code = match parse_invitation_code(&input.invitation_code) {
        Some(code) => code,
        None => {
            return pairing_failed(
                RemoteControlPairingFailureStage::Input,
                "The invitation code must be exactly eight uppercase hexadecimal characters.",
            )
        }
    };
    let Some(candidate) = controls.candidate.as_ref() else {
        return pairing_failed(
            RemoteControlPairingFailureStage::Candidate,
            "No current signed pairing candidate is retained.",
        );
    };
    if candidate.candidate_id != input.candidate_id {
        return pairing_failed(
            RemoteControlPairingFailureStage::Candidate,
            "The pairing candidate is stale or does not match the retained observation.",
        );
    }
    let endpoint = candidate.endpoint;
    let expires_at = candidate.expires_at;
    let retained_candidate = snapshots.read().pairing;
    controls.retain_initiation(input.candidate_id.clone());
    snapshots.update(|snapshot| {
        snapshot.active_operation = Some(DevelopmentNodeOperation {
            kind: DevelopmentNodeOperationKind::Pairing,
            started_at_millis: U64String::from(wall_clock_millis()),
        });
        snapshot.pairing = RemoteControlPairingState::InvitationSubmitted {
            candidate_id: input.candidate_id,
        };
    });
    let result = handle
        .initiate_remote_control_controller_pairing(InitiateRemoteControlControllerPairing {
            endpoint,
            invitation_code,
            expires_at,
        })
        .await;
    match result {
        Ok(received) => {
            let attempt_id = match received.admission {
                personal_rns::engine::AdmitRemoteControlControllerPairingResponseOutcome::Offer(
                    personal_rns::remote_control::ReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired {
                        attempt_id,
                    },
                ) => attempt_id,
                _ => {
                    return pairing_failed_visible(
                        controls,
                        snapshots,
                        RemoteControlPairingFailureStage::Confirmation,
                        "The upstream pairing response advanced without the required confirmation attempt.",
                    );
                }
            };
            controls.retain_attempt(attempt_id_string(attempt_id));
            RemoteControlPairingCommandOutcome::Accepted {
                snapshot: Box::new(snapshots.read()),
            }
        }
        Err(personal_rns::runtime::InitiateRemoteControlControllerPairingError::Busy {
            ..
        }) => {
            controls.cancel_initiation();
            snapshots.update(|snapshot| {
                snapshot.pairing = retained_candidate;
                snapshot.active_operation = None;
            });
            RemoteControlPairingCommandOutcome::Busy
        }
        Err(error) => {
            let stage = match error {
                personal_rns::runtime::InitiateRemoteControlControllerPairingError::EstablishLink(_) => RemoteControlPairingFailureStage::Link,
                personal_rns::runtime::InitiateRemoteControlControllerPairingError::Identify { .. } => RemoteControlPairingFailureStage::Identification,
                personal_rns::runtime::InitiateRemoteControlControllerPairingError::ResponseExpired { .. } => RemoteControlPairingFailureStage::Expired,
                personal_rns::runtime::InitiateRemoteControlControllerPairingError::Request { .. }
                | personal_rns::runtime::InitiateRemoteControlControllerPairingError::ResponseNotAdvanced { .. } => RemoteControlPairingFailureStage::Request,
                personal_rns::runtime::InitiateRemoteControlControllerPairingError::Begin { .. } => RemoteControlPairingFailureStage::Confirmation,
                personal_rns::runtime::InitiateRemoteControlControllerPairingError::NodeStopped { .. }
                | personal_rns::runtime::InitiateRemoteControlControllerPairingError::Busy { .. } => RemoteControlPairingFailureStage::Node,
            };
            pairing_failed_visible(
                controls,
                snapshots,
                stage,
                "The upstream RemoteControl pairing initiation failed.",
            )
        }
    }
}

async fn approve_pairing(
    handle: &PrnsNodeHandle,
    controls: &mut PairingControls,
    snapshots: &SnapshotStore,
    input: RemoteControlPairingDecisionInput,
) -> RemoteControlPairingCommandOutcome {
    let Some(confirmation) = controls.confirmation.as_ref() else {
        return pairing_failed(
            RemoteControlPairingFailureStage::Confirmation,
            "No RemoteControl confirmation is awaiting approval.",
        );
    };
    let expected = attempt_id_string(confirmation.confirmation().attempt_id());
    if input.attempt_id != expected {
        return pairing_failed(
            RemoteControlPairingFailureStage::Confirmation,
            "The approval does not match the retained pairing attempt.",
        );
    }
    let approval = confirmation.approval();
    let retained_confirmation = snapshots.read().pairing;
    snapshots.update(|snapshot| {
        snapshot.pairing = RemoteControlPairingState::AwaitingTargetApproval {
            attempt_id: input.attempt_id.clone(),
        };
    });
    match handle
        .approve_remote_control_controller_pairing(approval)
        .await
    {
        Ok(_) => {
            snapshots.update(|snapshot| {
                snapshot.pairing = RemoteControlPairingState::Persisting {
                    attempt_id: input.attempt_id,
                };
            });
            RemoteControlPairingCommandOutcome::Accepted {
                snapshot: Box::new(snapshots.read()),
            }
        }
        Err(RemoteControlPairingControlError::Busy) => {
            snapshots.update(|snapshot| snapshot.pairing = retained_confirmation);
            RemoteControlPairingCommandOutcome::Busy
        }
        Err(RemoteControlPairingControlError::NodeStopped) => pairing_failed_visible(
            controls,
            snapshots,
            RemoteControlPairingFailureStage::Node,
            "The Prns node stopped while approving the pairing attempt.",
        ),
        Err(RemoteControlPairingControlError::Failed(_)) => pairing_failed_visible(
            controls,
            snapshots,
            RemoteControlPairingFailureStage::Confirmation,
            "The upstream RemoteControl approval failed.",
        ),
    }
}

async fn reject_pairing(
    handle: &PrnsNodeHandle,
    controls: &mut PairingControls,
    snapshots: &SnapshotStore,
    input: RemoteControlPairingDecisionInput,
) -> RemoteControlPairingCommandOutcome {
    let Some(confirmation) = controls.confirmation.as_ref() else {
        return pairing_failed(
            RemoteControlPairingFailureStage::Confirmation,
            "No RemoteControl confirmation is awaiting rejection.",
        );
    };
    let expected = attempt_id_string(confirmation.confirmation().attempt_id());
    if input.attempt_id != expected {
        return pairing_failed(
            RemoteControlPairingFailureStage::Confirmation,
            "The rejection does not match the retained pairing attempt.",
        );
    }
    let rejection = confirmation.rejection();
    match handle
        .reject_remote_control_controller_pairing(rejection)
        .await
    {
        Ok(_) => {
            controls.clear_attempt();
            snapshots.update(|snapshot| {
                snapshot.pairing = RemoteControlPairingState::Rejected {
                    detail: "The controller rejected the pairing confirmation.".to_owned(),
                };
                snapshot.active_operation = None;
            });
            RemoteControlPairingCommandOutcome::Accepted {
                snapshot: Box::new(snapshots.read()),
            }
        }
        Err(RemoteControlPairingControlError::Busy) => RemoteControlPairingCommandOutcome::Busy,
        Err(RemoteControlPairingControlError::NodeStopped) => pairing_failed_visible(
            controls,
            snapshots,
            RemoteControlPairingFailureStage::Node,
            "The Prns node stopped while rejecting the pairing attempt.",
        ),
        Err(RemoteControlPairingControlError::Failed(_)) => pairing_failed_visible(
            controls,
            snapshots,
            RemoteControlPairingFailureStage::Confirmation,
            "The upstream RemoteControl rejection failed.",
        ),
    }
}

impl BluetoothMonitor {
    const fn is_not_compiled(&self) -> bool {
        #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
        {
            false
        }
        #[cfg(not(all(feature = "apple", any(target_os = "ios", target_os = "macos"))))]
        {
            matches!(self, Self::NotCompiled)
        }
    }

    fn snapshot(&self) -> BluetoothState {
        match self {
            #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
            Self::Apple {
                status,
                preparation_failure,
            } => {
                use personal_rns::interfaces::{ConnectionState, InterfaceStatus};
                match status.connection() {
                    ConnectionState::Connected => BluetoothState::Ready,
                    ConnectionState::Degraded | ConnectionState::Reconnecting => {
                        BluetoothState::Degraded {
                            detail: "Bluetooth Auto is reconnecting or recently lost a peer."
                                .to_owned(),
                        }
                    }
                    ConnectionState::Failed => BluetoothState::Unavailable {
                        detail: preparation_failure.clone().unwrap_or_else(|| {
                            "Bluetooth Auto reported an unavailable platform backend.".to_owned()
                        }),
                    },
                    ConnectionState::Disabled => BluetoothState::Disabled,
                    ConnectionState::Initializing => BluetoothState::Preparing,
                    ConnectionState::Disconnected | ConnectionState::Unknown => {
                        BluetoothState::Ready
                    }
                }
            }
            #[cfg(not(all(feature = "apple", any(target_os = "ios", target_os = "macos"))))]
            Self::NotCompiled => BluetoothState::NotCompiled,
        }
    }
}

fn parse_invitation_code(value: &str) -> Option<RemoteControlPairingInvitationCode> {
    if value.len() != 8
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
    {
        return None;
    }
    u32::from_str_radix(value, 16)
        .ok()
        .map(RemoteControlPairingInvitationCode::from_value)
}

fn pairing_failed(
    stage: RemoteControlPairingFailureStage,
    detail: &str,
) -> RemoteControlPairingCommandOutcome {
    RemoteControlPairingCommandOutcome::Failed {
        stage,
        detail: detail.to_owned(),
    }
}

fn pairing_failed_visible(
    controls: &mut PairingControls,
    snapshots: &SnapshotStore,
    stage: RemoteControlPairingFailureStage,
    detail: &str,
) -> RemoteControlPairingCommandOutcome {
    controls.clear_attempt();
    snapshots.update(|snapshot| {
        snapshot.pairing = RemoteControlPairingState::Failed {
            stage,
            detail: detail.to_owned(),
        };
        snapshot.active_operation = None;
    });
    pairing_failed(stage, detail)
}

fn boot_failure(
    ready: &std_mpsc::SyncSender<ReadyResult>,
    snapshots: &SnapshotStore,
    stage: DevelopmentNodeFailureStage,
    detail: String,
) -> (DevelopmentNodeStopStage, String) {
    snapshots.fail(DevelopmentNodeFailure {
        stage,
        detail: detail.clone(),
    });
    let _ = ready.send(Err((stage, detail.clone())));
    let stop_stage = match stage {
        DevelopmentNodeFailureStage::Storage | DevelopmentNodeFailureStage::PersistenceRestore => {
            DevelopmentNodeStopStage::Persistence
        }
        DevelopmentNodeFailureStage::Runtime => DevelopmentNodeStopStage::Worker,
        DevelopmentNodeFailureStage::Identity
        | DevelopmentNodeFailureStage::Bluetooth
        | DevelopmentNodeFailureStage::Node
        | DevelopmentNodeFailureStage::Contract => DevelopmentNodeStopStage::Node,
    };
    (stop_stage, detail)
}

fn wall_clock_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_active_pairing(snapshots: &SnapshotStore, pairing: RemoteControlPairingState) {
        snapshots.update(|snapshot| {
            snapshot.runtime = DevelopmentNodeRuntime::Running;
            snapshot.pairing = pairing;
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::Pairing,
                started_at_millis: U64String::from(7),
            });
        });
    }

    #[test]
    fn invitation_input_is_exact_uppercase_hex() {
        assert!(parse_invitation_code("00000000").is_some());
        assert!(parse_invitation_code("89ABCDEF").is_some());
        assert!(parse_invitation_code("89abcdef").is_none());
        assert!(parse_invitation_code("123456").is_none());
        assert!(parse_invitation_code("123456789").is_none());
    }

    #[test]
    fn fatal_pairing_commands_clear_control_state_and_project_the_failure() {
        let endpoint = personal_rns::remote_control::RemoteControlPairingIdentity::new(
            personal_rns::identity::IdentityHash::new([0x42; 16]),
        )
        .endpoint();
        let mut controls = PairingControls {
            candidate: Some(crate::pairing::PairingCandidateControl {
                candidate_id: "candidate".to_owned(),
                endpoint,
                expires_at: personal_rns::units::InstantMillis(10),
            }),
            initiated_candidate_id: Some("candidate".to_owned()),
            active_attempt_id: Some("attempt".to_owned()),
            ..PairingControls::default()
        };
        let snapshots = SnapshotStore::new();
        seed_active_pairing(
            &snapshots,
            RemoteControlPairingState::InvitationSubmitted {
                candidate_id: "candidate".to_owned(),
            },
        );

        let outcome = pairing_failed_visible(
            &mut controls,
            &snapshots,
            RemoteControlPairingFailureStage::Link,
            "link failed",
        );

        assert_eq!(
            outcome,
            RemoteControlPairingCommandOutcome::Failed {
                stage: RemoteControlPairingFailureStage::Link,
                detail: "link failed".to_owned(),
            }
        );
        assert!(controls.candidate.is_none());
        assert!(controls.confirmation.is_none());
        assert!(controls.initiated_candidate_id.is_none());
        assert!(controls.active_attempt_id.is_none());
        let snapshot = snapshots.read();
        assert_eq!(
            snapshot.pairing,
            RemoteControlPairingState::Failed {
                stage: RemoteControlPairingFailureStage::Link,
                detail: "link failed".to_owned(),
            }
        );
        assert!(snapshot.active_operation.is_none());
    }

    #[test]
    fn persistence_terminal_failure_projects_pairing_and_worker_failure() {
        let snapshots = SnapshotStore::new();
        seed_active_pairing(
            &snapshots,
            RemoteControlPairingState::Persisting {
                attempt_id: "attempt".to_owned(),
            },
        );
        let admitted = AtomicBool::new(true);
        let result = Err((
            DevelopmentNodeStopStage::Persistence,
            "authorization flush failed".to_owned(),
        ));

        settle_worker_result(&admitted, &snapshots, &result);

        assert!(!admitted.load(Ordering::Acquire));
        let snapshot = snapshots.read();
        assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Failed);
        assert_eq!(
            snapshot.pairing,
            RemoteControlPairingState::Failed {
                stage: RemoteControlPairingFailureStage::Persistence,
                detail: "The paired target authorization could not be persisted.".to_owned(),
            }
        );
        assert_eq!(
            snapshot.failure,
            Some(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::PersistenceRestore,
                detail: "authorization flush failed".to_owned(),
            })
        );
        assert!(snapshot.active_operation.is_none());
    }

    #[test]
    fn non_persistence_worker_failure_is_terminal_without_rewriting_pairing() {
        let snapshots = SnapshotStore::new();
        let pairing = RemoteControlPairingState::ConfirmationRequired {
            attempt_id: "attempt".to_owned(),
            confirmation_code: "123456".to_owned(),
            target_identity_fingerprint: vec![0x42; 16],
            permissions: vec![crate::contract::RemoteControlRequestKind::Describe],
        };
        seed_active_pairing(&snapshots, pairing.clone());
        let admitted = AtomicBool::new(true);
        let result = Err((DevelopmentNodeStopStage::Node, "node stopped".to_owned()));

        settle_worker_result(&admitted, &snapshots, &result);

        assert!(!admitted.load(Ordering::Acquire));
        let snapshot = snapshots.read();
        assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Failed);
        assert_eq!(snapshot.pairing, pairing);
        assert_eq!(
            snapshot.failure,
            Some(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Node,
                detail: "node stopped".to_owned(),
            })
        );
        assert!(snapshot.active_operation.is_none());
    }

    #[test]
    fn a_node_generation_stops_and_restarts_with_the_same_private_state() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");

        assert!(matches!(
            start(&storage),
            DevelopmentNodeStartOutcome::Started { .. }
        ));
        assert_eq!(snapshot().runtime, DevelopmentNodeRuntime::Running);
        assert_eq!(stop(), DevelopmentNodeStopOutcome::Stopped);
        assert!(matches!(
            start(&storage),
            DevelopmentNodeStartOutcome::Started { .. }
        ));
        assert_eq!(stop(), DevelopmentNodeStopOutcome::Stopped);
        assert!(matches!(
            reset(&storage),
            DevelopmentNodeStopOutcome::AlreadyStopped
        ));
        assert!(!storage.exists());
    }
}
