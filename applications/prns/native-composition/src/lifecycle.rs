use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use personal_rns::node_introspection::DestinationIdentityQuery;
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
    LocalIdentityFileError, NodePersistence, RemoteControlIdentityDirectory,
    RemoteControlPairingControlError,
};
use personal_rns::wire::DestinationHash;
use prns_core::identity::vault::{FileVault, FileVaultError, IdentityLabel, IdentityVault};
use prns_core::identity::PrivateIdentityMaterial;
use prns_host::{BackendInfo, BackendKind, Capability, InterfaceKind, PersistenceSnapshot};
use prns_host_snapshot::{assemble_host_snapshot, HostInterfaceAttachment};
use tokio::sync::{mpsc, watch};

use crate::contract::{
    ContactDestinationInput, ContactListOutcome, ContactLookupOutcome, ContactMutationOutcome,
    CreateManualContactInput, DescribeRemoteControlTargetInput, DevelopmentNodeFailure,
    DevelopmentNodeFailureStage, DevelopmentNodeOperation, DevelopmentNodeOperationKind,
    DevelopmentNodeRuntime, DevelopmentNodeSnapshot, DevelopmentNodeStartOutcome,
    DevelopmentNodeStopOutcome, DevelopmentNodeStopStage, IdentityCreationOutcome,
    IdentityImportPreviewOutcome, InitiateRemoteControlPairingInput, LocalHostState,
    PrimaryIdentityState, RemoteControlDescribeFailureStage, RemoteControlDescribeOutcome,
    RemoteControlPairingCommandOutcome, RemoteControlPairingDecisionInput,
    RemoteControlPairingFailureStage, RemoteControlPairingState, SetContactAliasInput,
    SetContactPinnedInput, U64String,
};
use crate::development_store::{DevelopmentStoreFailure, DevelopmentStoreOwner, StoreReply};
use crate::directory::{DirectoryRequest, DirectoryResponse};
use crate::node::{prepare_storage, reset_storage, NodeStoragePaths};
use crate::pairing::{
    apply_event, apply_overflow_failure, apply_persistence_event, attempt_id_string,
    expire_candidate, send_event, AppliedNodeEvent, OwnedNodeEvent, PairingControls,
    EVENT_LANE_CAPACITY,
};
use crate::snapshot::SnapshotStore;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const HOST_INSPECTION_TIMEOUT: Duration = Duration::from_millis(1_500);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(2);
const DIRECTORY_TIMEOUT: Duration = Duration::from_secs(5);
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
    application_owner: Option<DevelopmentStoreOwner>,
    identity_owner: Option<IdentityOwner>,
}

struct IdentityOwner {
    root: PathBuf,
    vault: FileVault,
}

struct Worker {
    commands: mpsc::Sender<Command>,
    shutdown: ShutdownSignal,
    done: std_mpsc::Receiver<WorkerResult>,
    join: Option<JoinHandle<()>>,
    storage_root: PathBuf,
}

#[derive(Clone)]
struct ShutdownSignal {
    sender: watch::Sender<bool>,
    requested: Arc<AtomicBool>,
}

impl ShutdownSignal {
    fn request(&self) {
        self.requested.store(true, Ordering::Release);
        let _ = self.sender.send(true);
    }

    fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

enum Command {
    Snapshot(std_mpsc::SyncSender<DevelopmentNodeSnapshot>),
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
    ObservedIdentity(
        [u8; 16],
        std_mpsc::SyncSender<Result<Option<[u8; 16]>, String>>,
    ),
}

enum HostAttachment {
    #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
    Apple {
        attached: personal_rns::bluetooth_auto::AttachedBle,
    },
    #[cfg(not(all(feature = "apple", any(target_os = "ios", target_os = "macos"))))]
    None,
}

#[must_use]
pub fn inspect_identity(storage_root: &Path) -> PrimaryIdentityState {
    let supervisor = supervisor();
    let mut state = supervisor.lock_state();
    let result = inspect_identity_locked(&mut state, storage_root);
    supervisor.snapshots.set_primary_identity(result.clone());
    result
}

#[must_use]
pub fn preview_identity_import(bytes: &[u8]) -> IdentityImportPreviewOutcome {
    let Ok(material) = PrivateIdentityMaterial::from_slice(bytes) else {
        return IdentityImportPreviewOutcome::InvalidLength;
    };
    IdentityImportPreviewOutcome::Valid {
        identity_hash: material.identity_hash().as_bytes().to_vec(),
    }
}

pub fn create_generated_identity(storage_root: &Path) -> IdentityCreationOutcome {
    create_identity(storage_root, IdentityCreation::Generate)
}

pub fn create_imported_identity(storage_root: &Path, bytes: &[u8]) -> IdentityCreationOutcome {
    create_identity(storage_root, IdentityCreation::Import(bytes))
}

enum IdentityCreation<'a> {
    Generate,
    Import(&'a [u8]),
}

fn create_identity(storage_root: &Path, creation: IdentityCreation<'_>) -> IdentityCreationOutcome {
    let supervisor = supervisor();
    let mut state = supervisor.lock_state();
    let paths = match prepare_storage(storage_root) {
        Ok(paths) => paths,
        Err(detail) => return IdentityCreationOutcome::Unavailable { detail },
    };
    let vault = match identity_vault(&mut state, &paths) {
        Ok(vault) => vault,
        Err(state) => return creation_failure(state),
    };
    let label = match primary_label() {
        Ok(label) => label,
        Err(state) => return creation_failure(state),
    };
    match vault.load(&label) {
        Ok(Some(_)) => IdentityCreationOutcome::AlreadyExists,
        Ok(None) => {
            let material = match creation {
                IdentityCreation::Generate => match personal_rns::try_generate_identity_secret() {
                    Ok(secret) => PrivateIdentityMaterial::from(secret),
                    Err(error) => {
                        return IdentityCreationOutcome::Unavailable {
                            detail: format!("could not generate identity material: {error}"),
                        }
                    }
                },
                IdentityCreation::Import(bytes) => {
                    let Ok(material) = PrivateIdentityMaterial::from_slice(bytes) else {
                        return IdentityCreationOutcome::InvalidLength;
                    };
                    material
                }
            };
            match vault.store(&label, material.as_bytes()) {
                Ok(()) => {
                    let identity_hash = material.identity_hash().as_bytes().to_vec();
                    let primary = PrimaryIdentityState::Present {
                        identity_hash: identity_hash.clone(),
                    };
                    supervisor.snapshots.set_primary_identity(primary);
                    IdentityCreationOutcome::Created { identity_hash }
                }
                Err(error) => creation_failure(primary_vault_failure(error)),
            }
        }
        Err(error) => creation_failure(primary_vault_failure(error)),
    }
}

fn inspect_identity_locked(
    state: &mut SupervisorState,
    storage_root: &Path,
) -> PrimaryIdentityState {
    let paths = match prepare_storage(storage_root) {
        Ok(paths) => paths,
        Err(detail) => return PrimaryIdentityState::Unavailable { detail },
    };
    let vault = match identity_vault(state, &paths) {
        Ok(vault) => vault,
        Err(state) => return state,
    };
    let label = match primary_label() {
        Ok(label) => label,
        Err(state) => return state,
    };
    match vault.load(&label) {
        Ok(Some(secret)) => {
            let material = PrivateIdentityMaterial::from(secret);
            PrimaryIdentityState::Present {
                identity_hash: material.identity_hash().as_bytes().to_vec(),
            }
        }
        Ok(None) => PrimaryIdentityState::Missing,
        Err(error) => primary_vault_failure(error),
    }
}

fn identity_vault<'a>(
    state: &'a mut SupervisorState,
    paths: &NodeStoragePaths,
) -> Result<&'a mut FileVault, PrimaryIdentityState> {
    if state
        .application_owner
        .as_ref()
        .is_some_and(|owner| owner.root != paths.root)
    {
        if state.worker.is_some() {
            return Err(PrimaryIdentityState::Unavailable {
                detail: "the active native aggregate owns a different development root".to_owned(),
            });
        }
        if let Some(owner) = state.application_owner.take() {
            owner.close().map_err(|failure| match failure {
                DevelopmentStoreFailure::Unavailable(detail) => {
                    PrimaryIdentityState::Unavailable { detail }
                }
                DevelopmentStoreFailure::ResetRequired(reason) => {
                    PrimaryIdentityState::DevelopmentResetRequired { reason }
                }
            })?;
        }
    }
    if state
        .identity_owner
        .as_ref()
        .is_some_and(|owner| owner.root != paths.root)
    {
        if state.worker.is_some() {
            return Err(PrimaryIdentityState::Unavailable {
                detail: "the active native generation owns a different development root".to_owned(),
            });
        }
        state.identity_owner = None;
    }
    let owner = state.identity_owner.get_or_insert_with(|| IdentityOwner {
        root: paths.root.clone(),
        vault: FileVault::new(&paths.identities),
    });
    Ok(&mut owner.vault)
}

fn primary_label() -> Result<IdentityLabel, PrimaryIdentityState> {
    IdentityLabel::new("primary").map_err(|_| PrimaryIdentityState::Unavailable {
        detail: "the built-in primary identity label is invalid".to_owned(),
    })
}

fn primary_vault_failure(error: FileVaultError) -> PrimaryIdentityState {
    match error {
        FileVaultError::MalformedLength { found } => {
            PrimaryIdentityState::DevelopmentResetRequired {
                reason: format!("primary identity holds {found} bytes instead of 64"),
            }
        }
        FileVaultError::BlobOutgrewBuffer {
            blob_len,
            buffer_len,
        } => PrimaryIdentityState::DevelopmentResetRequired {
            reason: format!(
                "primary identity blob holds {blob_len} bytes but its buffer holds {buffer_len}"
            ),
        },
        FileVaultError::Io(error) => PrimaryIdentityState::Unavailable {
            detail: format!("could not access the primary identity: {error}"),
        },
    }
}

fn creation_failure(state: PrimaryIdentityState) -> IdentityCreationOutcome {
    match state {
        PrimaryIdentityState::DevelopmentResetRequired { reason } => {
            IdentityCreationOutcome::DevelopmentResetRequired { reason }
        }
        PrimaryIdentityState::Unavailable { detail } => {
            IdentityCreationOutcome::Unavailable { detail }
        }
        PrimaryIdentityState::Missing | PrimaryIdentityState::Present { .. } => {
            IdentityCreationOutcome::Unavailable {
                detail: "identity creation reached an inconsistent state".to_owned(),
            }
        }
    }
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

    let primary_identity = inspect_identity_locked(&mut state, storage_root);
    supervisor
        .snapshots
        .set_primary_identity(primary_identity.clone());
    let identity_failure = match &primary_identity {
        PrimaryIdentityState::Present { .. } => None,
        PrimaryIdentityState::Missing => {
            Some("Create or import a primary identity before starting the local node.".to_owned())
        }
        PrimaryIdentityState::Unavailable { detail } => Some(detail.clone()),
        PrimaryIdentityState::DevelopmentResetRequired { reason } => Some(reason.clone()),
    };
    if let Some(detail) = identity_failure {
        supervisor.snapshots.fail(DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::Identity,
            detail: detail.clone(),
        });
        return DevelopmentNodeStartOutcome::Failed {
            stage: DevelopmentNodeFailureStage::Identity,
            detail,
        };
    }
    supervisor.snapshots.begin_generation(primary_identity);
    supervisor
        .operation_admitted
        .store(false, Ordering::Release);

    let (ready_tx, ready_rx) = std_mpsc::sync_channel(1);
    let (done_tx, done_rx) = std_mpsc::sync_channel(1);
    let (command_tx, command_rx) = mpsc::channel(COMMAND_LANE_CAPACITY);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let shutdown = ShutdownSignal {
        sender: shutdown_tx,
        requested: Arc::new(AtomicBool::new(false)),
    };
    let snapshots = Arc::clone(&supervisor.snapshots);
    let operation_admitted = Arc::clone(&supervisor.operation_admitted);
    let storage_for_worker = paths.root.clone();
    let worker_shutdown = shutdown.clone();
    let join = std::thread::Builder::new()
        .name("prns-app-native".to_owned())
        .spawn(move || {
            let result = run_worker(
                paths,
                command_rx,
                worker_shutdown,
                shutdown_rx,
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
        shutdown,
        done: done_rx,
        join: Some(join),
        storage_root: storage_for_worker,
    });

    match ready_rx.recv_timeout(STARTUP_TIMEOUT + Duration::from_secs(2)) {
        Ok(Ok(snapshot)) => DevelopmentNodeStartOutcome::Started { snapshot },
        Ok(Err((stage, detail))) => {
            finish_failed_start(supervisor, &mut state, stage, detail.clone());
            DevelopmentNodeStartOutcome::Failed { stage, detail }
        }
        Err(std_mpsc::RecvTimeoutError::Timeout) => {
            let detail = "The native node did not restore persistence before the startup deadline."
                .to_owned();
            finish_failed_start(
                supervisor,
                &mut state,
                DevelopmentNodeFailureStage::PersistenceRestore,
                detail.clone(),
            );
            DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::PersistenceRestore,
                detail,
            }
        }
        Err(std_mpsc::RecvTimeoutError::Disconnected) => {
            let detail = "The native node worker exited before publishing readiness.".to_owned();
            finish_failed_start(
                supervisor,
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
    snapshot_with_supervisor(supervisor())
}

fn snapshot_with_supervisor(supervisor: &Supervisor) -> DevelopmentNodeSnapshot {
    let state = supervisor.lock_state();
    let current = supervisor.snapshots.read();
    if current.runtime != DevelopmentNodeRuntime::Running {
        return current;
    }
    let Some(worker) = state.worker.as_ref() else {
        return current;
    };
    let (response_tx, response_rx) = std_mpsc::sync_channel(1);
    if worker
        .commands
        .try_send(Command::Snapshot(response_tx))
        .is_err()
    {
        supervisor.snapshots.set_local_host_unavailable_if_running(
            "The local Host snapshot command could not be admitted.".to_owned(),
        );
        return supervisor.snapshots.read();
    }
    match response_rx.recv_timeout(SNAPSHOT_TIMEOUT) {
        Ok(snapshot) => snapshot,
        Err(_) => {
            supervisor.snapshots.set_local_host_unavailable_if_running(
                "The local Host snapshot command exceeded its bounded wait.".to_owned(),
            );
            supervisor.snapshots.read()
        }
    }
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

pub fn save_observed_destination(
    storage_root: &Path,
    input: ContactDestinationInput,
) -> ContactMutationOutcome {
    save_observed_destination_with_supervisor(supervisor(), storage_root, input)
}

fn save_observed_destination_with_supervisor(
    supervisor: &Supervisor,
    storage_root: &Path,
    input: ContactDestinationInput,
) -> ContactMutationOutcome {
    let state = supervisor.lock_state();
    if supervisor.snapshots.read().runtime != DevelopmentNodeRuntime::Running {
        return ContactMutationOutcome::LocalNodeStopped;
    }
    let Some(worker) = state.worker.as_ref() else {
        return ContactMutationOutcome::LocalNodeStopped;
    };
    let paths = match prepare_storage(storage_root) {
        Ok(paths) => paths,
        Err(detail) => return ContactMutationOutcome::DevelopmentUnavailable { detail },
    };
    if paths.root != worker.storage_root {
        return ContactMutationOutcome::DevelopmentUnavailable {
            detail: "The running native generation owns a different development root.".to_owned(),
        };
    }
    let generation_commands = worker.commands.clone();
    let (identity_tx, identity_rx) = std_mpsc::sync_channel(1);
    match generation_commands.try_send(Command::ObservedIdentity(input.destination, identity_tx)) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            return ContactMutationOutcome::DevelopmentUnavailable {
                detail: "The local observation command lane is full.".to_owned(),
            };
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            return ContactMutationOutcome::DevelopmentUnavailable {
                detail: "The local node stopped before the observation was admitted.".to_owned(),
            };
        }
    }
    drop(state);
    let identity = match identity_rx.recv_timeout(HOST_INSPECTION_TIMEOUT + Duration::from_secs(1))
    {
        Ok(Ok(Some(identity))) => identity,
        Ok(Ok(None)) => return ContactMutationOutcome::NotObserved,
        Ok(Err(detail)) => {
            return ContactMutationOutcome::DevelopmentUnavailable { detail };
        }
        Err(_) => {
            return ContactMutationOutcome::DevelopmentUnavailable {
                detail: "The local observation command exceeded its bounded wait.".to_owned(),
            };
        }
    };
    let mut state = supervisor.lock_state();
    let same_generation = supervisor.snapshots.read().runtime == DevelopmentNodeRuntime::Running
        && state.worker.as_ref().is_some_and(|worker| {
            worker.storage_root == paths.root && worker.commands.same_channel(&generation_commands)
        });
    if !same_generation {
        return ContactMutationOutcome::DevelopmentUnavailable {
            detail: "The observed association belongs to a native generation that has stopped."
                .to_owned(),
        };
    }
    let response = match admit_directory_locked(
        &mut state,
        &paths,
        DirectoryRequest::SaveObserved {
            destination: input.destination,
            identity,
        },
    ) {
        Ok(response) => response,
        Err(failure) => return mutation_store_failure(failure),
    };
    drop(state);
    receive_mutation(response)
}

pub fn create_manual_contact(
    storage_root: &Path,
    input: CreateManualContactInput,
) -> ContactMutationOutcome {
    call_contact_mutation(
        storage_root,
        DirectoryRequest::CreateManual {
            destination: input.destination,
            identity: input.identity,
            alias: input.alias,
        },
    )
}

pub fn set_contact_alias(
    storage_root: &Path,
    input: SetContactAliasInput,
) -> ContactMutationOutcome {
    call_contact_mutation(
        storage_root,
        DirectoryRequest::SetAlias {
            destination: input.destination,
            alias: input.alias,
        },
    )
}

pub fn set_contact_pinned(
    storage_root: &Path,
    input: SetContactPinnedInput,
) -> ContactMutationOutcome {
    call_contact_mutation(
        storage_root,
        DirectoryRequest::SetPinned {
            destination: input.destination,
            pinned: input.pinned,
        },
    )
}

pub fn delete_contact(
    storage_root: &Path,
    input: ContactDestinationInput,
) -> ContactMutationOutcome {
    call_contact_mutation(
        storage_root,
        DirectoryRequest::Delete {
            destination: input.destination,
        },
    )
}

pub fn get_contact(storage_root: &Path, input: ContactDestinationInput) -> ContactLookupOutcome {
    let response = match admit_directory(
        storage_root,
        DirectoryRequest::Get {
            destination: input.destination,
        },
    ) {
        Ok(response) => response,
        Err(failure) => return lookup_store_failure(failure),
    };
    match response.recv_timeout(DIRECTORY_TIMEOUT) {
        Ok(Ok(DirectoryResponse::Lookup(outcome))) => outcome,
        Ok(Ok(_)) => ContactLookupOutcome::DevelopmentUnavailable {
            detail: "The development database returned an unexpected contact result.".to_owned(),
        },
        Ok(Err(failure)) => lookup_store_failure(failure),
        Err(_) => ContactLookupOutcome::DevelopmentUnavailable {
            detail: "The development contact lookup exceeded its bounded wait.".to_owned(),
        },
    }
}

pub fn list_contacts(storage_root: &Path) -> ContactListOutcome {
    let response = match admit_directory(storage_root, DirectoryRequest::List) {
        Ok(response) => response,
        Err(failure) => return list_store_failure(failure),
    };
    match response.recv_timeout(DIRECTORY_TIMEOUT) {
        Ok(Ok(DirectoryResponse::List(outcome))) => outcome,
        Ok(Ok(_)) => ContactListOutcome::DevelopmentUnavailable {
            detail: "The development database returned an unexpected contact result.".to_owned(),
        },
        Ok(Err(failure)) => list_store_failure(failure),
        Err(_) => ContactListOutcome::DevelopmentUnavailable {
            detail: "The development contact list exceeded its bounded wait.".to_owned(),
        },
    }
}

fn call_contact_mutation(storage_root: &Path, request: DirectoryRequest) -> ContactMutationOutcome {
    let response = match admit_directory(storage_root, request) {
        Ok(response) => response,
        Err(failure) => return mutation_store_failure(failure),
    };
    receive_mutation(response)
}

fn receive_mutation(response: std_mpsc::Receiver<StoreReply>) -> ContactMutationOutcome {
    match response.recv_timeout(DIRECTORY_TIMEOUT) {
        Ok(Ok(DirectoryResponse::Mutation(outcome))) => outcome,
        Ok(Ok(_)) => ContactMutationOutcome::DevelopmentUnavailable {
            detail: "The development database returned an unexpected contact result.".to_owned(),
        },
        Ok(Err(failure)) => mutation_store_failure(failure),
        Err(_) => ContactMutationOutcome::DevelopmentUnavailable {
            detail: "The development contact mutation exceeded its bounded wait.".to_owned(),
        },
    }
}

fn admit_directory(
    storage_root: &Path,
    request: DirectoryRequest,
) -> Result<std_mpsc::Receiver<StoreReply>, DevelopmentStoreFailure> {
    let supervisor = supervisor();
    let mut state = supervisor.lock_state();
    let paths = prepare_storage(storage_root).map_err(DevelopmentStoreFailure::unavailable)?;
    admit_directory_locked(&mut state, &paths, request)
}

fn admit_directory_locked(
    state: &mut SupervisorState,
    paths: &NodeStoragePaths,
    request: DirectoryRequest,
) -> Result<std_mpsc::Receiver<StoreReply>, DevelopmentStoreFailure> {
    if state
        .worker
        .as_ref()
        .is_some_and(|worker| worker.storage_root != paths.root)
    {
        return Err(DevelopmentStoreFailure::unavailable(
            "the active native generation owns a different development root",
        ));
    }
    if state
        .identity_owner
        .as_ref()
        .is_some_and(|owner| owner.root != paths.root)
    {
        state.identity_owner = None;
    }
    if state
        .application_owner
        .as_ref()
        .is_some_and(|owner| owner.root != paths.root)
    {
        if state.worker.is_some() {
            return Err(DevelopmentStoreFailure::unavailable(
                "the active development database owns a different development root",
            ));
        }
        if let Some(owner) = state.application_owner.take() {
            owner.close()?;
        }
    }
    if state.application_owner.is_none() {
        state.application_owner = Some(DevelopmentStoreOwner::open(
            &paths.root,
            &paths.application,
        )?);
    }
    state
        .application_owner
        .as_ref()
        .ok_or_else(|| {
            DevelopmentStoreFailure::unavailable(
                "the development database owner was not initialized",
            )
        })?
        .admit(request)
}

fn mutation_store_failure(failure: DevelopmentStoreFailure) -> ContactMutationOutcome {
    match failure {
        DevelopmentStoreFailure::Unavailable(detail) => {
            ContactMutationOutcome::DevelopmentUnavailable { detail }
        }
        DevelopmentStoreFailure::ResetRequired(reason) => {
            ContactMutationOutcome::DevelopmentResetRequired { reason }
        }
    }
}

fn lookup_store_failure(failure: DevelopmentStoreFailure) -> ContactLookupOutcome {
    match failure {
        DevelopmentStoreFailure::Unavailable(detail) => {
            ContactLookupOutcome::DevelopmentUnavailable { detail }
        }
        DevelopmentStoreFailure::ResetRequired(reason) => {
            ContactLookupOutcome::DevelopmentResetRequired { reason }
        }
    }
}

fn list_store_failure(failure: DevelopmentStoreFailure) -> ContactListOutcome {
    match failure {
        DevelopmentStoreFailure::Unavailable(detail) => {
            ContactListOutcome::DevelopmentUnavailable { detail }
        }
        DevelopmentStoreFailure::ResetRequired(reason) => {
            ContactListOutcome::DevelopmentResetRequired { reason }
        }
    }
}

pub fn stop() -> DevelopmentNodeStopOutcome {
    let supervisor = supervisor();
    let mut state = supervisor.lock_state();
    stop_locked(supervisor, &mut state)
}

fn stop_locked(supervisor: &Supervisor, state: &mut SupervisorState) -> DevelopmentNodeStopOutcome {
    let Some(worker) = state.worker.as_mut() else {
        return DevelopmentNodeStopOutcome::AlreadyStopped;
    };

    supervisor
        .snapshots
        .set_runtime(DevelopmentNodeRuntime::Stopping);
    supervisor.snapshots.update(|snapshot| {
        snapshot.active_operation = Some(DevelopmentNodeOperation {
            kind: DevelopmentNodeOperationKind::Shutdown,
            started_at_millis: U64String::from(wall_clock_millis()),
        });
    });
    request_worker_shutdown(worker);

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
            state.worker = None;
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            supervisor.snapshots.fail(DevelopmentNodeFailure {
                stage: failure_stage_for_stop(stage),
                detail: detail.clone(),
            });
            DevelopmentNodeStopOutcome::Failed { stage, detail }
        }
        Err(std_mpsc::RecvTimeoutError::Timeout) => {
            let detail = "The native worker did not finish bounded shutdown.".to_owned();
            supervisor.snapshots.fail(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Runtime,
                detail: detail.clone(),
            });
            DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Worker,
                detail,
            }
        }
        Err(std_mpsc::RecvTimeoutError::Disconnected) => {
            join_finished_worker(worker);
            let detail = "The native worker exited without a shutdown result.".to_owned();
            state.worker = None;
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            supervisor.snapshots.fail(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Runtime,
                detail: detail.clone(),
            });
            DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Worker,
                detail,
            }
        }
    }
}

fn request_worker_shutdown(worker: &Worker) {
    worker.shutdown.request();
}

pub fn reset(storage_root: &Path) -> DevelopmentNodeStopOutcome {
    let supervisor = supervisor();
    let mut state = supervisor.lock_state();
    let owned_roots = [
        state.worker.as_ref().map(|worker| &worker.storage_root),
        state.application_owner.as_ref().map(|owner| &owner.root),
        state.identity_owner.as_ref().map(|owner| &owner.root),
    ];
    if owned_roots.into_iter().flatten().next().is_some() {
        let requested_root = storage_root
            .canonicalize()
            .unwrap_or_else(|_| storage_root.to_path_buf());
        if owned_roots
            .into_iter()
            .flatten()
            .any(|owned_root| requested_root != *owned_root)
        {
            return DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Persistence,
                detail: "Reset must name the private directory owned by the native aggregate."
                    .to_owned(),
            };
        }
    }
    let stop_outcome = stop_locked(supervisor, &mut state);
    if !matches!(
        stop_outcome,
        DevelopmentNodeStopOutcome::Stopped | DevelopmentNodeStopOutcome::AlreadyStopped
    ) && state.worker.is_some()
    {
        return stop_outcome;
    }
    let reset_outcome = match stop_outcome {
        DevelopmentNodeStopOutcome::Failed { .. } => DevelopmentNodeStopOutcome::Stopped,
        outcome => outcome,
    };
    if let Some(owner) = state.application_owner.take() {
        if let Err(failure) = owner.close() {
            let detail = match failure {
                DevelopmentStoreFailure::Unavailable(detail) => detail,
                DevelopmentStoreFailure::ResetRequired(reason) => reason,
            };
            return DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Persistence,
                detail,
            };
        }
    }
    state.identity_owner = None;
    match reset_storage(storage_root) {
        Ok(()) => {
            supervisor.snapshots.reset();
            reset_outcome
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
    supervisor: &Supervisor,
    state: &mut SupervisorState,
    stage: DevelopmentNodeFailureStage,
    detail: String,
) {
    let mut worker_finished = false;
    if let Some(worker) = state.worker.as_mut() {
        request_worker_shutdown(worker);
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
    shutdown: ShutdownSignal,
    shutdown_rx: watch::Receiver<bool>,
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
        shutdown,
        shutdown_rx,
        snapshots,
        operation_admitted,
        ready,
    ))
}

async fn run_generation(
    paths: NodeStoragePaths,
    commands: mpsc::Receiver<Command>,
    shutdown: ShutdownSignal,
    mut shutdown_rx: watch::Receiver<bool>,
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
        let detail = format!("could not load the installation Bluetooth identity: {error}");
        if matches!(
            error,
            LocalIdentityFileError::Malformed { .. }
                | LocalIdentityFileError::EmptyBleIdentity
                | LocalIdentityFileError::InvalidBleIdentity(_)
        ) {
            snapshots.set_local_host(LocalHostState::DevelopmentResetRequired {
                reason: detail.clone(),
            });
        }
        boot_failure(
            &ready,
            &snapshots,
            DevelopmentNodeFailureStage::Identity,
            detail,
        )
    })?;

    #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
    let prepared_bluetooth =
        match personal_rns::bluetooth_auto::AutoBle::prepare(bluetooth_identity).await {
            Ok(prepared) => prepared,
            Err(_) => personal_rns::bluetooth_auto::AutoBle::unavailable(bluetooth_identity),
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
    let host_attachment = HostAttachment::Apple {
        attached: handle.attach(prepared_bluetooth),
    };
    #[cfg(not(all(feature = "apple", any(target_os = "ios", target_os = "macos"))))]
    let host_attachment = HostAttachment::None;

    snapshots.update(|snapshot| {
        snapshot.controller_identity_fingerprint = Some(controller_identity_fingerprint);
        if host_attachment.is_none() {
            snapshot.pairing = RemoteControlPairingState::BluetoothUnavailable;
        }
    });

    let node_run = node.run_until(async {
        if !*shutdown_rx.borrow() {
            let _ = shutdown_rx.changed().await;
        }
    });
    let actor = run_actor(
        handle,
        commands,
        event_rx,
        overflowed,
        host_attachment,
        clock,
        Arc::clone(&snapshots),
        Arc::clone(&operation_admitted),
        ready,
        shutdown.clone(),
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
            if shutdown.is_requested() {
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
            if !matches!(
                &snapshot.local_host,
                LocalHostState::DevelopmentResetRequired { .. }
            ) {
                snapshot.local_host = LocalHostState::Stopped {
                    last_start_failure: Some(detail.clone()),
                };
            }
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
    host_attachment: HostAttachment,
    clock: personal_rns::manifold::tokio::TokioClock,
    snapshots: Arc<SnapshotStore>,
    operation_admitted: Arc<AtomicBool>,
    ready: std_mpsc::SyncSender<ReadyResult>,
    shutdown: ShutdownSignal,
) -> WorkerResult {
    let mut controls = PairingControls::default();
    let mut persistence = PersistenceSnapshot::persistent();
    let started = Instant::now();
    let mut host_revision = 0_u64;
    let startup = async {
        loop {
            let Some(event) = events.recv().await else {
                return Err("The native event lane closed before persistence restoration.");
            };
            apply_persistence_event(&event, &mut persistence);
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
            shutdown.request();
            return Err((DevelopmentNodeStopStage::Persistence, detail));
        }
        Err(_) => {
            let detail =
                "Prns did not publish PersistenceRestored before the startup deadline.".to_owned();
            let _ = ready.send(Err((
                DevelopmentNodeFailureStage::PersistenceRestore,
                detail.clone(),
            )));
            shutdown.request();
            return Err((DevelopmentNodeStopStage::Persistence, detail));
        }
    }

    let _ = crate::remote_control::refresh_targets(&handle, &snapshots).await;
    snapshots.update(|snapshot| {
        snapshot.runtime = DevelopmentNodeRuntime::Running;
        snapshot.failure = None;
    });
    refresh_host_snapshot(
        &handle,
        &host_attachment,
        &persistence,
        &mut host_revision,
        started,
        &snapshots,
    )
    .await;
    let _ = ready.send(Ok(snapshots.read()));

    let mut candidate_expiry = tokio::time::interval(Duration::from_millis(500));
    loop {
        if overflowed.swap(false, std::sync::atomic::Ordering::AcqRel) {
            apply_overflow_failure(&snapshots);
        }
        tokio::select! {
            _ = candidate_expiry.tick() => {
                expire_candidate(&mut controls, &snapshots, clock.now());
            }
            event = events.recv() => {
                match event {
                    Some(event) => {
                        apply_persistence_event(&event, &mut persistence);
                        if apply_event(event, &mut controls, &snapshots)
                            == AppliedNodeEvent::TargetInventoryChanged
                        {
                            if let Err(detail) = crate::remote_control::refresh_targets(&handle, &snapshots).await {
                                snapshots.fail(DevelopmentNodeFailure {
                                    stage: DevelopmentNodeFailureStage::Node,
                                    detail: detail.clone(),
                                });
                                shutdown.request();
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
                        shutdown.request();
                        return Err((
                            DevelopmentNodeStopStage::Node,
                            "The native actor lost its Prns event producer.".to_owned(),
                        ));
                    }
                }
            }
            command = commands.recv() => match command {
                Some(Command::Snapshot(response)) => {
                    refresh_host_snapshot(
                        &handle,
                        &host_attachment,
                        &persistence,
                        &mut host_revision,
                        started,
                        &snapshots,
                    ).await;
                    let _ = response.send(snapshots.read());
                }
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
                Some(Command::ObservedIdentity(destination, response)) => {
                    let identity = tokio::time::timeout(
                        HOST_INSPECTION_TIMEOUT,
                        handle.destination_identity(DestinationIdentityQuery::Destination(
                            DestinationHash::new(destination),
                        )),
                    )
                    .await
                    .map(|snapshot| snapshot.map(|snapshot| *snapshot.identity.as_bytes()))
                    .map_err(|_| {
                        "The local destination-identity query exceeded its bounded wait."
                            .to_owned()
                    });
                    let _ = response.send(identity);
                }
                None => {
                    commands.close();
                    snapshots.set_runtime(DevelopmentNodeRuntime::Stopping);
                    shutdown.request();
                    return Ok(());
                }
            }
        }
    }
}

async fn refresh_host_snapshot(
    handle: &PrnsNodeHandle,
    attachment: &HostAttachment,
    persistence: &PersistenceSnapshot,
    revision: &mut u64,
    started: Instant,
    snapshots: &SnapshotStore,
) {
    let interfaces = handle.interface_inventory();
    let engine =
        match tokio::time::timeout(HOST_INSPECTION_TIMEOUT, handle.engine_inspection_snapshot())
            .await
        {
            Ok(Some(engine)) => engine,
            Ok(None) => {
                snapshots.set_local_host_unavailable_if_running(
                    "The local node inspection lane is unavailable.".to_owned(),
                );
                return;
            }
            Err(_) => {
                snapshots.set_local_host_unavailable_if_running(
                    "The local node inspection lane exceeded its bounded wait.".to_owned(),
                );
                return;
            }
        };
    *revision = revision.saturating_add(1);
    let host = assemble_host_snapshot(
        interfaces,
        attachment.metadata(),
        engine,
        BackendInfo::new(
            BackendKind::Native,
            [Capability::Bluetooth],
            [InterfaceKind::AutomaticBluetoothLe],
        ),
        persistence.clone(),
        *revision,
        started.elapsed(),
    );
    snapshots.set_local_host(LocalHostState::Running {
        host: Box::new(host),
    });
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

impl HostAttachment {
    const fn is_none(&self) -> bool {
        #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
        {
            false
        }
        #[cfg(not(all(feature = "apple", any(target_os = "ios", target_os = "macos"))))]
        {
            matches!(self, Self::None)
        }
    }

    fn metadata(&self) -> Vec<HostInterfaceAttachment> {
        match self {
            #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
            Self::Apple { attached } => vec![HostInterfaceAttachment::new(
                attached.id(),
                InterfaceKind::AutomaticBluetoothLe,
            )],
            #[cfg(not(all(feature = "apple", any(target_os = "ios", target_os = "macos"))))]
            Self::None => Vec::new(),
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

    fn running_host_state() -> LocalHostState {
        LocalHostState::Running {
            host: Box::new(prns_host::HostSnapshot {
                revision: 1,
                backend: BackendInfo::new(
                    BackendKind::Native,
                    [Capability::Bluetooth],
                    [InterfaceKind::AutomaticBluetoothLe],
                ),
                interfaces: Vec::new(),
                routes: Vec::new(),
                active_link_count: 0,
                destination_identities: Vec::new(),
                runtime: prns_host::RuntimeHealthSnapshot {
                    running: true,
                    uptime_millis: 1,
                    interface_count: 0,
                    online_interface_count: 0,
                    route_count: 0,
                    link_count: 0,
                    transported_link_count: 0,
                    rx_bytes: 0,
                    tx_bytes: 0,
                    rx_bps: 0,
                    tx_bps: 0,
                },
                persistence: PersistenceSnapshot::persistent(),
            }),
        }
    }

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

    fn host_revision(snapshot: &DevelopmentNodeSnapshot) -> u64 {
        match &snapshot.local_host {
            LocalHostState::Running { host } => host.revision,
            LocalHostState::Stopped { .. }
            | LocalHostState::Unavailable { .. }
            | LocalHostState::DevelopmentResetRequired { .. } => 0,
        }
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
        snapshots.set_local_host(running_host_state());
        let admitted = AtomicBool::new(true);
        let result = Err((DevelopmentNodeStopStage::Node, "node stopped".to_owned()));

        settle_worker_result(&admitted, &snapshots, &result);

        assert!(!admitted.load(Ordering::Acquire));
        let snapshot = snapshots.read();
        assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Failed);
        assert_eq!(
            snapshot.local_host,
            LocalHostState::Stopped {
                last_start_failure: Some("node stopped".to_owned()),
            }
        );
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
    fn terminal_failure_preserves_a_reset_required_host() {
        let snapshots = SnapshotStore::new();
        snapshots.set_local_host(LocalHostState::DevelopmentResetRequired {
            reason: "malformed persisted Bluetooth identity".to_owned(),
        });

        settle_worker_result(
            &AtomicBool::new(true),
            &snapshots,
            &Err((DevelopmentNodeStopStage::Node, "node stopped".to_owned())),
        );

        assert!(matches!(
            snapshots.read().local_host,
            LocalHostState::DevelopmentResetRequired { .. }
        ));
    }

    #[test]
    fn failed_start_cleanup_joins_and_clears_a_shutdown_responsive_worker() {
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(true)),
            state: Mutex::new(SupervisorState::default()),
        };
        let (commands, _command_rx) = mpsc::channel(1);
        let (shutdown_sender, mut shutdown_rx) = watch::channel(false);
        let shutdown = ShutdownSignal {
            sender: shutdown_sender,
            requested: Arc::new(AtomicBool::new(false)),
        };
        let (done_tx, done) = std_mpsc::sync_channel(1);
        let join = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime");
            runtime.block_on(async {
                shutdown_rx.changed().await.expect("shutdown signal");
                assert!(*shutdown_rx.borrow());
            });
            let _ = done_tx.send(Ok(()));
        });
        let mut state = SupervisorState {
            worker: Some(Worker {
                commands,
                shutdown,
                done,
                join: Some(join),
                storage_root: PathBuf::from("/tmp/prns/development"),
            }),
            application_owner: None,
            identity_owner: None,
        };

        finish_failed_start(
            &supervisor,
            &mut state,
            DevelopmentNodeFailureStage::PersistenceRestore,
            "readiness stalled".to_owned(),
        );

        assert!(state.worker.is_none());
        assert!(!supervisor.operation_admitted.load(Ordering::Acquire));
        assert_eq!(
            supervisor.snapshots.read().failure,
            Some(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::PersistenceRestore,
                detail: "readiness stalled".to_owned(),
            })
        );
    }

    #[test]
    fn snapshot_failure_settles_before_a_concurrent_lifecycle_reset() {
        let supervisor = Arc::new(Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(SupervisorState::default()),
        });
        supervisor
            .snapshots
            .set_runtime(DevelopmentNodeRuntime::Running);
        supervisor.snapshots.set_local_host(running_host_state());

        let (commands, mut command_rx) = mpsc::channel(1);
        let (shutdown_sender, _shutdown_rx) = watch::channel(false);
        let shutdown = ShutdownSignal {
            sender: shutdown_sender,
            requested: Arc::new(AtomicBool::new(false)),
        };
        let (done_tx, done) = std_mpsc::sync_channel(1);
        let (inspection_started_tx, inspection_started_rx) = std_mpsc::sync_channel(1);
        let (release_inspection_tx, release_inspection_rx) = std_mpsc::sync_channel(1);
        let join = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime");
            runtime.block_on(async {
                let Some(Command::Snapshot(response)) = command_rx.recv().await else {
                    panic!("snapshot command was not admitted");
                };
                inspection_started_tx
                    .send(())
                    .expect("publish inspection start");
                release_inspection_rx
                    .recv()
                    .expect("release stalled inspection");
                drop(response);
            });
            let _ = done_tx.send(Ok(()));
        });
        supervisor.lock_state().worker = Some(Worker {
            commands,
            shutdown,
            done,
            join: Some(join),
            storage_root: PathBuf::from("/tmp/prns/development"),
        });

        let snapshot_supervisor = Arc::clone(&supervisor);
        let snapshot_thread =
            std::thread::spawn(move || snapshot_with_supervisor(&snapshot_supervisor));
        inspection_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("snapshot inspection started");

        let reset_supervisor = Arc::clone(&supervisor);
        let (reset_done_tx, reset_done_rx) = std_mpsc::sync_channel(1);
        let reset_thread = std::thread::spawn(move || {
            let mut state = reset_supervisor.lock_state();
            let worker = state.worker.as_mut().expect("active worker");
            assert!(matches!(
                worker.done.recv_timeout(Duration::from_secs(1)),
                Ok(Ok(()))
            ));
            join_finished_worker(worker);
            state.worker = None;
            reset_supervisor.snapshots.reset();
            reset_done_tx.send(()).expect("publish reset completion");
        });

        assert!(matches!(
            reset_done_rx.recv_timeout(Duration::from_millis(25)),
            Err(std_mpsc::RecvTimeoutError::Timeout)
        ));
        release_inspection_tx
            .send(())
            .expect("release stalled inspection");
        let snapshot = snapshot_thread.join().expect("snapshot thread");
        assert!(matches!(
            snapshot.local_host,
            LocalHostState::Unavailable { .. }
        ));
        reset_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("reset completed");
        reset_thread.join().expect("reset thread");
        assert_eq!(
            supervisor.snapshots.read().runtime,
            DevelopmentNodeRuntime::Stopped
        );
        assert!(matches!(
            supervisor.snapshots.read().local_host,
            LocalHostState::Stopped { .. }
        ));
    }

    #[test]
    fn observed_save_rechecks_generation_after_waiting_outside_the_supervisor_lock() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");
        let paths = prepare_storage(&storage).expect("private storage");
        let supervisor = Arc::new(Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(SupervisorState::default()),
        });
        supervisor
            .snapshots
            .set_runtime(DevelopmentNodeRuntime::Running);

        let (old_commands, mut old_command_rx) = mpsc::channel(1);
        let (query_started_tx, query_started_rx) = std_mpsc::sync_channel(1);
        let (release_query_tx, release_query_rx) = std_mpsc::sync_channel(1);
        let actor = std::thread::spawn(move || {
            let Some(Command::ObservedIdentity(destination, response)) =
                old_command_rx.blocking_recv()
            else {
                panic!("observed identity command was not admitted");
            };
            assert_eq!(destination, [4; 16]);
            query_started_tx.send(()).expect("publish query start");
            release_query_rx.recv().expect("release identity response");
            response
                .send(Ok(Some([5; 16])))
                .expect("publish identity response");
        });
        let (shutdown_sender, _shutdown_rx) = watch::channel(false);
        let (_done_tx, done) = std_mpsc::sync_channel(1);
        supervisor.lock_state().worker = Some(Worker {
            commands: old_commands,
            shutdown: ShutdownSignal {
                sender: shutdown_sender,
                requested: Arc::new(AtomicBool::new(false)),
            },
            done,
            join: None,
            storage_root: paths.root.clone(),
        });

        let (result_tx, result_rx) = std_mpsc::sync_channel(1);
        let save_supervisor = Arc::clone(&supervisor);
        let save_storage = storage.clone();
        let save = std::thread::spawn(move || {
            result_tx
                .send(save_observed_destination_with_supervisor(
                    &save_supervisor,
                    &save_storage,
                    ContactDestinationInput {
                        destination: [4; 16],
                    },
                ))
                .expect("publish save result");
        });
        query_started_rx.recv().expect("identity query started");

        let (new_commands, _new_command_rx) = mpsc::channel(1);
        let (new_shutdown_sender, _new_shutdown_rx) = watch::channel(false);
        let (_new_done_tx, new_done) = std_mpsc::sync_channel(1);
        supervisor.lock_state().worker = Some(Worker {
            commands: new_commands,
            shutdown: ShutdownSignal {
                sender: new_shutdown_sender,
                requested: Arc::new(AtomicBool::new(false)),
            },
            done: new_done,
            join: None,
            storage_root: paths.root,
        });
        release_query_tx.send(()).expect("release identity query");

        assert!(matches!(
            result_rx.recv().expect("save result"),
            ContactMutationOutcome::DevelopmentUnavailable { .. }
        ));
        assert!(supervisor.lock_state().application_owner.is_none());
        assert!(!storage.join("application.redb").exists());
        save.join().expect("save thread");
        actor.join().expect("actor thread");
    }

    #[test]
    fn observed_save_persists_the_generation_identity_and_reopens() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");
        let paths = prepare_storage(&storage).expect("private storage");
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(SupervisorState::default()),
        };
        supervisor
            .snapshots
            .set_runtime(DevelopmentNodeRuntime::Running);

        let destination = [0x31; 16];
        let identity = [0x42; 16];
        let (commands, mut command_rx) = mpsc::channel(1);
        let actor = std::thread::spawn(move || {
            let Some(Command::ObservedIdentity(observed_destination, response)) =
                command_rx.blocking_recv()
            else {
                panic!("observed identity command was not admitted");
            };
            assert_eq!(observed_destination, destination);
            response
                .send(Ok(Some(identity)))
                .expect("publish identity response");
        });
        let (shutdown_sender, _shutdown_rx) = watch::channel(false);
        let (_done_tx, done) = std_mpsc::sync_channel(1);
        supervisor.lock_state().worker = Some(Worker {
            commands,
            shutdown: ShutdownSignal {
                sender: shutdown_sender,
                requested: Arc::new(AtomicBool::new(false)),
            },
            done,
            join: None,
            storage_root: paths.root.clone(),
        });

        assert!(matches!(
            save_observed_destination_with_supervisor(
                &supervisor,
                &storage,
                ContactDestinationInput { destination },
            ),
            ContactMutationOutcome::Saved { contact }
                if contact.destination == destination
                    && contact.identity == Some(identity)
                    && contact.alias.is_none()
                    && !contact.pinned
        ));
        actor.join().expect("actor thread");

        let owner = supervisor
            .lock_state()
            .application_owner
            .take()
            .expect("save opened the application database");
        owner.close().expect("close application database");
        let reopened = DevelopmentStoreOwner::open(&paths.root, &paths.application)
            .expect("reopen application database");
        let reply = reopened
            .admit(DirectoryRequest::Get { destination })
            .expect("admit contact lookup")
            .recv()
            .expect("receive contact lookup")
            .expect("contact lookup succeeds");
        assert!(matches!(
            reply,
            DirectoryResponse::Lookup(ContactLookupOutcome::Found { contact })
                if contact.destination == destination && contact.identity == Some(identity)
        ));
        reopened.close().expect("close reopened database");
    }

    #[test]
    fn stopped_root_transition_closes_the_old_application_owner() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let first_storage = temporary
            .path()
            .join("first")
            .join("prns")
            .join("development");
        let second_storage = temporary
            .path()
            .join("second")
            .join("prns")
            .join("development");
        let first_paths = prepare_storage(&first_storage).expect("first private storage");
        let second_paths = prepare_storage(&second_storage).expect("second private storage");
        let mut state = SupervisorState {
            worker: None,
            application_owner: Some(
                DevelopmentStoreOwner::open(&first_paths.root, &first_paths.application)
                    .expect("first application owner"),
            ),
            identity_owner: None,
        };

        assert_eq!(
            inspect_identity_locked(&mut state, &second_storage),
            PrimaryIdentityState::Missing
        );
        assert!(state.application_owner.is_none());
        assert_eq!(
            state.identity_owner.as_ref().map(|owner| &owner.root),
            Some(&second_paths.root)
        );
        redb::Database::open(&first_paths.application)
            .expect("the old application owner was closed before the root changed");
    }

    #[test]
    fn running_root_transition_rejects_without_dropping_owned_state() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let first_storage = temporary
            .path()
            .join("first")
            .join("prns")
            .join("development");
        let second_storage = temporary
            .path()
            .join("second")
            .join("prns")
            .join("development");
        let first_paths = prepare_storage(&first_storage).expect("first private storage");
        let second_paths = prepare_storage(&second_storage).expect("second private storage");
        let (commands, _command_rx) = mpsc::channel(1);
        let (shutdown_sender, _shutdown_rx) = watch::channel(false);
        let (_done_tx, done) = std_mpsc::sync_channel(1);
        let mut state = SupervisorState {
            worker: Some(Worker {
                commands,
                shutdown: ShutdownSignal {
                    sender: shutdown_sender,
                    requested: Arc::new(AtomicBool::new(false)),
                },
                done,
                join: None,
                storage_root: first_paths.root.clone(),
            }),
            application_owner: Some(
                DevelopmentStoreOwner::open(&first_paths.root, &first_paths.application)
                    .expect("first application owner"),
            ),
            identity_owner: Some(IdentityOwner {
                root: first_paths.root.clone(),
                vault: FileVault::new(&first_paths.identities),
            }),
        };

        assert!(matches!(
            admit_directory_locked(&mut state, &second_paths, DirectoryRequest::List),
            Err(DevelopmentStoreFailure::Unavailable(_))
        ));
        assert!(matches!(
            inspect_identity_locked(&mut state, &second_storage),
            PrimaryIdentityState::Unavailable { .. }
        ));
        assert_eq!(
            state.worker.as_ref().map(|worker| &worker.storage_root),
            Some(&first_paths.root)
        );
        assert_eq!(
            state.application_owner.as_ref().map(|owner| &owner.root),
            Some(&first_paths.root)
        );
        assert_eq!(
            state.identity_owner.as_ref().map(|owner| &owner.root),
            Some(&first_paths.root)
        );
        let response = state
            .application_owner
            .as_ref()
            .expect("application owner was retained")
            .admit(DirectoryRequest::List)
            .expect("retained owner accepts work");
        assert!(matches!(
            response.recv().expect("receive retained owner response"),
            Ok(DirectoryResponse::List(ContactListOutcome::Listed { contacts }))
                if contacts.is_empty()
        ));

        state.worker = None;
        state
            .application_owner
            .take()
            .expect("application owner remains present")
            .close()
            .expect("close retained owner");
    }

    #[test]
    fn stop_uses_priority_shutdown_when_the_snapshot_lane_is_saturated() {
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(SupervisorState::default()),
        };
        supervisor
            .snapshots
            .set_runtime(DevelopmentNodeRuntime::Running);
        supervisor.snapshots.set_local_host(running_host_state());

        let (commands, mut command_rx) = mpsc::channel(1);
        let (shutdown_sender, mut shutdown_rx) = watch::channel(false);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let shutdown_requested_for_assertion = Arc::clone(&shutdown_requested);
        let shutdown = ShutdownSignal {
            sender: shutdown_sender,
            requested: shutdown_requested,
        };
        let (done_tx, done) = std_mpsc::sync_channel(1);
        let (inspection_started_tx, inspection_started_rx) = std_mpsc::sync_channel(1);
        let join = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime");
            runtime.block_on(async {
                let stalled_snapshot = command_rx.recv().await.expect("snapshot command");
                inspection_started_tx
                    .send(())
                    .expect("publish inspection start");
                shutdown_rx.changed().await.expect("priority shutdown");
                assert!(*shutdown_rx.borrow());
                drop(stalled_snapshot);
            });
            let _ = done_tx.send(Ok(()));
        });
        let (first_response, first_result) = std_mpsc::sync_channel(1);
        assert!(commands.try_send(Command::Snapshot(first_response)).is_ok());
        inspection_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("snapshot inspection started");
        let (queued_response, queued_result) = std_mpsc::sync_channel(1);
        assert!(commands
            .try_send(Command::Snapshot(queued_response))
            .is_ok());
        let mut state = SupervisorState {
            worker: Some(Worker {
                commands,
                shutdown,
                done,
                join: Some(join),
                storage_root: PathBuf::from("/tmp/prns/development"),
            }),
            application_owner: None,
            identity_owner: None,
        };

        assert_eq!(
            stop_locked(&supervisor, &mut state),
            DevelopmentNodeStopOutcome::Stopped
        );

        assert!(state.worker.is_none());
        assert!(shutdown_requested_for_assertion.load(Ordering::Acquire));
        assert!(matches!(
            first_result.recv_timeout(Duration::from_millis(25)),
            Err(std_mpsc::RecvTimeoutError::Disconnected)
        ));
        assert!(matches!(
            queued_result.recv_timeout(Duration::from_millis(25)),
            Err(std_mpsc::RecvTimeoutError::Disconnected)
        ));
        supervisor
            .snapshots
            .set_local_host_unavailable_if_running("late snapshot failure".to_owned());
        assert!(matches!(
            supervisor.snapshots.read().local_host,
            LocalHostState::Stopped { .. }
        ));
    }

    #[test]
    fn stop_reaps_an_already_terminal_worker_and_preserves_failure_state() {
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(true)),
            state: Mutex::new(SupervisorState::default()),
        };
        supervisor
            .snapshots
            .set_runtime(DevelopmentNodeRuntime::Running);
        supervisor.snapshots.set_local_host(running_host_state());
        let terminal = Err((
            DevelopmentNodeStopStage::Node,
            "node stopped unexpectedly".to_owned(),
        ));
        settle_worker_result(
            &supervisor.operation_admitted,
            &supervisor.snapshots,
            &terminal,
        );

        let (commands, _command_rx) = mpsc::channel(1);
        let (shutdown_sender, _shutdown_rx) = watch::channel(false);
        let shutdown = ShutdownSignal {
            sender: shutdown_sender,
            requested: Arc::new(AtomicBool::new(false)),
        };
        let (done_tx, done) = std_mpsc::sync_channel(1);
        done_tx.send(terminal).expect("terminal worker result");
        let join = std::thread::spawn(|| {});
        let mut state = SupervisorState {
            worker: Some(Worker {
                commands,
                shutdown,
                done,
                join: Some(join),
                storage_root: PathBuf::from("/tmp/prns/development"),
            }),
            application_owner: None,
            identity_owner: None,
        };

        assert_eq!(
            stop_locked(&supervisor, &mut state),
            DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Node,
                detail: "node stopped unexpectedly".to_owned(),
            }
        );
        assert!(state.worker.is_none());
        let snapshot = supervisor.snapshots.read();
        assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Failed);
        assert!(snapshot.active_operation.is_none());
        assert_eq!(
            snapshot.local_host,
            LocalHostState::Stopped {
                last_start_failure: Some("node stopped unexpectedly".to_owned()),
            }
        );
    }

    #[test]
    fn primary_vault_malformed_and_operational_failures_are_distinct() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");
        let paths = prepare_storage(&storage).expect("private storage");
        std::fs::write(paths.identities.join("primary"), [0_u8; 63])
            .expect("malformed primary identity");
        let mut state = SupervisorState::default();

        assert!(matches!(
            inspect_identity_locked(&mut state, &storage),
            PrimaryIdentityState::DevelopmentResetRequired { .. }
        ));
        assert!(matches!(
            primary_vault_failure(FileVaultError::Io(std::io::Error::other(
                "identity store unavailable"
            ))),
            PrimaryIdentityState::Unavailable { detail }
                if detail.contains("identity store unavailable")
        ));

        #[cfg(unix)]
        {
            let unavailable_storage = temporary
                .path()
                .join("unavailable")
                .join("prns")
                .join("development");
            let unavailable_paths = prepare_storage(&unavailable_storage).expect("private storage");
            std::os::unix::fs::symlink("primary", unavailable_paths.identities.join("primary"))
                .expect("identity symlink loop");
            let mut unavailable_state = SupervisorState::default();
            assert!(matches!(
                inspect_identity_locked(&mut unavailable_state, &unavailable_storage),
                PrimaryIdentityState::Unavailable { .. }
            ));
        }
    }

    #[test]
    fn identity_and_node_reopen_reset_and_recreate_in_one_process() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");
        let imported = [0x42; 64];

        assert_eq!(
            preview_identity_import(&imported[..63]),
            IdentityImportPreviewOutcome::InvalidLength
        );
        let preview = preview_identity_import(&imported);
        let identity_hash = match preview {
            IdentityImportPreviewOutcome::Valid { identity_hash } => identity_hash,
            IdentityImportPreviewOutcome::InvalidLength => Vec::new(),
        };
        assert_eq!(identity_hash.len(), 16);
        assert_eq!(inspect_identity(&storage), PrimaryIdentityState::Missing);
        let (creation_tx, creation_rx) = std_mpsc::channel();
        std::thread::scope(|scope| {
            for _ in 0..2 {
                let sender = creation_tx.clone();
                let storage = &storage;
                scope.spawn(move || {
                    let _ = sender.send(create_imported_identity(storage, &imported));
                });
            }
        });
        drop(creation_tx);
        let creations = creation_rx.into_iter().collect::<Vec<_>>();
        assert_eq!(creations.len(), 2);
        assert_eq!(
            creations
                .iter()
                .filter(|outcome| matches!(outcome, IdentityCreationOutcome::Created { .. }))
                .count(),
            1
        );
        assert_eq!(
            creations
                .iter()
                .filter(|outcome| matches!(outcome, IdentityCreationOutcome::AlreadyExists))
                .count(),
            1
        );
        assert_eq!(
            create_generated_identity(&storage),
            IdentityCreationOutcome::AlreadyExists
        );
        assert_eq!(
            inspect_identity(&storage),
            PrimaryIdentityState::Present { identity_hash }
        );
        let contact_destination = [0x24; 16];
        assert_eq!(
            save_observed_destination(
                &storage,
                ContactDestinationInput {
                    destination: contact_destination,
                },
            ),
            ContactMutationOutcome::LocalNodeStopped
        );
        assert!(matches!(
            create_manual_contact(
                &storage,
                CreateManualContactInput {
                    destination: contact_destination,
                    identity: None,
                    alias: Some(" Offline contact ".to_owned()),
                },
            ),
            ContactMutationOutcome::Saved { .. }
        ));
        let wrong_storage = temporary
            .path()
            .join("wrong")
            .join("prns")
            .join("development");
        assert!(matches!(
            reset(&wrong_storage),
            DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Persistence,
                ..
            }
        ));
        assert!(storage.exists());
        assert!(matches!(
            list_contacts(&storage),
            ContactListOutcome::Listed { contacts }
                if contacts.len() == 1 && contacts[0].destination == contact_destination
        ));

        assert!(matches!(
            start(&storage),
            DevelopmentNodeStartOutcome::Started { .. }
        ));
        let running = snapshot();
        assert_eq!(running.runtime, DevelopmentNodeRuntime::Running);
        let LocalHostState::Running { host } = running.local_host else {
            panic!("the running generation did not publish its canonical Host snapshot");
        };
        assert_eq!(host.backend.backend(), BackendKind::Native);
        assert!(host.backend.supports(Capability::Bluetooth));
        assert_eq!(host.backend.capabilities().len(), 1);
        assert!(host.persistence.persistent);
        assert!(host.persistence.restored);
        assert!(host.revision >= 1);
        let captured_revision = host.revision;
        std::thread::sleep(Duration::from_millis(550));
        assert_eq!(
            host_revision(&supervisor().snapshots.read()),
            captured_revision
        );
        assert!(host_revision(&snapshot()) > captured_revision);
        let bluetooth_path = storage.join("identities").join("bluetooth-auto.identity");
        let bluetooth_record = std::fs::read(&bluetooth_path).unwrap_or_default();
        assert_eq!(bluetooth_record.len(), 40);
        assert_eq!(stop(), DevelopmentNodeStopOutcome::Stopped);
        assert!(matches!(
            start(&storage),
            DevelopmentNodeStartOutcome::Started { .. }
        ));
        assert_eq!(
            std::fs::read(&bluetooth_path).unwrap_or_default(),
            bluetooth_record
        );
        assert_eq!(stop(), DevelopmentNodeStopOutcome::Stopped);
        std::fs::write(&bluetooth_path, [0_u8; 40]).expect("malformed Bluetooth record");
        assert!(matches!(
            start(&storage),
            DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::Identity,
                ..
            }
        ));
        assert!(matches!(
            supervisor().snapshots.read().local_host,
            LocalHostState::DevelopmentResetRequired { .. }
        ));
        assert!(matches!(
            list_contacts(&storage),
            ContactListOutcome::Listed { contacts } if contacts.len() == 1
        ));
        let reset_guard = supervisor().lock_state();
        let (reset_tx, reset_rx) = std_mpsc::sync_channel(1);
        let reset_storage = storage.clone();
        let reset_thread = std::thread::spawn(move || {
            let _ = reset_tx.send(reset(&reset_storage));
        });
        assert!(matches!(
            reset_rx.recv_timeout(Duration::from_millis(25)),
            Err(std_mpsc::RecvTimeoutError::Timeout)
        ));
        drop(reset_guard);
        let reset_outcome =
            reset_rx
                .recv_timeout(STOP_TIMEOUT)
                .unwrap_or(DevelopmentNodeStopOutcome::Failed {
                    stage: DevelopmentNodeStopStage::Worker,
                    detail: "reset thread did not settle".to_owned(),
                });
        assert!(matches!(
            reset_outcome,
            DevelopmentNodeStopOutcome::AlreadyStopped
        ));
        let _ = reset_thread.join();
        assert!(!storage.exists());
        assert_eq!(inspect_identity(&storage), PrimaryIdentityState::Missing);
        assert_eq!(
            list_contacts(&storage),
            ContactListOutcome::Listed { contacts: vec![] }
        );
        let generated_hash = match create_generated_identity(&storage) {
            IdentityCreationOutcome::Created { identity_hash } => identity_hash,
            _ => Vec::new(),
        };
        assert_eq!(generated_hash.len(), 16);
        supervisor().lock_state().identity_owner = None;
        assert_eq!(
            inspect_identity(&storage),
            PrimaryIdentityState::Present {
                identity_hash: generated_hash,
            }
        );
        assert!(matches!(
            start(&storage),
            DevelopmentNodeStartOutcome::Started { .. }
        ));
        assert_eq!(stop(), DevelopmentNodeStopOutcome::Stopped);
        assert!(matches!(
            reset(&storage),
            DevelopmentNodeStopOutcome::AlreadyStopped
        ));
    }
}
