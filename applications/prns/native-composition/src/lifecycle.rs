use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(all(feature = "apple", target_os = "ios"))]
use personal_rns::bluetooth_auto::{AutoBle, CoreBluetoothCentralRestorationIdentifier};
#[cfg(any(test, all(feature = "apple", target_os = "ios")))]
use personal_rns::interfaces::bluetooth_auto::BleIdentity;
use personal_rns::node_introspection::DestinationIdentityQuery;
use personal_rns::prelude::{
    GrowableHeap, InitiateRemoteControlControllerPairing, ManuallyAttached, PrnsNode,
    PrnsNodeHandle, PrnsNodeRecipe, RemoteControlControllerPairingInitiationControl,
    RemoteControlPairingControl,
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
use prns_core::identity::vault::{
    FileVault, FileVaultError, IdentityLabel, IdentitySecretKey, IdentityVault,
};
use prns_core::identity::PrivateIdentityMaterial;
use prns_host::{BackendInfo, BackendKind, Capability, InterfaceKind, PersistenceSnapshot};
use prns_host_snapshot::{assemble_host_snapshot, HostInterfaceAttachment};
#[cfg(all(feature = "apple", target_os = "ios"))]
use prns_interfaces_tokio::bluetooth_auto::PreparedAutoBle;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use crate::contract::{
    AnnounceLxmfOutcome, AnnounceRemoteControlTargetInput,
    AppleBluetoothRestorationPreparationFailureStage, AppleBluetoothRestorationPreparationOutcome,
    CancelLxmfMessageInput, CancelLxmfMessageOutcome, ContactDestinationInput, ContactListOutcome,
    ContactLookupOutcome, ContactMutationOutcome, CreateManualContactInput,
    DescribeRemoteControlTargetInput, DevelopmentNodeFailure, DevelopmentNodeFailureStage,
    DevelopmentNodeOperation, DevelopmentNodeOperationKind, DevelopmentNodeRuntime,
    DevelopmentNodeSnapshot, DevelopmentNodeStartInput, DevelopmentNodeStartOutcome,
    DevelopmentNodeStopOutcome, DevelopmentNodeStopStage, IdentityCreationOutcome,
    IdentityImportPreviewOutcome, InitiateRemoteControlPairingInput, ListLxmfMessagesInput,
    LocalHostState, LxmfMessageListOutcome, LxmfPeerListOutcome, MeasureLxmfTextInput,
    MeasureLxmfTextOutcome, PrimaryIdentityState, RemoteControlAnnounceFailureStage,
    RemoteControlAnnounceOperation, RemoteControlAnnounceOutcome, RemoteControlAnnounceStatus,
    RemoteControlDescribeFailureStage, RemoteControlDescribeOutcome,
    RemoteControlPairingCommandOutcome, RemoteControlPairingDecisionInput,
    RemoteControlPairingFailureStage, RemoteControlPairingState, RetryLxmfMessageInput,
    RetryLxmfMessageOutcome, SendDirectTextInput, SendDirectTextOutcome, SetContactAliasInput,
    SetContactPinnedInput, U64String,
};
use crate::development_store::{
    DevelopmentStoreFailure, DevelopmentStoreOwner, MailboxStoreReply, StoreReply,
};
use crate::directory::{DirectoryRequest, DirectoryResponse};
use crate::node::{prepare_storage, reset_storage, NodeStoragePaths};
use crate::pairing::{
    apply_event, apply_overflow_failure, apply_persistence_event, attempt_id_string,
    expire_candidates, publish_candidate_resolution, remove_selected_candidate, send_event,
    AppliedNodeEvent, OwnedNodeEvent, PairingCandidateResolution, PairingControls,
    EVENT_LANE_CAPACITY,
};
use crate::snapshot::SnapshotStore;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const PAIRING_APPROVAL_TIMEOUT: Duration = Duration::from_millis(
    personal_rns::remote_control::MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT.0 + 5_000,
);
const HOST_INSPECTION_TIMEOUT: Duration = Duration::from_millis(1_500);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(2);
const DIRECTORY_TIMEOUT: Duration = Duration::from_secs(5);
const LXMF_QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const LXMF_HEALTH_RETRY_DELAY: Duration = Duration::from_millis(250);
const STOP_TIMEOUT: Duration = Duration::from_secs(25);
const COMMAND_LANE_CAPACITY: usize = 8;
const LXMF_SEND_TASK_CAPACITY: usize = 8;

type ReadyResult = Result<DevelopmentNodeSnapshot, (DevelopmentNodeFailureStage, String)>;
type WorkerResult = Result<(), (DevelopmentNodeStopStage, String)>;

#[derive(Clone, Debug, PartialEq, Eq)]
enum AppleBluetoothPreparation {
    WithoutRestoration,
    CentralOnlyRestoration { central: String },
}

impl AppleBluetoothPreparation {
    fn validate(&self) -> Result<(), String> {
        let Self::CentralOnlyRestoration { central } = self else {
            return Ok(());
        };
        if central.is_empty() {
            return Err(
                "central CoreBluetooth restoration identifier must not be empty".to_owned(),
            );
        }
        Ok(())
    }
}

#[cfg(any(test, all(feature = "apple", target_os = "ios")))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct AppleBluetoothOwnerKey {
    storage_root: PathBuf,
    preparation: AppleBluetoothPreparation,
    identity: BleIdentity,
}

#[cfg(any(test, all(feature = "apple", target_os = "ios")))]
struct PreparedAppleBluetoothOwner<T> {
    key: AppleBluetoothOwnerKey,
    prepared: T,
}

struct WorkerBluetoothPreparation {
    preparation: AppleBluetoothPreparation,
    #[cfg(all(feature = "apple", target_os = "ios"))]
    owner_key: AppleBluetoothOwnerKey,
    #[cfg(all(feature = "apple", target_os = "ios"))]
    identity: BleIdentity,
    #[cfg(all(feature = "apple", target_os = "ios"))]
    prepared: Option<PreparedAutoBle>,
}

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
    #[cfg(all(feature = "apple", target_os = "ios"))]
    pending_apple_bluetooth: Option<PreparedAppleBluetoothOwner<PreparedAutoBle>>,
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
    incomplete_stop: Option<(DevelopmentNodeStopStage, String)>,
    storage_root: PathBuf,
    #[cfg(all(feature = "apple", target_os = "ios"))]
    bluetooth_owner_key: AppleBluetoothOwnerKey,
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
}

enum Command {
    AnnounceSelf(RemoteControlAnnounceOperation),
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
    ListLxmfPeers(std_mpsc::SyncSender<LxmfPeerListOutcome>),
    ListLxmfMessages(
        prns_lxmf::mailbox::MailboxListRequest,
        std_mpsc::SyncSender<LxmfMessageListOutcome>,
    ),
    RetryLxmfMessage(u64, std_mpsc::SyncSender<RetryLxmfMessageOutcome>),
    CancelLxmfMessage(u64, u64, std_mpsc::SyncSender<CancelLxmfMessageOutcome>),
    MeasureLxmfText(
        MeasureLxmfTextInput,
        std_mpsc::SyncSender<MeasureLxmfTextOutcome>,
    ),
    AnnounceLxmf(std_mpsc::SyncSender<AnnounceLxmfOutcome>),
    SendDirectText(
        SendDirectTextInput,
        std_mpsc::SyncSender<SendDirectTextOutcome>,
    ),
}

#[derive(Default)]
struct HostAttachment {
    #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
    bluetooth: Option<personal_rns::bluetooth_auto::AttachedBle>,
    #[cfg(any(feature = "apple", feature = "host-test"))]
    tcp: Option<personal_rns::runtime::AttachedInterface>,
}

#[must_use]
pub fn inspect_identity(storage_root: &Path) -> PrimaryIdentityState {
    let supervisor = supervisor();
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);
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
    reap_completed_worker_locked(supervisor, &mut state);
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

fn load_primary_identity_secret(
    state: &mut SupervisorState,
    paths: &NodeStoragePaths,
) -> Result<IdentitySecretKey, PrimaryIdentityState> {
    let label = primary_label()?;
    match identity_vault(state, paths)?.load(&label) {
        Ok(Some(secret)) => Ok(secret),
        Ok(None) => Err(PrimaryIdentityState::Missing),
        Err(error) => Err(primary_vault_failure(error)),
    }
}

fn validate_development_tcp_target(target: Option<&str>) -> Result<Option<String>, String> {
    let Some(target) = target else {
        return Ok(None);
    };
    let address = target.parse::<SocketAddr>().map_err(|_| {
        "developmentTcpTarget must be an explicit IP address and port such as 192.0.2.1:4242"
            .to_owned()
    })?;
    if address.port() == 0 {
        return Err("developmentTcpTarget port must be from 1 through 65535".to_owned());
    }
    Ok(Some(address.to_string()))
}

#[cfg(any(test, all(feature = "apple", target_os = "ios")))]
fn take_matching_prepared_owner<T>(
    pending: &mut Option<PreparedAppleBluetoothOwner<T>>,
    requested: &AppleBluetoothOwnerKey,
) -> Result<Option<T>, String> {
    let Some(owner) = pending.as_ref() else {
        return Ok(None);
    };
    if owner.key != *requested {
        return Err(
            "The prepared CoreBluetooth owner does not match this storage root, restoration configuration, and Bluetooth identity."
                .to_owned(),
        );
    }
    Ok(pending.take().map(|owner| owner.prepared))
}

#[cfg(any(test, all(feature = "apple", target_os = "ios")))]
fn discard_prepared_owner<T>(pending: &mut Option<PreparedAppleBluetoothOwner<T>>) {
    *pending = None;
}

fn apple_bluetooth_preparation_failed(
    stage: AppleBluetoothRestorationPreparationFailureStage,
    detail: impl Into<String>,
) -> AppleBluetoothRestorationPreparationOutcome {
    AppleBluetoothRestorationPreparationOutcome::Failed {
        stage,
        detail: detail.into(),
    }
}

pub(crate) fn prepare_apple_bluetooth_central_restoration(
    storage_root: &Path,
    central_identifier: String,
) -> AppleBluetoothRestorationPreparationOutcome {
    let preparation = AppleBluetoothPreparation::CentralOnlyRestoration {
        central: central_identifier,
    };
    if let Err(detail) = preparation.validate() {
        return apple_bluetooth_preparation_failed(
            AppleBluetoothRestorationPreparationFailureStage::Contract,
            detail,
        );
    }

    prepare_apple_bluetooth_central_restoration_with_supervisor(
        supervisor(),
        storage_root,
        preparation,
    )
}

#[cfg(all(feature = "apple", target_os = "ios"))]
fn prepare_apple_bluetooth_central_restoration_with_supervisor(
    supervisor: &Supervisor,
    storage_root: &Path,
    preparation: AppleBluetoothPreparation,
) -> AppleBluetoothRestorationPreparationOutcome {
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);

    let paths = match prepare_storage(storage_root) {
        Ok(paths) => paths,
        Err(detail) => {
            return apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Storage,
                detail,
            )
        }
    };
    let identity = match personal_rns::load_or_create_ble_identity(&paths.bluetooth_identity) {
        Ok(identity) => identity,
        Err(error) => {
            return apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Identity,
                format!("could not load the installation Bluetooth identity: {error}"),
            )
        }
    };
    let requested = AppleBluetoothOwnerKey {
        storage_root: paths.root,
        preparation: preparation.clone(),
        identity,
    };

    if let Some(worker) = state.worker.as_ref() {
        return if worker.bluetooth_owner_key == requested {
            AppleBluetoothRestorationPreparationOutcome::AlreadyRunning
        } else {
            apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Contract,
                "A different CoreBluetooth owner is already active in this process.",
            )
        };
    }
    if let Some(owner) = state.pending_apple_bluetooth.as_ref() {
        return if owner.key == requested {
            AppleBluetoothRestorationPreparationOutcome::AlreadyPrepared
        } else {
            apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Contract,
                "A different CoreBluetooth owner is already prepared in this process.",
            )
        };
    }

    let AppleBluetoothPreparation::CentralOnlyRestoration { central } = preparation else {
        return apple_bluetooth_preparation_failed(
            AppleBluetoothRestorationPreparationFailureStage::Contract,
            "CoreBluetooth restoration preparation requires restoration identifiers.",
        );
    };
    let identifier = match CoreBluetoothCentralRestorationIdentifier::new(central) {
        Ok(identifier) => identifier,
        Err(error) => {
            return apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Contract,
                error.to_string(),
            )
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Runtime,
                format!("could not create the Bluetooth preparation runtime: {error}"),
            )
        }
    };
    let prepared = match runtime.block_on(AutoBle::prepare_central_only_with_restoration(
        identity, identifier,
    )) {
        Ok(prepared) => prepared,
        Err(error) => {
            return apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Runtime,
                format!("could not create the CoreBluetooth restoration managers: {error:?}"),
            )
        }
    };
    state.pending_apple_bluetooth = Some(PreparedAppleBluetoothOwner {
        key: requested,
        prepared,
    });
    AppleBluetoothRestorationPreparationOutcome::Prepared
}

#[cfg(not(all(feature = "apple", target_os = "ios")))]
fn prepare_apple_bluetooth_central_restoration_with_supervisor(
    _supervisor: &Supervisor,
    _storage_root: &Path,
    _preparation: AppleBluetoothPreparation,
) -> AppleBluetoothRestorationPreparationOutcome {
    apple_bluetooth_preparation_failed(
        AppleBluetoothRestorationPreparationFailureStage::Runtime,
        "CoreBluetooth restoration preparation is only available in the iOS Apple build.",
    )
}

#[cfg(test)]
pub fn start(storage_root: &Path) -> DevelopmentNodeStartOutcome {
    start_configured(
        storage_root,
        DevelopmentNodeStartInput {
            development_tcp_target: None,
        },
    )
}

pub fn start_configured(
    storage_root: &Path,
    input: DevelopmentNodeStartInput,
) -> DevelopmentNodeStartOutcome {
    start_configured_with_supervisor(
        supervisor(),
        storage_root,
        input,
        AppleBluetoothPreparation::WithoutRestoration,
    )
}

pub(crate) fn start_configured_with_apple_bluetooth_central_restoration(
    storage_root: &Path,
    input: DevelopmentNodeStartInput,
    central_identifier: String,
) -> DevelopmentNodeStartOutcome {
    start_configured_with_supervisor(
        supervisor(),
        storage_root,
        input,
        AppleBluetoothPreparation::CentralOnlyRestoration {
            central: central_identifier,
        },
    )
}

fn start_configured_with_supervisor(
    supervisor: &Supervisor,
    storage_root: &Path,
    input: DevelopmentNodeStartInput,
    bluetooth_preparation: AppleBluetoothPreparation,
) -> DevelopmentNodeStartOutcome {
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);
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

    if let Err(detail) = bluetooth_preparation.validate() {
        supervisor.snapshots.fail(DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::Contract,
            detail: detail.clone(),
        });
        return DevelopmentNodeStartOutcome::Failed {
            stage: DevelopmentNodeFailureStage::Contract,
            detail,
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
    #[cfg(all(feature = "apple", target_os = "ios"))]
    let bluetooth_identity =
        match personal_rns::load_or_create_ble_identity(&paths.bluetooth_identity) {
            Ok(identity) => identity,
            Err(error) => {
                let detail = format!("could not load the installation Bluetooth identity: {error}");
                if matches!(
                    error,
                    LocalIdentityFileError::Malformed { .. }
                        | LocalIdentityFileError::EmptyBleIdentity
                        | LocalIdentityFileError::InvalidBleIdentity(_)
                ) {
                    supervisor
                        .snapshots
                        .set_local_host(LocalHostState::DevelopmentResetRequired {
                            reason: detail.clone(),
                        });
                }
                supervisor.snapshots.fail(DevelopmentNodeFailure {
                    stage: DevelopmentNodeFailureStage::Identity,
                    detail: detail.clone(),
                });
                return DevelopmentNodeStartOutcome::Failed {
                    stage: DevelopmentNodeFailureStage::Identity,
                    detail,
                };
            }
        };
    #[cfg(all(feature = "apple", target_os = "ios"))]
    let bluetooth_owner_key = AppleBluetoothOwnerKey {
        storage_root: paths.root.clone(),
        preparation: bluetooth_preparation.clone(),
        identity: bluetooth_identity,
    };
    #[cfg(all(feature = "apple", target_os = "ios"))]
    if state
        .pending_apple_bluetooth
        .as_ref()
        .is_some_and(|owner| owner.key != bluetooth_owner_key)
    {
        let detail = "The prepared CoreBluetooth owner does not match this full start.".to_owned();
        supervisor.snapshots.fail(DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::Contract,
            detail: detail.clone(),
        });
        return DevelopmentNodeStartOutcome::Failed {
            stage: DevelopmentNodeFailureStage::Contract,
            detail,
        };
    }
    let development_tcp_target =
        match validate_development_tcp_target(input.development_tcp_target.as_deref()) {
            Ok(target) => target,
            Err(detail) => {
                supervisor.snapshots.fail(DevelopmentNodeFailure {
                    stage: DevelopmentNodeFailureStage::Contract,
                    detail: detail.clone(),
                });
                return DevelopmentNodeStartOutcome::Failed {
                    stage: DevelopmentNodeFailureStage::Contract,
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
    let primary_identity_secret = match load_primary_identity_secret(&mut state, &paths) {
        Ok(secret) => secret,
        Err(primary_identity) => {
            supervisor
                .snapshots
                .set_primary_identity(primary_identity.clone());
            let detail = match primary_identity {
                PrimaryIdentityState::Unavailable { detail } => detail,
                PrimaryIdentityState::DevelopmentResetRequired { reason } => reason,
                PrimaryIdentityState::Missing => {
                    "Create or import a primary identity before starting the local node.".to_owned()
                }
                PrimaryIdentityState::Present { .. } => {
                    "The primary identity could not be retained for LXMF signing.".to_owned()
                }
            };
            supervisor.snapshots.fail(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Identity,
                detail: detail.clone(),
            });
            return DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::Identity,
                detail,
            };
        }
    };
    let mailbox_submitter = match ensure_application_owner_locked(&mut state, &paths) {
        Ok(owner) => owner.mailbox_submitter(),
        Err(failure) => {
            let detail = match failure {
                DevelopmentStoreFailure::Unavailable(detail) => detail,
                DevelopmentStoreFailure::ResetRequired(reason) => {
                    supervisor
                        .snapshots
                        .set_local_host(LocalHostState::DevelopmentResetRequired {
                            reason: reason.clone(),
                        });
                    reason
                }
            };
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
    #[cfg(all(feature = "apple", target_os = "ios"))]
    let prepared_bluetooth = match take_matching_prepared_owner(
        &mut state.pending_apple_bluetooth,
        &bluetooth_owner_key,
    ) {
        Ok(prepared) => prepared,
        Err(detail) => {
            supervisor.snapshots.fail(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Contract,
                detail: detail.clone(),
            });
            return DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::Contract,
                detail,
            };
        }
    };
    #[cfg(all(feature = "apple", target_os = "ios"))]
    let worker_bluetooth_preparation = WorkerBluetoothPreparation {
        preparation: bluetooth_preparation,
        owner_key: bluetooth_owner_key.clone(),
        identity: bluetooth_identity,
        prepared: prepared_bluetooth,
    };
    #[cfg(not(all(feature = "apple", target_os = "ios")))]
    let worker_bluetooth_preparation = WorkerBluetoothPreparation {
        preparation: bluetooth_preparation,
    };
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
    #[cfg(all(feature = "apple", target_os = "ios"))]
    let bluetooth_owner_key_for_worker = worker_bluetooth_preparation.owner_key.clone();
    let worker_shutdown = shutdown.clone();
    let join = std::thread::Builder::new()
        .name("prns-app-native".to_owned())
        .spawn(move || {
            let result = run_worker(
                paths,
                primary_identity_secret,
                mailbox_submitter,
                development_tcp_target,
                worker_bluetooth_preparation,
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
        incomplete_stop: None,
        storage_root: storage_for_worker,
        #[cfg(all(feature = "apple", target_os = "ios"))]
        bluetooth_owner_key: bluetooth_owner_key_for_worker,
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
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);
    let current = supervisor.snapshots.read();
    // The actor owns one command at a time. Its retained operation record is
    // immediately readable while a remote announcement awaits network settlement.
    if current
        .last_announcement
        .as_ref()
        .is_some_and(|operation| operation.status == RemoteControlAnnounceStatus::Pending)
    {
        return current;
    }
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
    call_pairing(COMMAND_TIMEOUT, |response| {
        Command::Initiate(input, response)
    })
}

pub fn approve(input: RemoteControlPairingDecisionInput) -> RemoteControlPairingCommandOutcome {
    call_pairing(PAIRING_APPROVAL_TIMEOUT, |response| {
        Command::Approve(input, response)
    })
}

pub fn reject(input: RemoteControlPairingDecisionInput) -> RemoteControlPairingCommandOutcome {
    call_pairing(COMMAND_TIMEOUT, |response| Command::Reject(input, response))
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

/// Admit once without tying remote settlement to the lifetime of the bridge call.
pub fn announce_self(input: AnnounceRemoteControlTargetInput) -> RemoteControlAnnounceOutcome {
    announce_self_with_supervisor(supervisor(), input)
}

fn announce_self_with_supervisor(
    supervisor: &Supervisor,
    input: AnnounceRemoteControlTargetInput,
) -> RemoteControlAnnounceOutcome {
    static NEXT_OPERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    if input.target_identity_fingerprint.len() != 16 {
        return RemoteControlAnnounceOutcome::Failed {
            stage: RemoteControlAnnounceFailureStage::Input,
        };
    }
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);
    if supervisor.snapshots.read().runtime != DevelopmentNodeRuntime::Running
        || supervisor.snapshots.is_explicit_stop_in_progress()
    {
        return RemoteControlAnnounceOutcome::Failed {
            stage: RemoteControlAnnounceFailureStage::Node,
        };
    }
    let Some(worker) = state.worker.as_ref() else {
        return RemoteControlAnnounceOutcome::Failed {
            stage: RemoteControlAnnounceFailureStage::Node,
        };
    };
    let commands = &worker.commands;
    if supervisor
        .operation_admitted
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return RemoteControlAnnounceOutcome::Busy;
    }
    let permit = match commands.try_reserve() {
        Ok(permit) => permit,
        Err(error) => {
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            return match error {
                mpsc::error::TrySendError::Full(_) => RemoteControlAnnounceOutcome::Busy,
                mpsc::error::TrySendError::Closed(_) => RemoteControlAnnounceOutcome::Failed {
                    stage: RemoteControlAnnounceFailureStage::Node,
                },
            };
        }
    };
    let Ok(id) =
        NEXT_OPERATION.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
    else {
        supervisor
            .operation_admitted
            .store(false, Ordering::Release);
        return RemoteControlAnnounceOutcome::Busy;
    };
    let operation = RemoteControlAnnounceOperation {
        operation_id: U64String::from(id),
        target_identity_fingerprint: input.target_identity_fingerprint,
        status: RemoteControlAnnounceStatus::Pending,
    };
    let mut published = false;
    supervisor.snapshots.update(|snapshot| {
        if snapshot.runtime != DevelopmentNodeRuntime::Running {
            return;
        }
        snapshot.last_announcement = Some(operation.clone());
        snapshot.active_operation = Some(DevelopmentNodeOperation {
            kind: DevelopmentNodeOperationKind::AnnounceSelf,
            started_at_millis: U64String::from(wall_clock_millis()),
        });
        published = true;
    });
    if !published {
        supervisor
            .operation_admitted
            .store(false, Ordering::Release);
        return RemoteControlAnnounceOutcome::Failed {
            stage: RemoteControlAnnounceFailureStage::Node,
        };
    }
    permit.send(Command::AnnounceSelf(operation.clone()));
    RemoteControlAnnounceOutcome::Accepted {
        operation,
        snapshot: Box::new(supervisor.snapshots.read()),
    }
}

enum LxmfAdmissionFailure {
    LocalNodeStopped,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DurableMailboxAccess {
    RunningGeneration,
    Offline,
    GenerationTransition,
}

const fn durable_mailbox_access(
    runtime: DevelopmentNodeRuntime,
    worker_present: bool,
) -> DurableMailboxAccess {
    match (runtime, worker_present) {
        (DevelopmentNodeRuntime::Running, true) => DurableMailboxAccess::RunningGeneration,
        (DevelopmentNodeRuntime::Stopped | DevelopmentNodeRuntime::Failed, false) => {
            DurableMailboxAccess::Offline
        }
        (
            DevelopmentNodeRuntime::Stopped
            | DevelopmentNodeRuntime::Starting
            | DevelopmentNodeRuntime::Running
            | DevelopmentNodeRuntime::Stopping
            | DevelopmentNodeRuntime::Failed,
            _,
        ) => DurableMailboxAccess::GenerationTransition,
    }
}

fn reap_completed_worker_locked(supervisor: &Supervisor, state: &mut SupervisorState) {
    let Some(worker) = state.worker.as_mut() else {
        return;
    };
    if worker.incomplete_stop.is_some() {
        return;
    }
    if !worker
        .join
        .as_ref()
        .is_some_and(std::thread::JoinHandle::is_finished)
    {
        return;
    }

    let result = worker.done.try_recv();
    join_finished_worker(worker);
    state.worker = None;
    supervisor
        .operation_admitted
        .store(false, Ordering::Release);

    let snapshot_failed = supervisor.snapshots.read().runtime == DevelopmentNodeRuntime::Failed;
    match result {
        Ok(Ok(())) if !snapshot_failed => supervisor.snapshots.stopped(),
        Ok(Err((stage, detail))) if !snapshot_failed => {
            supervisor.snapshots.fail(DevelopmentNodeFailure {
                stage: failure_stage_for_stop(stage),
                detail,
            });
        }
        Err(std_mpsc::TryRecvError::Empty | std_mpsc::TryRecvError::Disconnected)
            if !snapshot_failed =>
        {
            supervisor.snapshots.fail(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Runtime,
                detail: "The native worker exited without publishing its terminal result."
                    .to_owned(),
            });
        }
        Ok(Ok(())) | Ok(Err(_)) | Err(_) => {}
    }
}

fn admit_lxmf<Output>(
    command: impl FnOnce(std_mpsc::SyncSender<Output>) -> Command,
) -> Result<std_mpsc::Receiver<Output>, LxmfAdmissionFailure> {
    let Some(commands) = running_commands() else {
        return Err(LxmfAdmissionFailure::LocalNodeStopped);
    };
    let (response_tx, response_rx) = std_mpsc::sync_channel(1);
    match commands.try_send(command(response_tx)) {
        Ok(()) => Ok(response_rx),
        Err(mpsc::error::TrySendError::Full(_)) => Err(LxmfAdmissionFailure::Busy),
        Err(mpsc::error::TrySendError::Closed(_)) => Err(LxmfAdmissionFailure::LocalNodeStopped),
    }
}

const fn lxmf_send_has_capacity(active: usize) -> bool {
    active < LXMF_SEND_TASK_CAPACITY
}

async fn dispatch_lxmf_message_list(
    service: &prns_lxmf::mailbox::DurableDirectLxmfService,
    request: prns_lxmf::mailbox::MailboxListRequest,
    response: std_mpsc::SyncSender<LxmfMessageListOutcome>,
) {
    let outcome = match service.snapshot(request).await {
        Ok(snapshot) => crate::lxmf::project_messages(&snapshot.messages),
        Err(failure) => crate::lxmf::message_list_failure(failure),
    };
    let _ = response.send(outcome);
}

fn dispatch_lxmf_send(
    service: &prns_lxmf::mailbox::DurableDirectLxmfService,
    send_tasks: &mut JoinSet<()>,
    input: SendDirectTextInput,
    response: std_mpsc::SyncSender<SendDirectTextOutcome>,
    timestamp: u64,
) {
    if !lxmf_send_has_capacity(send_tasks.len()) {
        let _ = response.send(SendDirectTextOutcome::DevelopmentUnavailable {
            detail: "The bounded LXMF response lane is full.".to_owned(),
        });
        return;
    }
    let service = service.clone();
    send_tasks.spawn(async move {
        let outcome = service
            .send_direct_text(
                input.destination,
                timestamp,
                input.title.as_bytes(),
                input.content.as_bytes(),
            )
            .await;
        let _ = response.send(crate::lxmf::project_send_outcome(outcome));
    });
}

#[must_use]
pub fn list_lxmf_peers() -> LxmfPeerListOutcome {
    let response = match admit_lxmf(Command::ListLxmfPeers) {
        Ok(response) => response,
        Err(LxmfAdmissionFailure::LocalNodeStopped) => {
            return LxmfPeerListOutcome::LocalNodeStopped
        }
        Err(LxmfAdmissionFailure::Busy) => return LxmfPeerListOutcome::Busy,
    };
    match response.recv_timeout(LXMF_QUERY_TIMEOUT) {
        Ok(outcome) => outcome,
        Err(std_mpsc::RecvTimeoutError::Timeout) => LxmfPeerListOutcome::Busy,
        Err(std_mpsc::RecvTimeoutError::Disconnected) => LxmfPeerListOutcome::LocalNodeStopped,
    }
}

#[must_use]
pub fn list_lxmf_messages(
    storage_root: &Path,
    input: ListLxmfMessagesInput,
) -> LxmfMessageListOutcome {
    list_lxmf_messages_with_supervisor(supervisor(), storage_root, input)
}

fn list_lxmf_messages_with_supervisor(
    supervisor: &Supervisor,
    storage_root: &Path,
    input: ListLxmfMessagesInput,
) -> LxmfMessageListOutcome {
    let request = match crate::lxmf::mailbox_list_request(input) {
        Ok(request) => request,
        Err(outcome) => return outcome,
    };
    let paths = match prepare_storage(storage_root) {
        Ok(paths) => paths,
        Err(detail) => return LxmfMessageListOutcome::DevelopmentUnavailable { detail },
    };
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);
    if state
        .worker
        .as_ref()
        .is_some_and(|worker| worker.storage_root != paths.root)
    {
        return LxmfMessageListOutcome::DevelopmentUnavailable {
            detail: "The active native generation owns a different development root.".to_owned(),
        };
    }
    match durable_mailbox_access(supervisor.snapshots.read().runtime, state.worker.is_some()) {
        DurableMailboxAccess::RunningGeneration => {}
        DurableMailboxAccess::Offline => {
            let receiver = match admit_mailbox_locked(
                &mut state,
                &paths,
                prns_lxmf::mailbox::MailboxRequest::List(request),
            ) {
                Ok(receiver) => receiver,
                Err(failure) => return crate::lxmf::message_list_failure(failure),
            };
            drop(state);
            return match receiver.recv_timeout(LXMF_QUERY_TIMEOUT) {
                Ok(Ok(prns_lxmf::mailbox::MailboxReply::Listed { messages, .. })) => {
                    crate::lxmf::project_messages(&messages)
                }
                Ok(Ok(_)) => LxmfMessageListOutcome::DevelopmentUnavailable {
                    detail: "The development database returned an unexpected mailbox result."
                        .to_owned(),
                },
                Ok(Err(failure)) => crate::lxmf::message_list_failure(failure),
                Err(_) => LxmfMessageListOutcome::DevelopmentUnavailable {
                    detail: "The durable LXMF query exceeded its bounded wait.".to_owned(),
                },
            };
        }
        DurableMailboxAccess::GenerationTransition => {
            return LxmfMessageListOutcome::DevelopmentUnavailable {
                detail: "The local node generation is transitioning; durable mailbox access is not yet available."
                    .to_owned(),
            };
        }
    }
    {
        let Some(commands) = state.worker.as_ref().map(|worker| worker.commands.clone()) else {
            return LxmfMessageListOutcome::DevelopmentUnavailable {
                detail: "The running native generation has no command authority.".to_owned(),
            };
        };
        drop(state);
        let (response, receiver) = std_mpsc::sync_channel(1);
        match commands.try_send(Command::ListLxmfMessages(request, response)) {
            Ok(()) => receiver
                .recv_timeout(LXMF_QUERY_TIMEOUT)
                .unwrap_or_else(|_| LxmfMessageListOutcome::DevelopmentUnavailable {
                    detail: "The durable LXMF query exceeded its bounded wait.".to_owned(),
                }),
            Err(mpsc::error::TrySendError::Full(_)) => {
                LxmfMessageListOutcome::DevelopmentUnavailable {
                    detail: "The native LXMF command lane is full.".to_owned(),
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                LxmfMessageListOutcome::DevelopmentUnavailable {
                    detail: "The local node stopped before the query was admitted.".to_owned(),
                }
            }
        }
    }
}

#[must_use]
pub fn retry_lxmf_message(
    storage_root: &Path,
    input: RetryLxmfMessageInput,
) -> RetryLxmfMessageOutcome {
    retry_lxmf_message_with_supervisor(supervisor(), storage_root, input)
}

fn retry_lxmf_message_with_supervisor(
    supervisor: &Supervisor,
    storage_root: &Path,
    input: RetryLxmfMessageInput,
) -> RetryLxmfMessageOutcome {
    let Some(local_record_id) = crate::lxmf::parse_canonical_u64(&input.local_record_id.0) else {
        return RetryLxmfMessageOutcome::DevelopmentUnavailable {
            detail: "localRecordId must be a canonical unsigned 64-bit decimal string".to_owned(),
        };
    };
    let paths = match prepare_storage(storage_root) {
        Ok(paths) => paths,
        Err(detail) => return RetryLxmfMessageOutcome::DevelopmentUnavailable { detail },
    };
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);
    if state
        .worker
        .as_ref()
        .is_some_and(|worker| worker.storage_root != paths.root)
    {
        return RetryLxmfMessageOutcome::DevelopmentUnavailable {
            detail: "The active native generation owns a different development root.".to_owned(),
        };
    }
    match durable_mailbox_access(supervisor.snapshots.read().runtime, state.worker.is_some()) {
        DurableMailboxAccess::RunningGeneration => {}
        DurableMailboxAccess::Offline => {
            let receiver = match admit_mailbox_locked(
                &mut state,
                &paths,
                prns_lxmf::mailbox::MailboxRequest::Retry { local_record_id },
            ) {
                Ok(receiver) => receiver,
                Err(failure) => return crate::lxmf::retry_failure(failure),
            };
            drop(state);
            return match receiver.recv() {
                Ok(Ok(prns_lxmf::mailbox::MailboxReply::Retry(transition))) => {
                    project_offline_retry_transition(transition)
                }
                Ok(Ok(_)) => RetryLxmfMessageOutcome::DevelopmentUnavailable {
                    detail: "The development database returned an unexpected retry result."
                        .to_owned(),
                },
                Ok(Err(failure)) => crate::lxmf::retry_failure(failure),
                Err(_) => RetryLxmfMessageOutcome::DevelopmentUnavailable {
                    detail: "The development database owner stopped without reporting the durable retry."
                        .to_owned(),
                },
            };
        }
        DurableMailboxAccess::GenerationTransition => {
            return RetryLxmfMessageOutcome::DevelopmentUnavailable {
                detail: "The local node generation is transitioning; durable mailbox retry is not yet available."
                    .to_owned(),
            };
        }
    }
    {
        let Some(commands) = state.worker.as_ref().map(|worker| worker.commands.clone()) else {
            return RetryLxmfMessageOutcome::DevelopmentUnavailable {
                detail: "The running native generation has no command authority.".to_owned(),
            };
        };
        drop(state);
        let (response, receiver) = std_mpsc::sync_channel(1);
        match commands.try_send(Command::RetryLxmfMessage(local_record_id, response)) {
            Ok(()) => receiver.recv().unwrap_or_else(|_| {
                RetryLxmfMessageOutcome::DevelopmentUnavailable {
                    detail:
                        "The native LXMF response owner stopped without reporting the durable retry."
                            .to_owned(),
                }
            }),
            Err(mpsc::error::TrySendError::Full(_)) => {
                RetryLxmfMessageOutcome::DevelopmentUnavailable {
                    detail: "The native LXMF command lane is full.".to_owned(),
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                RetryLxmfMessageOutcome::DevelopmentUnavailable {
                    detail: "The local node stopped before retry was admitted.".to_owned(),
                }
            }
        }
    }
}

#[must_use]
pub fn cancel_lxmf_message(
    storage_root: &Path,
    input: CancelLxmfMessageInput,
) -> CancelLxmfMessageOutcome {
    cancel_lxmf_message_with_supervisor(supervisor(), storage_root, input)
}

fn cancel_lxmf_message_with_supervisor(
    supervisor: &Supervisor,
    storage_root: &Path,
    input: CancelLxmfMessageInput,
) -> CancelLxmfMessageOutcome {
    let Some(local_record_id) = crate::lxmf::parse_canonical_u64(&input.local_record_id.0) else {
        return CancelLxmfMessageOutcome::DevelopmentUnavailable {
            detail: "localRecordId must be a canonical unsigned 64-bit decimal string".to_owned(),
        };
    };
    let cancelled_at_millis = wall_clock_millis();
    let paths = match prepare_storage(storage_root) {
        Ok(paths) => paths,
        Err(detail) => return CancelLxmfMessageOutcome::DevelopmentUnavailable { detail },
    };
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);
    if state
        .worker
        .as_ref()
        .is_some_and(|worker| worker.storage_root != paths.root)
    {
        return CancelLxmfMessageOutcome::DevelopmentUnavailable {
            detail: "The active native generation owns a different development root.".to_owned(),
        };
    }
    match durable_mailbox_access(supervisor.snapshots.read().runtime, state.worker.is_some()) {
        DurableMailboxAccess::RunningGeneration => {}
        DurableMailboxAccess::Offline => {
            let receiver = match admit_mailbox_locked(
                &mut state,
                &paths,
                prns_lxmf::mailbox::MailboxRequest::Cancel {
                    local_record_id,
                    cancelled_at_millis,
                },
            ) {
                Ok(receiver) => receiver,
                Err(failure) => return crate::lxmf::cancel_failure(failure),
            };
            drop(state);
            return match receiver.recv() {
                Ok(Ok(prns_lxmf::mailbox::MailboxReply::Cancel(transition))) => {
                    project_offline_cancel_transition(transition)
                }
                Ok(Ok(_)) => CancelLxmfMessageOutcome::DevelopmentUnavailable {
                    detail: "The development database returned an unexpected cancellation result."
                        .to_owned(),
                },
                Ok(Err(failure)) => crate::lxmf::cancel_failure(failure),
                Err(_) => CancelLxmfMessageOutcome::DevelopmentUnavailable {
                    detail: "The development database owner stopped without reporting the durable cancellation."
                        .to_owned(),
                },
            };
        }
        DurableMailboxAccess::GenerationTransition => {
            return CancelLxmfMessageOutcome::DevelopmentUnavailable {
                detail: "The local node generation is transitioning; durable mailbox cancellation is not yet available."
                    .to_owned(),
            };
        }
    }
    {
        let Some(commands) = state.worker.as_ref().map(|worker| worker.commands.clone()) else {
            return CancelLxmfMessageOutcome::DevelopmentUnavailable {
                detail: "The running native generation has no command authority.".to_owned(),
            };
        };
        drop(state);
        let (response, receiver) = std_mpsc::sync_channel(1);
        match commands.try_send(Command::CancelLxmfMessage(
            local_record_id,
            cancelled_at_millis,
            response,
        )) {
            Ok(()) => receiver.recv().unwrap_or_else(|_| {
                CancelLxmfMessageOutcome::DevelopmentUnavailable {
                    detail: "The native LXMF response owner stopped without reporting the durable cancellation."
                        .to_owned(),
                }
            }),
            Err(mpsc::error::TrySendError::Full(_)) => {
                CancelLxmfMessageOutcome::DevelopmentUnavailable {
                    detail: "The native LXMF command lane is full.".to_owned(),
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                CancelLxmfMessageOutcome::DevelopmentUnavailable {
                    detail: "The local node stopped before cancellation was admitted.".to_owned(),
                }
            }
        }
    }
}

fn project_offline_retry_transition(
    transition: prns_lxmf::mailbox::RetryTransition,
) -> RetryLxmfMessageOutcome {
    crate::lxmf::project_retry_outcome(match transition {
        prns_lxmf::mailbox::RetryTransition::Accepted { message, .. } => {
            prns_lxmf::mailbox::RetryLxmfMessageOutcome::Accepted {
                local_record_id: message.local_record_id,
            }
        }
        prns_lxmf::mailbox::RetryTransition::NotFound => {
            prns_lxmf::mailbox::RetryLxmfMessageOutcome::NotFound
        }
        prns_lxmf::mailbox::RetryTransition::NotFailed { current } => {
            prns_lxmf::mailbox::RetryLxmfMessageOutcome::NotFailed { current }
        }
    })
}

fn project_offline_cancel_transition(
    transition: prns_lxmf::mailbox::CancelTransition,
) -> CancelLxmfMessageOutcome {
    crate::lxmf::project_cancel_outcome(match transition {
        prns_lxmf::mailbox::CancelTransition::Cancelled { message, .. } => {
            prns_lxmf::mailbox::CancelLxmfMessageOutcome::Cancelled {
                local_record_id: message.local_record_id,
            }
        }
        prns_lxmf::mailbox::CancelTransition::NotFound => {
            prns_lxmf::mailbox::CancelLxmfMessageOutcome::NotFound
        }
        prns_lxmf::mailbox::CancelTransition::AlreadyDelivered => {
            prns_lxmf::mailbox::CancelLxmfMessageOutcome::AlreadyDelivered
        }
        prns_lxmf::mailbox::CancelTransition::AlreadyCancelled => {
            prns_lxmf::mailbox::CancelLxmfMessageOutcome::AlreadyCancelled
        }
        prns_lxmf::mailbox::CancelTransition::NotCancellable { current } => {
            prns_lxmf::mailbox::CancelLxmfMessageOutcome::NotCancellable { current }
        }
    })
}

#[must_use]
pub fn measure_lxmf_text(input: MeasureLxmfTextInput) -> MeasureLxmfTextOutcome {
    let response = match admit_lxmf(|response| Command::MeasureLxmfText(input, response)) {
        Ok(response) => response,
        Err(LxmfAdmissionFailure::LocalNodeStopped) => {
            return MeasureLxmfTextOutcome::LocalNodeStopped
        }
        Err(LxmfAdmissionFailure::Busy) => return MeasureLxmfTextOutcome::Busy,
    };
    match response.recv_timeout(LXMF_QUERY_TIMEOUT) {
        Ok(outcome) => outcome,
        Err(std_mpsc::RecvTimeoutError::Timeout) => MeasureLxmfTextOutcome::Busy,
        Err(std_mpsc::RecvTimeoutError::Disconnected) => MeasureLxmfTextOutcome::LocalNodeStopped,
    }
}

#[must_use]
pub fn announce_lxmf() -> AnnounceLxmfOutcome {
    let response = match admit_lxmf(Command::AnnounceLxmf) {
        Ok(response) => response,
        Err(LxmfAdmissionFailure::LocalNodeStopped) => {
            return AnnounceLxmfOutcome::LocalNodeStopped
        }
        Err(LxmfAdmissionFailure::Busy) => return AnnounceLxmfOutcome::Busy,
    };
    match response.recv_timeout(LXMF_QUERY_TIMEOUT) {
        Ok(outcome) => outcome,
        Err(std_mpsc::RecvTimeoutError::Timeout) => AnnounceLxmfOutcome::Failed,
        Err(std_mpsc::RecvTimeoutError::Disconnected) => AnnounceLxmfOutcome::LocalNodeStopped,
    }
}

#[must_use]
pub fn send_direct_text(input: SendDirectTextInput) -> SendDirectTextOutcome {
    let response = match admit_lxmf(|response| Command::SendDirectText(input, response)) {
        Ok(response) => response,
        Err(LxmfAdmissionFailure::LocalNodeStopped) => {
            return SendDirectTextOutcome::DevelopmentUnavailable {
                detail: "The local node is not running; peer eligibility is generation-bound."
                    .to_owned(),
            }
        }
        Err(LxmfAdmissionFailure::Busy) => {
            return SendDirectTextOutcome::DevelopmentUnavailable {
                detail: "The native LXMF command lane is full.".to_owned(),
            }
        }
    };
    // After admission, only the database owner can determine whether the unique
    // insert committed. A synthetic timeout would let a caller retry an insert
    // that may later commit and create a duplicate durable message.
    wait_for_admitted_lxmf_send(response)
}

fn wait_for_admitted_lxmf_send(
    response: std_mpsc::Receiver<SendDirectTextOutcome>,
) -> SendDirectTextOutcome {
    match response.recv() {
        Ok(outcome) => outcome,
        Err(_) => SendDirectTextOutcome::DevelopmentUnavailable {
            detail: "The native LXMF response owner stopped without reporting the queue commit."
                .to_owned(),
        },
    }
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
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);
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
    reap_completed_worker_locked(supervisor, &mut state);
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
    reap_completed_worker_locked(supervisor, &mut state);
    let paths = prepare_storage(storage_root).map_err(DevelopmentStoreFailure::unavailable)?;
    admit_directory_locked(&mut state, &paths, request)
}

fn admit_directory_locked(
    state: &mut SupervisorState,
    paths: &NodeStoragePaths,
    request: DirectoryRequest,
) -> Result<std_mpsc::Receiver<StoreReply>, DevelopmentStoreFailure> {
    ensure_application_owner_locked(state, paths)?.admit(request)
}

fn ensure_application_owner_locked<'a>(
    state: &'a mut SupervisorState,
    paths: &NodeStoragePaths,
) -> Result<&'a DevelopmentStoreOwner, DevelopmentStoreFailure> {
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
    state.application_owner.as_ref().ok_or_else(|| {
        DevelopmentStoreFailure::unavailable("the development database owner was not initialized")
    })
}

fn admit_mailbox_locked(
    state: &mut SupervisorState,
    paths: &NodeStoragePaths,
    request: prns_lxmf::mailbox::MailboxRequest,
) -> Result<std_mpsc::Receiver<MailboxStoreReply>, prns_lxmf::mailbox::MailboxFailure> {
    ensure_application_owner_locked(state, paths)
        .map_err(|failure| match failure {
            DevelopmentStoreFailure::Unavailable(detail) => {
                prns_lxmf::mailbox::MailboxFailure::Unavailable(detail)
            }
            DevelopmentStoreFailure::ResetRequired(reason) => {
                prns_lxmf::mailbox::MailboxFailure::ResetRequired(reason)
            }
        })?
        .admit_mailbox(request)
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
    #[cfg(all(feature = "apple", target_os = "ios"))]
    {
        discard_prepared_owner(&mut state.pending_apple_bluetooth);
    }
    let Some(worker) = state.worker.as_mut() else {
        return DevelopmentNodeStopOutcome::AlreadyStopped;
    };

    if worker.join.is_none() {
        if let Some((stage, detail)) = worker.incomplete_stop.as_ref() {
            return DevelopmentNodeStopOutcome::Failed {
                stage: *stage,
                detail: detail.clone(),
            };
        }
    }

    let stop_started_at_millis = U64String::from(wall_clock_millis());
    let terminal_snapshot = supervisor.snapshots.read();
    let preserve_terminal_failure = terminal_snapshot.runtime == DevelopmentNodeRuntime::Failed;
    let prior_terminal_failure = terminal_snapshot.failure;
    supervisor
        .snapshots
        .begin_stop(stop_started_at_millis.clone());
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
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            let failure = DevelopmentNodeFailure {
                stage: failure_stage_for_stop(stage),
                detail: detail.clone(),
            };
            if preserve_terminal_failure {
                state.worker = None;
                supervisor
                    .snapshots
                    .terminal_fail(prior_terminal_failure.unwrap_or(failure));
            } else {
                worker.incomplete_stop = Some((stage, detail.clone()));
                supervisor
                    .snapshots
                    .incomplete_stop(failure, stop_started_at_millis);
            }
            DevelopmentNodeStopOutcome::Failed { stage, detail }
        }
        Err(std_mpsc::RecvTimeoutError::Timeout) => {
            let detail = "The native worker did not finish bounded shutdown.".to_owned();
            worker.incomplete_stop = Some((DevelopmentNodeStopStage::Worker, detail.clone()));
            supervisor.snapshots.incomplete_stop(
                DevelopmentNodeFailure {
                    stage: DevelopmentNodeFailureStage::Runtime,
                    detail: detail.clone(),
                },
                stop_started_at_millis,
            );
            DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Worker,
                detail,
            }
        }
        Err(std_mpsc::RecvTimeoutError::Disconnected) => {
            join_finished_worker(worker);
            let detail = "The native worker exited without a shutdown result.".to_owned();
            supervisor
                .operation_admitted
                .store(false, Ordering::Release);
            let failure = DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Runtime,
                detail: detail.clone(),
            };
            if preserve_terminal_failure {
                state.worker = None;
                supervisor
                    .snapshots
                    .terminal_fail(prior_terminal_failure.unwrap_or(failure));
            } else {
                worker.incomplete_stop = Some((DevelopmentNodeStopStage::Worker, detail.clone()));
                supervisor
                    .snapshots
                    .incomplete_stop(failure, stop_started_at_millis);
            }
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
    reset_with_supervisor(supervisor(), storage_root)
}

fn reset_with_supervisor(
    supervisor: &Supervisor,
    storage_root: &Path,
) -> DevelopmentNodeStopOutcome {
    let mut state = supervisor.lock_state();
    #[cfg(all(feature = "apple", target_os = "ios"))]
    let pending_bluetooth_root = state
        .pending_apple_bluetooth
        .as_ref()
        .map(|owner| &owner.key.storage_root);
    #[cfg(not(all(feature = "apple", target_os = "ios")))]
    let pending_bluetooth_root: Option<&PathBuf> = None;
    let owned_roots = [
        state.worker.as_ref().map(|worker| &worker.storage_root),
        state.application_owner.as_ref().map(|owner| &owner.root),
        state.identity_owner.as_ref().map(|owner| &owner.root),
        pending_bluetooth_root,
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
    let discard_joined_incomplete_stop = state
        .worker
        .as_ref()
        .is_some_and(|worker| worker.join.is_none() && worker.incomplete_stop.is_some());
    let stop_outcome = if discard_joined_incomplete_stop {
        supervisor
            .operation_admitted
            .store(false, Ordering::Release);
        DevelopmentNodeStopOutcome::Stopped
    } else {
        stop_locked(supervisor, &mut state)
    };
    if !matches!(
        stop_outcome,
        DevelopmentNodeStopOutcome::Stopped | DevelopmentNodeStopOutcome::AlreadyStopped
    ) {
        return stop_outcome;
    }
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
            if discard_joined_incomplete_stop {
                state.worker = None;
            }
            supervisor.snapshots.reset();
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
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);
    if supervisor.snapshots.read().runtime != DevelopmentNodeRuntime::Running {
        return None;
    }
    state.worker.as_ref().map(|worker| worker.commands.clone())
}

fn call_pairing(
    timeout: Duration,
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
    response_rx.recv_timeout(timeout).unwrap_or_else(|_| {
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

#[allow(clippy::too_many_arguments)]
fn run_worker(
    paths: NodeStoragePaths,
    primary_identity_secret: IdentitySecretKey,
    mailbox_submitter: Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
    development_tcp_target: Option<String>,
    bluetooth_preparation: WorkerBluetoothPreparation,
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
        primary_identity_secret,
        mailbox_submitter,
        development_tcp_target,
        bluetooth_preparation,
        commands,
        shutdown,
        shutdown_rx,
        snapshots,
        operation_admitted,
        ready,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn run_generation(
    paths: NodeStoragePaths,
    primary_identity_secret: IdentitySecretKey,
    mailbox_submitter: Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
    development_tcp_target: Option<String>,
    bluetooth_preparation: WorkerBluetoothPreparation,
    commands: mpsc::Receiver<Command>,
    shutdown: ShutdownSignal,
    shutdown_rx: watch::Receiver<bool>,
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
    #[cfg(all(feature = "apple", target_os = "ios"))]
    let bluetooth_identity = bluetooth_preparation.identity;
    #[cfg(not(all(feature = "apple", target_os = "ios")))]
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

    #[cfg(all(feature = "apple", target_os = "ios"))]
    let prepared_bluetooth = match bluetooth_preparation.prepared {
        Some(prepared) => prepared,
        None => match bluetooth_preparation.preparation {
            AppleBluetoothPreparation::WithoutRestoration => {
                match AutoBle::prepare_central_only_without_restoration(bluetooth_identity).await {
                    Ok(prepared) => prepared,
                    Err(_) => {
                        AutoBle::unavailable_central_only_without_restoration(bluetooth_identity)
                    }
                }
            }
            AppleBluetoothPreparation::CentralOnlyRestoration { central } => {
                let restoration =
                    CoreBluetoothCentralRestorationIdentifier::new(central).map_err(|error| {
                        boot_failure(
                            &ready,
                            &snapshots,
                            DevelopmentNodeFailureStage::Contract,
                            error.to_string(),
                        )
                    })?;
                match AutoBle::prepare_central_only_with_restoration(
                    bluetooth_identity,
                    restoration.clone(),
                )
                .await
                {
                    Ok(prepared) => prepared,
                    Err(_) => AutoBle::unavailable_central_only_with_restoration(
                        bluetooth_identity,
                        restoration,
                    ),
                }
            }
        },
    };
    #[cfg(all(feature = "apple", target_os = "macos"))]
    let prepared_bluetooth =
        match personal_rns::bluetooth_auto::AutoBle::prepare_without_restoration(bluetooth_identity)
            .await
        {
            Ok(prepared) => prepared,
            Err(_) => personal_rns::bluetooth_auto::AutoBle::unavailable_without_restoration(
                bluetooth_identity,
            ),
        };
    #[cfg(all(feature = "apple", target_os = "macos"))]
    let _ = bluetooth_preparation.preparation;
    #[cfg(not(all(feature = "apple", any(target_os = "ios", target_os = "macos"))))]
    let _ = (bluetooth_identity, bluetooth_preparation.preparation);

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
    let mut announce_app_data = [0_u8; 64];
    let announce_len =
        prns_lxmf::wire::encode_current_lxmf_announce(b"prns", &mut announce_app_data).map_err(
            |error| {
                boot_failure(
                    &ready,
                    &snapshots,
                    DevelopmentNodeFailureStage::Contract,
                    format!("could not encode the built-in LXMF announce: {error:?}"),
                )
            },
        )?;
    let (lxmf_identity, lxmf_destination) = prns_lxmf::direct::prepare_local_lxmf_destination(
        primary_identity_secret,
        &announce_app_data[..announce_len],
    )
    .map_err(|error| {
        boot_failure(
            &ready,
            &snapshots,
            DevelopmentNodeFailureStage::Contract,
            format!("could not derive the built-in LXMF destination: {error:?}"),
        )
    })?;
    let (pending_lxmf, lxmf_callbacks) =
        prns_lxmf::mailbox::DurableDirectLxmfService::prepare(lxmf_identity);
    let (event_tx, event_rx) = mpsc::channel(EVENT_LANE_CAPACITY);
    let overflowed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let event_overflowed = Arc::clone(&overflowed);
    let lxmf_events = lxmf_callbacks.clone();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control,
        pre_configured_destinations: [lxmf_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: personal_rns::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence,
        on_event: move |event, _state: &()| {
            let _lxmf_outcome = lxmf_events.on_prns_event(&event);
            send_event(&event_tx, &event_overflowed, event);
        },
    })
    .with_accepted_announce_observer(lxmf_callbacks.accepted_announce_observer());
    let handle = node.handle();
    let clock = node.clock();

    #[cfg(any(feature = "apple", feature = "host-test"))]
    let mut host_attachment = HostAttachment::default();
    #[cfg(not(any(feature = "apple", feature = "host-test")))]
    let host_attachment = HostAttachment::default();
    #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
    {
        host_attachment.bluetooth = Some(handle.attach(prepared_bluetooth));
    }
    #[cfg(any(feature = "apple", feature = "host-test"))]
    if let Some(target) = development_tcp_target {
        host_attachment.tcp =
            Some(handle.attach(personal_rns::tcp::TcpClientInterface::new(target)));
    }
    #[cfg(not(any(feature = "apple", feature = "host-test")))]
    if development_tcp_target.is_some() {
        return Err(boot_failure(
            &ready,
            &snapshots,
            DevelopmentNodeFailureStage::Contract,
            "this native build does not include the development TCP fixture".to_owned(),
        ));
    }

    let lxmf_service = pending_lxmf
        .start_paused(
            Arc::new(prns_lxmf::direct::PrnsDirectNetwork::new(handle.clone())),
            mailbox_submitter,
            Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
        )
        .await
        .map_err(|error| {
            boot_failure(
                &ready,
                &snapshots,
                DevelopmentNodeFailureStage::Runtime,
                format!("could not start the LXMF service worker: {error:?}"),
            )
        })?;
    let lxmf_refresh = lxmf_service.subscribe();
    refresh_lxmf_health(&lxmf_service, &snapshots)
        .await
        .map(|_| ())
        .map_err(|failure| {
            let detail = lxmf_storage_failure_detail(&snapshots, &failure);
            boot_failure(
                &ready,
                &snapshots,
                DevelopmentNodeFailureStage::Storage,
                detail,
            )
        })?;

    snapshots.update(|snapshot| {
        snapshot.controller_identity_fingerprint = Some(controller_identity_fingerprint);
        if host_attachment.bluetooth_is_none() {
            snapshot.pairing = RemoteControlPairingState::BluetoothUnavailable;
        }
    });

    let (node_shutdown_tx, mut node_shutdown_rx) = watch::channel(false);
    let node_run = node.run_until(async {
        if !*node_shutdown_rx.borrow() {
            let _ = node_shutdown_rx.changed().await;
        }
    });
    let actor = run_actor(
        handle,
        lxmf_service,
        lxmf_refresh,
        commands,
        event_rx,
        overflowed,
        host_attachment,
        clock,
        Arc::clone(&snapshots),
        Arc::clone(&operation_admitted),
        ready,
        shutdown_rx,
    );
    tokio::pin!(node_run);
    tokio::pin!(actor);
    let result = tokio::select! {
        actor_result = &mut actor => {
            let _changed = node_shutdown_tx.send(true);
            let node_result = node_run.await.map_err(|error| map_node_run_error(
                error,
                "the Prns node failed during shutdown",
            ));
            actor_result.and(node_result)
        }
        node_result = &mut node_run => {
            let node_result = node_result.map_err(|error| map_node_run_error(
                error,
                "the Prns node stopped unexpectedly",
            ));
            shutdown.request();
            let actor_result = actor.await;
            match node_result {
                Err(failure) => Err(failure),
                Ok(()) => actor_result.and(Err((
                    DevelopmentNodeStopStage::Node,
                    "The Prns node stopped before lifecycle shutdown completed.".to_owned(),
                ))),
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
            let explicit_stop_in_progress = snapshots.is_explicit_stop_in_progress();
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
            if !explicit_stop_in_progress {
                snapshot.runtime = DevelopmentNodeRuntime::Failed;
                if !matches!(
                    &snapshot.local_host,
                    LocalHostState::DevelopmentResetRequired { .. }
                ) {
                    snapshot.local_host = LocalHostState::Stopped {
                        last_start_failure: Some(detail.clone()),
                    };
                }
                snapshot.active_operation = None;
            }
            snapshot.failure = Some(DevelopmentNodeFailure {
                stage: failure_stage_for_stop(*stage),
                detail: detail.clone(),
            });
        });
    } else if !snapshots.is_explicit_stop_in_progress() {
        snapshots.stopped();
    }
    snapshots.interrupt_announcement();
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
    lxmf_service: prns_lxmf::mailbox::DurableDirectLxmfService,
    mut lxmf_refresh: watch::Receiver<u64>,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Receiver<OwnedNodeEvent>,
    overflowed: Arc<std::sync::atomic::AtomicBool>,
    host_attachment: HostAttachment,
    clock: personal_rns::manifold::tokio::TokioClock,
    snapshots: Arc<SnapshotStore>,
    operation_admitted: Arc<AtomicBool>,
    ready: std_mpsc::SyncSender<ReadyResult>,
    shutdown_rx: watch::Receiver<bool>,
) -> WorkerResult {
    let mut send_tasks = JoinSet::new();
    let result = run_actor_loop(
        &handle,
        &lxmf_service,
        &mut lxmf_refresh,
        &mut send_tasks,
        commands,
        events,
        overflowed,
        &host_attachment,
        clock,
        &snapshots,
        &operation_admitted,
        &ready,
        shutdown_rx,
    )
    .await;

    let lxmf_stop_result = stop_lxmf_service_and_drain(&lxmf_service, &mut send_tasks).await;
    let health_result = refresh_lxmf_health(&lxmf_service, &snapshots)
        .await
        .map(|_| ())
        .map_err(|failure| {
            let detail = publish_lxmf_storage_failure(&snapshots, &failure);
            (DevelopmentNodeStopStage::Persistence, detail)
        });
    result.and(lxmf_stop_result).and(health_result)
}

#[allow(clippy::too_many_arguments)]
async fn run_actor_loop(
    handle: &PrnsNodeHandle,
    lxmf_service: &prns_lxmf::mailbox::DurableDirectLxmfService,
    lxmf_refresh: &mut watch::Receiver<u64>,
    send_tasks: &mut JoinSet<()>,
    mut commands: mpsc::Receiver<Command>,
    mut events: mpsc::Receiver<OwnedNodeEvent>,
    overflowed: Arc<std::sync::atomic::AtomicBool>,
    host_attachment: &HostAttachment,
    clock: personal_rns::manifold::tokio::TokioClock,
    snapshots: &SnapshotStore,
    operation_admitted: &AtomicBool,
    ready: &std_mpsc::SyncSender<ReadyResult>,
    mut shutdown_rx: watch::Receiver<bool>,
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
            if apply_event(event, &mut controls, snapshots, clock.now())
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
            return Err((DevelopmentNodeStopStage::Persistence, detail));
        }
        Err(_) => {
            let detail =
                "Prns did not publish PersistenceRestored before the startup deadline.".to_owned();
            let _ = ready.send(Err((
                DevelopmentNodeFailureStage::PersistenceRestore,
                detail.clone(),
            )));
            return Err((DevelopmentNodeStopStage::Persistence, detail));
        }
    }

    if !lxmf_service.activate_queued_attempts().await {
        let detail =
            "The durable LXMF service stopped before queued attempts were activated.".to_owned();
        let _ = ready.send(Err((DevelopmentNodeFailureStage::Runtime, detail.clone())));
        return Err((DevelopmentNodeStopStage::Worker, detail));
    }

    let _ = crate::remote_control::refresh_targets(handle, snapshots).await;
    snapshots.update(|snapshot| {
        snapshot.runtime = DevelopmentNodeRuntime::Running;
        snapshot.failure = None;
    });
    refresh_host_snapshot(
        handle,
        host_attachment,
        &persistence,
        &mut host_revision,
        started,
        snapshots,
    )
    .await;
    let _ = ready.send(Ok(snapshots.read()));

    let mut candidate_expiry = tokio::time::interval(Duration::from_millis(500));
    let mut lxmf_health_retry = tokio::time::interval(LXMF_HEALTH_RETRY_DELAY);
    lxmf_health_retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut retry_lxmf_health = false;
    loop {
        if overflowed.swap(false, std::sync::atomic::Ordering::AcqRel) {
            apply_overflow_failure(&mut controls, snapshots);
        }
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    commands.close();
                    snapshots.set_runtime(DevelopmentNodeRuntime::Stopping);
                    return Ok(());
                }
            }
            changed = lxmf_refresh.changed() => {
                if changed.is_err() {
                    snapshots.fail(DevelopmentNodeFailure {
                        stage: DevelopmentNodeFailureStage::Runtime,
                        detail: "The LXMF refresh authority closed unexpectedly.".to_owned(),
                    });
                    return Err((
                        DevelopmentNodeStopStage::Worker,
                        "The native actor lost its LXMF refresh authority.".to_owned(),
                    ));
                }
                refresh_running_lxmf_health_after_change(
                    lxmf_service,
                    snapshots,
                    &mut retry_lxmf_health,
                    &mut lxmf_health_retry,
                ).await?;
            }
            _ = lxmf_health_retry.tick(), if retry_lxmf_health => {
                retry_lxmf_health = refresh_running_lxmf_health(lxmf_service, snapshots).await?;
            }
            completed = send_tasks.join_next(), if !send_tasks.is_empty() => {
                if completed.is_some_and(|result| result.is_err()) {
                    snapshots.fail(DevelopmentNodeFailure {
                        stage: DevelopmentNodeFailureStage::Runtime,
                        detail: "An LXMF send task stopped unexpectedly.".to_owned(),
                    });
                    return Err((
                        DevelopmentNodeStopStage::Worker,
                        "An LXMF send task stopped unexpectedly.".to_owned(),
                    ));
                }
            }
            _ = candidate_expiry.tick() => {
                expire_candidates(&mut controls, snapshots, clock.now());
            }
            event = events.recv() => {
                match event {
                    Some(event) => {
                        apply_persistence_event(&event, &mut persistence);
                        if apply_event(event, &mut controls, snapshots, clock.now())
                            == AppliedNodeEvent::TargetInventoryChanged
                        {
                            if let Err(detail) = crate::remote_control::refresh_targets(handle, snapshots).await {
                                snapshots.fail(DevelopmentNodeFailure {
                                    stage: DevelopmentNodeFailureStage::Node,
                                    detail: detail.clone(),
                                });
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
                        handle,
                        host_attachment,
                        &persistence,
                        &mut host_revision,
                        started,
                        snapshots,
                    ).await;
                    let _ = response.send(snapshots.read());
                }
                Some(Command::Initiate(input, response)) => {
                    let outcome = if controls.pairing_in_progress() {
                        RemoteControlPairingCommandOutcome::Busy
                    } else {
                        initiate_pairing(handle, &mut controls, snapshots, clock.now(), input).await
                    };
                    operation_admitted.store(false, Ordering::Release);
                    let _ = response.send(outcome);
                }
                Some(Command::Approve(input, response)) => {
                    let outcome = approve_pairing(handle, &mut controls, snapshots, input).await;
                    operation_admitted.store(false, Ordering::Release);
                    let _ = response.send(outcome);
                }
                Some(Command::Reject(input, response)) => {
                    let outcome = reject_pairing(handle, &mut controls, snapshots, input).await;
                    operation_admitted.store(false, Ordering::Release);
                    let _ = response.send(outcome);
                }
                Some(Command::Describe(input, response)) => {
                    let outcome = if controls.pairing_in_progress() {
                        RemoteControlDescribeOutcome::Busy
                    } else {
                        crate::remote_control::describe(handle, snapshots, input).await
                    };
                    operation_admitted.store(false, Ordering::Release);
                    let _ = response.send(outcome);
                }
                Some(Command::AnnounceSelf(operation)) => {
                    let status = if controls.pairing_in_progress() {
                        RemoteControlAnnounceStatus::Failed { stage: RemoteControlAnnounceFailureStage::Busy }
                    } else {
                        crate::remote_control::announce_self(handle, &operation.target_identity_fingerprint).await
                    };
                    crate::remote_control::complete_announcement(snapshots, &operation.operation_id, status);
                    operation_admitted.store(false, Ordering::Release);
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
                Some(Command::ListLxmfPeers(response)) => {
                    let outcome = match lxmf_service
                        .snapshot(prns_lxmf::mailbox::MailboxListRequest {
                            peer: None,
                            direction: None,
                            before: None,
                            limit: 1,
                        })
                        .await
                    {
                        Ok(snapshot) => crate::lxmf::project_peers(&snapshot, clock.now().0),
                        Err(_) => LxmfPeerListOutcome::Busy,
                    };
                    let _ = response.send(outcome);
                }
                Some(Command::ListLxmfMessages(request, response)) => {
                    dispatch_lxmf_message_list(lxmf_service, request, response).await;
                }
                Some(Command::RetryLxmfMessage(local_record_id, response)) => {
                    let outcome = lxmf_service.retry_lxmf_message(local_record_id).await;
                    let _ = response.send(crate::lxmf::project_retry_outcome(outcome));
                }
                Some(Command::CancelLxmfMessage(
                    local_record_id,
                    cancelled_at_millis,
                    response,
                )) => {
                    let outcome = lxmf_service
                        .cancel_lxmf_message(local_record_id, cancelled_at_millis)
                        .await;
                    let _ = response.send(crate::lxmf::project_cancel_outcome(outcome));
                }
                Some(Command::MeasureLxmfText(input, response)) => {
                    let _ = response.send(crate::lxmf::measure_text(&input));
                }
                Some(Command::AnnounceLxmf(response)) => {
                    let outcome = match lxmf_service.announce().await {
                        Ok(()) => AnnounceLxmfOutcome::Announced,
                        Err(_) => AnnounceLxmfOutcome::Failed,
                    };
                    let _ = response.send(outcome);
                }
                Some(Command::SendDirectText(input, response)) => {
                    dispatch_lxmf_send(
                        lxmf_service,
                        send_tasks,
                        input,
                        response,
                        wall_clock_millis(),
                    );
                }
                None => {
                    commands.close();
                    snapshots.set_runtime(DevelopmentNodeRuntime::Stopping);
                    return Ok(());
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LxmfHealthRefresh {
    Current,
    RetryNeeded,
}

async fn refresh_lxmf_health(
    service: &prns_lxmf::mailbox::DurableDirectLxmfService,
    snapshots: &SnapshotStore,
) -> Result<LxmfHealthRefresh, prns_lxmf::mailbox::MailboxFailure> {
    let snapshot = match service
        .snapshot(prns_lxmf::mailbox::MailboxListRequest {
            peer: None,
            direction: None,
            before: None,
            limit: 1,
        })
        .await
    {
        Ok(snapshot) => snapshot,
        Err(
            prns_lxmf::mailbox::MailboxFailure::Busy
            | prns_lxmf::mailbox::MailboxFailure::Unavailable(_),
        ) => {
            snapshots.set_lxmf_degraded();
            return Ok(LxmfHealthRefresh::RetryNeeded);
        }
        Err(failure @ prns_lxmf::mailbox::MailboxFailure::ResetRequired(_)) => {
            return Err(failure);
        }
    };
    match crate::lxmf::project_health(&snapshot) {
        Ok(health) => {
            snapshots.refresh_lxmf(health);
            Ok(LxmfHealthRefresh::Current)
        }
        Err(
            prns_lxmf::mailbox::MailboxFailure::Busy
            | prns_lxmf::mailbox::MailboxFailure::Unavailable(_),
        ) => {
            snapshots.set_lxmf_degraded();
            Ok(LxmfHealthRefresh::RetryNeeded)
        }
        Err(failure @ prns_lxmf::mailbox::MailboxFailure::ResetRequired(_)) => Err(failure),
    }
}

async fn refresh_running_lxmf_health(
    service: &prns_lxmf::mailbox::DurableDirectLxmfService,
    snapshots: &SnapshotStore,
) -> Result<bool, (DevelopmentNodeStopStage, String)> {
    match refresh_lxmf_health(service, snapshots).await {
        Ok(LxmfHealthRefresh::Current) => Ok(false),
        Ok(LxmfHealthRefresh::RetryNeeded) => Ok(true),
        Err(failure) => {
            let detail = publish_lxmf_storage_failure(snapshots, &failure);
            Err((DevelopmentNodeStopStage::Persistence, detail))
        }
    }
}

async fn refresh_running_lxmf_health_after_change(
    service: &prns_lxmf::mailbox::DurableDirectLxmfService,
    snapshots: &SnapshotStore,
    retry_pending: &mut bool,
    retry_timer: &mut tokio::time::Interval,
) -> WorkerResult {
    // A transient snapshot failure updates the service health and therefore
    // feeds this watch lane. Once a delayed retry is armed, consume those hints
    // without reading again or pushing the retry deadline forward.
    if *retry_pending {
        return Ok(());
    }
    *retry_pending = refresh_running_lxmf_health(service, snapshots).await?;
    if *retry_pending {
        retry_timer.reset();
    }
    Ok(())
}

fn lxmf_storage_failure_detail(
    snapshots: &SnapshotStore,
    failure: &prns_lxmf::mailbox::MailboxFailure,
) -> String {
    match failure {
        prns_lxmf::mailbox::MailboxFailure::Busy => {
            "The bounded development database lane is full.".to_owned()
        }
        prns_lxmf::mailbox::MailboxFailure::Unavailable(detail) => detail.clone(),
        prns_lxmf::mailbox::MailboxFailure::ResetRequired(reason) => {
            snapshots.set_local_host(LocalHostState::DevelopmentResetRequired {
                reason: reason.clone(),
            });
            reason.clone()
        }
    }
}

fn publish_lxmf_storage_failure(
    snapshots: &SnapshotStore,
    failure: &prns_lxmf::mailbox::MailboxFailure,
) -> String {
    let detail = lxmf_storage_failure_detail(snapshots, failure);
    snapshots.fail(DevelopmentNodeFailure {
        stage: DevelopmentNodeFailureStage::Storage,
        detail: detail.clone(),
    });
    detail
}

async fn drain_lxmf_response_tasks(send_tasks: &mut JoinSet<()>) -> WorkerResult {
    // These wrappers own already-admitted insert outcomes. Generation shutdown
    // may time out independently, but it must not abort a wrapper while the
    // application store can still report a definitive commit result.
    let mut first_failure = None;
    while let Some(completed) = send_tasks.join_next().await {
        if completed.is_err() && first_failure.is_none() {
            first_failure = Some((
                DevelopmentNodeStopStage::Worker,
                "An LXMF response task stopped before reporting its committed result.".to_owned(),
            ));
        }
    }
    first_failure.map_or(Ok(()), Err)
}

async fn stop_lxmf_service_and_drain(
    service: &prns_lxmf::mailbox::DurableDirectLxmfService,
    send_tasks: &mut JoinSet<()>,
) -> WorkerResult {
    let first_stop_result = service.stop().await;
    let response_result = drain_lxmf_response_tasks(send_tasks).await;
    let stop_result = match first_stop_result {
        Ok(()) => Ok(()),
        Err(_) => service.stop().await.map_err(|_| {
            (
                DevelopmentNodeStopStage::Worker,
                "The LXMF service did not finish bounded shutdown after admitted responses drained."
                    .to_owned(),
            )
        }),
    };
    stop_result.and(response_result)
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
        attachment.backend_info(),
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
    now: personal_rns::units::InstantMillis,
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
    let resolution = controls.resolve_candidate(&input.candidate_id, now);
    let (endpoint, expires_at) = match resolution {
        PairingCandidateResolution::Retained {
            endpoint,
            expires_at,
        } => (endpoint, expires_at),
        PairingCandidateResolution::Missing => {
            return pairing_failed(
                RemoteControlPairingFailureStage::Candidate,
                "The selected pairing session is no longer available.",
            )
        }
        PairingCandidateResolution::Expired => {
            publish_candidate_resolution(controls, snapshots, now, resolution);
            return pairing_failed(
                RemoteControlPairingFailureStage::Expired,
                "The selected pairing session expired.",
            );
        }
    };
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
            let stage = classify_pairing_initiation_failure(&error);
            pairing_failed_visible(
                controls,
                snapshots,
                stage,
                "The upstream RemoteControl pairing initiation failed.",
            )
        }
    }
}

fn classify_pairing_initiation_failure(
    error: &personal_rns::runtime::InitiateRemoteControlControllerPairingError,
) -> RemoteControlPairingFailureStage {
    use personal_rns::runtime::InitiateRemoteControlControllerPairingError;

    match error {
        InitiateRemoteControlControllerPairingError::EstablishLink(failure) => {
            match crate::remote_control::classify_establish_link_failure(failure) {
                crate::remote_control::EstablishLinkFailureClass::Route => {
                    RemoteControlPairingFailureStage::Route
                }
                crate::remote_control::EstablishLinkFailureClass::Link => {
                    RemoteControlPairingFailureStage::Link
                }
                crate::remote_control::EstablishLinkFailureClass::Node => {
                    RemoteControlPairingFailureStage::Node
                }
            }
        }
        InitiateRemoteControlControllerPairingError::Identify { .. } => {
            RemoteControlPairingFailureStage::Identification
        }
        InitiateRemoteControlControllerPairingError::ResponseExpired { .. } => {
            RemoteControlPairingFailureStage::Expired
        }
        InitiateRemoteControlControllerPairingError::Request { failure, .. } => {
            classify_pairing_request_failure(failure.cause)
        }
        InitiateRemoteControlControllerPairingError::ResponseNotAdvanced { .. }
        | InitiateRemoteControlControllerPairingError::Begin { .. } => {
            RemoteControlPairingFailureStage::Confirmation
        }
        InitiateRemoteControlControllerPairingError::NodeStopped { .. }
        | InitiateRemoteControlControllerPairingError::Busy { .. } => {
            RemoteControlPairingFailureStage::Node
        }
    }
}

const fn classify_pairing_request_failure(
    cause: personal_rns::engine::RemoteControlControllerPairingRequestFailureCause,
) -> RemoteControlPairingFailureStage {
    match cause {
        personal_rns::engine::RemoteControlControllerPairingRequestFailureCause::Request(
            failure,
        ) => match crate::remote_control::classify_send_request_failure(failure) {
            crate::remote_control::SendRequestFailureClass::Timeout => {
                RemoteControlPairingFailureStage::Timeout
            }
            crate::remote_control::SendRequestFailureClass::Link => {
                RemoteControlPairingFailureStage::Link
            }
            crate::remote_control::SendRequestFailureClass::Request => {
                RemoteControlPairingFailureStage::Request
            }
        },
        personal_rns::engine::RemoteControlControllerPairingRequestFailureCause::ResourceResponseUnsupported => {
            RemoteControlPairingFailureStage::Request
        }
    }
}

fn classify_pairing_approval_failure(
    failure: &personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure,
) -> RemoteControlPairingFailureStage {
    use personal_rns::engine::ApproveRemoteControlControllerPairingFailure;
    use personal_rns::remote_control::FailRemoteControlControllerPairingRequestOutcome;
    use personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure;

    match failure {
        ApproveRemoteControlControllerPairingControlFailure::Approve(
            ApproveRemoteControlControllerPairingFailure::Expired { .. },
        ) => RemoteControlPairingFailureStage::Expired,
        ApproveRemoteControlControllerPairingControlFailure::Approve(
            ApproveRemoteControlControllerPairingFailure::PersistenceInProgress { .. },
        ) => RemoteControlPairingFailureStage::Persistence,
        ApproveRemoteControlControllerPairingControlFailure::Approve(
            ApproveRemoteControlControllerPairingFailure::NoActiveAttempt
            | ApproveRemoteControlControllerPairingFailure::OfferNotReceived
            | ApproveRemoteControlControllerPairingFailure::AttemptMismatch { .. },
        ) => RemoteControlPairingFailureStage::Confirmation,
        ApproveRemoteControlControllerPairingControlFailure::Approve(
            ApproveRemoteControlControllerPairingFailure::RequestBuild { .. },
        ) => RemoteControlPairingFailureStage::Request,
        ApproveRemoteControlControllerPairingControlFailure::Request(request)
            if matches!(
                request.exchange,
                FailRemoteControlControllerPairingRequestOutcome::PersistenceInProgress { .. }
            ) =>
        {
            RemoteControlPairingFailureStage::Persistence
        }
        ApproveRemoteControlControllerPairingControlFailure::Request(request) => {
            classify_pairing_request_failure(request.cause)
        }
    }
}

const fn pairing_approval_failure_detail(stage: RemoteControlPairingFailureStage) -> &'static str {
    match stage {
        RemoteControlPairingFailureStage::Expired => {
            "The pairing attempt expired before approval completed."
        }
        RemoteControlPairingFailureStage::Timeout => {
            "The node did not finish pairing before the approval timed out."
        }
        RemoteControlPairingFailureStage::Route => {
            "No connection path to the node was available during approval."
        }
        RemoteControlPairingFailureStage::Link => {
            "The connection to the node closed before pairing finished."
        }
        RemoteControlPairingFailureStage::Persistence => {
            "Pairing could not advance while authorization was being saved."
        }
        RemoteControlPairingFailureStage::Request => {
            "The node could not complete the pairing approval request."
        }
        RemoteControlPairingFailureStage::Confirmation => {
            "The active pairing confirmation is no longer available."
        }
        RemoteControlPairingFailureStage::Node => {
            "This device went offline while pairing was being approved."
        }
        RemoteControlPairingFailureStage::Input
        | RemoteControlPairingFailureStage::Candidate
        | RemoteControlPairingFailureStage::Identification => {
            "Pairing approval could not be completed."
        }
    }
}

async fn approve_pairing(
    handle: &PrnsNodeHandle,
    controls: &mut PairingControls,
    snapshots: &SnapshotStore,
    input: RemoteControlPairingDecisionInput,
) -> RemoteControlPairingCommandOutcome {
    let Some(confirmation) = controls.take_confirmation_for_decision(&input.attempt_id) else {
        return pairing_failed(
            RemoteControlPairingFailureStage::Confirmation,
            "No RemoteControl confirmation is awaiting approval.",
        );
    };
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
            controls.restore_confirmation(confirmation);
            snapshots.update(|snapshot| snapshot.pairing = retained_confirmation);
            RemoteControlPairingCommandOutcome::Busy
        }
        Err(RemoteControlPairingControlError::NodeStopped) => pairing_failed_visible(
            controls,
            snapshots,
            RemoteControlPairingFailureStage::Node,
            "The Prns node stopped while approving the pairing attempt.",
        ),
        Err(RemoteControlPairingControlError::Failed(failure)) => {
            let stage = classify_pairing_approval_failure(&failure);
            pairing_failed_visible(
                controls,
                snapshots,
                stage,
                pairing_approval_failure_detail(stage),
            )
        }
    }
}

async fn reject_pairing(
    handle: &PrnsNodeHandle,
    controls: &mut PairingControls,
    snapshots: &SnapshotStore,
    input: RemoteControlPairingDecisionInput,
) -> RemoteControlPairingCommandOutcome {
    let Some(confirmation) = controls.take_confirmation_for_decision(&input.attempt_id) else {
        return pairing_failed(
            RemoteControlPairingFailureStage::Confirmation,
            "No RemoteControl confirmation is awaiting rejection.",
        );
    };
    let rejection = confirmation.rejection();
    match handle
        .reject_remote_control_controller_pairing(rejection)
        .await
    {
        Ok(_) => {
            let selected_candidate_id = controls.clear_attempt();
            snapshots.update(|snapshot| {
                remove_selected_candidate(snapshot, selected_candidate_id.as_deref());
                snapshot.pairing = RemoteControlPairingState::Rejected {
                    detail: "The controller rejected the pairing confirmation.".to_owned(),
                };
                snapshot.active_operation = None;
            });
            RemoteControlPairingCommandOutcome::Accepted {
                snapshot: Box::new(snapshots.read()),
            }
        }
        Err(RemoteControlPairingControlError::Busy) => {
            controls.restore_confirmation(confirmation);
            RemoteControlPairingCommandOutcome::Busy
        }
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
    const fn bluetooth_is_none(&self) -> bool {
        #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
        {
            self.bluetooth.is_none()
        }
        #[cfg(not(all(feature = "apple", any(target_os = "ios", target_os = "macos"))))]
        {
            true
        }
    }

    fn metadata(&self) -> Vec<HostInterfaceAttachment> {
        #[cfg(not(any(feature = "apple", feature = "host-test")))]
        return Vec::new();

        #[cfg(any(feature = "apple", feature = "host-test"))]
        let mut attachments = Vec::new();
        #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
        if let Some(attached) = self.bluetooth.as_ref() {
            attachments.push(HostInterfaceAttachment::new(
                attached.id(),
                InterfaceKind::AutomaticBluetoothLe,
            ));
        }
        #[cfg(any(feature = "apple", feature = "host-test"))]
        if let Some(attached) = self.tcp.as_ref() {
            attachments.push(HostInterfaceAttachment::new(
                attached.id(),
                InterfaceKind::TcpClient,
            ));
        }
        #[cfg(any(feature = "apple", feature = "host-test"))]
        attachments
    }

    fn backend_info(&self) -> BackendInfo {
        #[cfg(any(feature = "apple", feature = "host-test"))]
        {
            let mut capabilities = vec![Capability::Bluetooth];
            let mut interface_kinds = vec![InterfaceKind::AutomaticBluetoothLe];
            if self.tcp.is_some() {
                capabilities.push(Capability::TcpClient);
                interface_kinds.push(InterfaceKind::TcpClient);
            }
            BackendInfo::new(BackendKind::Native, capabilities, interface_kinds)
        }

        #[cfg(not(any(feature = "apple", feature = "host-test")))]
        BackendInfo::new(
            BackendKind::Native,
            [Capability::Bluetooth],
            [InterfaceKind::AutomaticBluetoothLe],
        )
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
    let selected_candidate_id = controls.clear_attempt();
    snapshots.update(|snapshot| {
        remove_selected_candidate(snapshot, selected_candidate_id.as_deref());
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

    #[test]
    fn worker_terminal_paths_do_not_leave_an_announcement_pending() {
        for result in [
            Ok(()),
            Err((DevelopmentNodeStopStage::Node, "unexpected exit".to_owned())),
        ] {
            let snapshots = SnapshotStore::new();
            snapshots.set_runtime(DevelopmentNodeRuntime::Running);
            snapshots.update(|snapshot| {
                snapshot.lxmf.state = crate::contract::LxmfHealthState::Ready;
                snapshot.last_announcement = Some(RemoteControlAnnounceOperation {
                    operation_id: U64String::from(1),
                    target_identity_fingerprint: vec![1; 16],
                    status: RemoteControlAnnounceStatus::Pending,
                });
                snapshot.active_operation = Some(DevelopmentNodeOperation {
                    kind: DevelopmentNodeOperationKind::AnnounceSelf,
                    started_at_millis: U64String::from(0),
                });
            });
            let admitted = AtomicBool::new(true);
            settle_worker_result(&admitted, &snapshots, &result);
            assert!(matches!(
                snapshots
                    .read()
                    .last_announcement
                    .map(|operation| operation.status),
                Some(RemoteControlAnnounceStatus::OutcomeUnknown {
                    reason: crate::contract::RemoteControlAnnounceUnknownReason::NodeStopped
                })
            ));
            assert!(snapshots.read().active_operation.is_none());
            if result.is_ok() {
                let stopped = snapshots.read();
                assert_eq!(stopped.runtime, DevelopmentNodeRuntime::Stopped);
                assert_eq!(
                    stopped.lxmf.state,
                    crate::contract::LxmfHealthState::Stopped
                );
                assert!(matches!(stopped.local_host, LocalHostState::Stopped { .. }));
                assert!(stopped.paired_targets.is_empty());
            }
            assert!(!admitted.load(Ordering::Acquire));
        }
    }

    #[test]
    fn announcement_admission_cannot_publish_after_stop_owns_the_supervisor() {
        let supervisor = Arc::new(Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(SupervisorState::default()),
        });
        supervisor
            .snapshots
            .set_runtime(DevelopmentNodeRuntime::Running);
        let (commands, mut command_rx) = mpsc::channel(1);
        let (sender, _shutdown_rx) = watch::channel(false);
        let (done_tx, done) = std_mpsc::channel();
        supervisor.lock_state().worker = Some(test_worker(
            commands,
            ShutdownSignal {
                sender,
                requested: Arc::new(AtomicBool::new(false)),
            },
            done,
            None,
            PathBuf::from("/tmp/prns/announcement-test"),
        ));

        let accepted = announce_self_with_supervisor(
            &supervisor,
            AnnounceRemoteControlTargetInput {
                target_identity_fingerprint: vec![1; 16],
            },
        );
        assert!(matches!(
            accepted,
            RemoteControlAnnounceOutcome::Accepted { .. }
        ));
        assert!(matches!(
            announce_self_with_supervisor(
                &supervisor,
                AnnounceRemoteControlTargetInput {
                    target_identity_fingerprint: vec![1; 16]
                }
            ),
            RemoteControlAnnounceOutcome::Busy
        ));
        assert!(matches!(
            command_rx.try_recv(),
            Ok(Command::AnnounceSelf(_))
        ));
        // Model a consumed command that never settles before priority shutdown.
        let mut state = supervisor.lock_state();
        supervisor.snapshots.begin_stop(U64String::from(2));
        let blocked = Arc::clone(&supervisor);
        let (started_tx, started_rx) = std_mpsc::channel();
        let submit = std::thread::spawn(move || {
            started_tx.send(()).expect("submission thread started");
            announce_self_with_supervisor(
                &blocked,
                AnnounceRemoteControlTargetInput {
                    target_identity_fingerprint: vec![2; 16],
                },
            )
        });
        started_rx
            .recv()
            .expect("submission reached held supervisor");
        done_tx.send(Ok(())).expect("worker shutdown completed");
        assert_eq!(
            stop_locked(&supervisor, &mut state),
            DevelopmentNodeStopOutcome::Stopped
        );
        drop(state);
        assert!(matches!(
            submit.join().expect("submission returned"),
            RemoteControlAnnounceOutcome::Failed {
                stage: RemoteControlAnnounceFailureStage::Node
            }
        ));
        assert!(command_rx.try_recv().is_err());
        assert_eq!(
            supervisor.snapshots.read().runtime,
            DevelopmentNodeRuntime::Stopped
        );
        assert!(supervisor.snapshots.read().active_operation.is_none());
        assert!(matches!(
            supervisor
                .snapshots
                .read()
                .last_announcement
                .map(|operation| operation.status),
            Some(RemoteControlAnnounceStatus::OutcomeUnknown { .. })
        ));
    }

    fn test_worker(
        commands: mpsc::Sender<Command>,
        shutdown: ShutdownSignal,
        done: std_mpsc::Receiver<WorkerResult>,
        join: Option<JoinHandle<()>>,
        storage_root: PathBuf,
    ) -> Worker {
        #[cfg(all(feature = "apple", target_os = "ios"))]
        let bluetooth_owner_key = AppleBluetoothOwnerKey {
            storage_root: storage_root.clone(),
            preparation: AppleBluetoothPreparation::WithoutRestoration,
            identity: BleIdentity::new([0xa5; 16]),
        };
        Worker {
            commands,
            shutdown,
            done,
            join,
            incomplete_stop: None,
            storage_root,
            #[cfg(all(feature = "apple", target_os = "ios"))]
            bluetooth_owner_key,
        }
    }

    fn test_supervisor_state(
        worker: Option<Worker>,
        application_owner: Option<DevelopmentStoreOwner>,
        identity_owner: Option<IdentityOwner>,
    ) -> SupervisorState {
        SupervisorState {
            worker,
            application_owner,
            identity_owner,
            #[cfg(all(feature = "apple", target_os = "ios"))]
            pending_apple_bluetooth: None,
        }
    }

    #[derive(Default)]
    struct PendingProofNetwork {
        send_entered: tokio::sync::Notify,
        release_send: tokio::sync::Notify,
        send_count: std::sync::atomic::AtomicUsize,
    }

    struct BlockingInsertSubmitter {
        inner: Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
        insert_entered: tokio::sync::Notify,
        release_insert: tokio::sync::Notify,
    }

    impl BlockingInsertSubmitter {
        fn new(inner: Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>) -> Self {
            Self {
                inner,
                insert_entered: tokio::sync::Notify::new(),
                release_insert: tokio::sync::Notify::new(),
            }
        }
    }

    impl prns_lxmf::mailbox::MailboxSubmitter for BlockingInsertSubmitter {
        fn submit(
            &self,
            request: prns_lxmf::mailbox::MailboxRequest,
        ) -> prns_lxmf::mailbox::MailboxFuture<'_> {
            Box::pin(async move {
                if matches!(
                    &request,
                    prns_lxmf::mailbox::MailboxRequest::InsertOutbound(_)
                ) {
                    self.insert_entered.notify_one();
                    self.release_insert.notified().await;
                }
                self.inner.submit(request).await
            })
        }
    }

    struct ResetCompletionSubmitter {
        inner: Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
    }

    impl prns_lxmf::mailbox::MailboxSubmitter for ResetCompletionSubmitter {
        fn submit(
            &self,
            request: prns_lxmf::mailbox::MailboxRequest,
        ) -> prns_lxmf::mailbox::MailboxFuture<'_> {
            Box::pin(async move {
                if matches!(
                    &request,
                    prns_lxmf::mailbox::MailboxRequest::CompleteAttempt { .. }
                ) {
                    return Err(prns_lxmf::mailbox::MailboxFailure::ResetRequired(
                        "mailbox record became unreadable".to_owned(),
                    ));
                }
                self.inner.submit(request).await
            })
        }
    }

    struct TransientListFailureSubmitter {
        inner: Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
        remaining_failures: std::sync::atomic::AtomicUsize,
        list_submissions: std::sync::atomic::AtomicUsize,
    }

    impl prns_lxmf::mailbox::MailboxSubmitter for TransientListFailureSubmitter {
        fn submit(
            &self,
            request: prns_lxmf::mailbox::MailboxRequest,
        ) -> prns_lxmf::mailbox::MailboxFuture<'_> {
            Box::pin(async move {
                if matches!(&request, prns_lxmf::mailbox::MailboxRequest::List(_)) {
                    self.list_submissions.fetch_add(1, Ordering::AcqRel);
                    let remaining = self.remaining_failures.fetch_update(
                        Ordering::AcqRel,
                        Ordering::Acquire,
                        |remaining| remaining.checked_sub(1),
                    );
                    if let Ok(remaining) = remaining {
                        return Err(if remaining == 2 {
                            prns_lxmf::mailbox::MailboxFailure::Busy
                        } else {
                            prns_lxmf::mailbox::MailboxFailure::Unavailable(
                                "transient mailbox read failure".to_owned(),
                            )
                        });
                    }
                }
                self.inner.submit(request).await
            })
        }
    }

    impl prns_lxmf::direct::DirectNetwork for PendingProofNetwork {
        fn has_route(
            &self,
            _destination: [u8; 16],
        ) -> prns_lxmf::direct::DirectNetworkFuture<'_, bool> {
            Box::pin(async { true })
        }

        fn request_path(
            &self,
            _destination: [u8; 16],
        ) -> prns_lxmf::direct::DirectNetworkFuture<
            '_,
            Result<(), prns_lxmf::direct::DirectSendFailure>,
        > {
            Box::pin(async { Ok(()) })
        }

        fn establish_link(
            &self,
            _destination: [u8; 16],
        ) -> prns_lxmf::direct::DirectNetworkFuture<
            '_,
            Result<[u8; 16], prns_lxmf::direct::DirectSendFailure>,
        > {
            Box::pin(async { Ok([0xa5; 16]) })
        }

        fn send_link_packet(
            &self,
            _link: [u8; 16],
            _complete_wire: Vec<u8>,
        ) -> prns_lxmf::direct::DirectNetworkFuture<
            '_,
            Result<prns_lxmf::direct::DirectDeliveryReceipt, prns_lxmf::direct::DirectSendFailure>,
        > {
            Box::pin(async move {
                self.send_count.fetch_add(1, Ordering::AcqRel);
                self.send_entered.notify_one();
                self.release_send.notified().await;
                Ok(prns_lxmf::direct::DirectDeliveryReceipt { rtt_millis: 23 })
            })
        }

        fn destination_public_key(
            &self,
            _destination: [u8; 16],
        ) -> prns_lxmf::direct::DirectNetworkFuture<'_, Option<[u8; 64]>> {
            Box::pin(async { None })
        }

        fn announce(
            &self,
            _destination: [u8; 16],
        ) -> prns_lxmf::direct::DirectNetworkFuture<
            '_,
            Result<(), prns_lxmf::direct::DirectAnnounceFailure>,
        > {
            Box::pin(async { Ok(()) })
        }
    }

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

    fn seed_failed_outbound_message(
        supervisor: &Supervisor,
        paths: &NodeStoragePaths,
        destination: [u8; 16],
    ) {
        let identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
            &[0x71; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
        )
        .expect("the fixed LXMF destination name is valid");
        let source = identity.destination();
        let mut encoded = [0_u8; prns_lxmf::wire::MAX_BASIC_LXMF_WIRE_BYTES];
        let prepared = prns_lxmf::wire::compose_basic_direct_lxmf(
            destination,
            source,
            1_700_000_009_000,
            b"Offline",
            b"Exact wire",
            None,
            &identity,
            &mut encoded,
        )
        .expect("the small direct message fits");
        let inserted = {
            let mut state = supervisor.lock_state();
            ensure_application_owner_locked(&mut state, paths)
                .expect("application owner")
                .admit_mailbox(prns_lxmf::mailbox::MailboxRequest::InsertOutbound(
                    prns_lxmf::mailbox::NewOutboundMessage {
                        message_id: prepared.message_id(),
                        source,
                        destination,
                        timestamp_unix_ms: 1_700_000_009_000,
                        title: b"Offline".to_vec(),
                        content: b"Exact wire".to_vec(),
                        exact_wire: encoded[..usize::from(prepared.wire_len())].to_vec(),
                    },
                ))
                .expect("insert admission")
        }
        .recv()
        .expect("insert reply")
        .expect("insert succeeds");
        assert!(matches!(
            inserted,
            prns_lxmf::mailbox::MailboxReply::OutboundInserted { .. }
        ));
        {
            let mut state = supervisor.lock_state();
            ensure_application_owner_locked(&mut state, paths)
                .expect("application owner")
                .admit_mailbox(prns_lxmf::mailbox::MailboxRequest::CompleteAttempt {
                    key: prns_lxmf::mailbox::AttemptKey {
                        local_record_id: 1,
                        generation: 1,
                    },
                    completion: prns_lxmf::mailbox::AttemptCompletion::Failed {
                        failure: prns_lxmf::direct::DirectSendFailure::DeliveryTimedOut,
                    },
                })
                .expect("failure admission")
        }
        .recv()
        .expect("failure reply")
        .expect("failure succeeds");
    }

    fn install_finished_failed_worker(supervisor: &Supervisor, paths: &NodeStoragePaths) {
        let (done_tx, done) = std_mpsc::sync_channel(1);
        let join = std::thread::spawn(move || {
            done_tx
                .send(Err((
                    DevelopmentNodeStopStage::Node,
                    "node stopped unexpectedly".to_owned(),
                )))
                .expect("terminal worker result");
        });
        while !join.is_finished() {
            std::thread::yield_now();
        }
        let (commands, _commands_rx) = mpsc::channel(1);
        let (shutdown_sender, _shutdown_rx) = watch::channel(false);
        supervisor.lock_state().worker = Some(test_worker(
            commands,
            ShutdownSignal {
                sender: shutdown_sender,
                requested: Arc::new(AtomicBool::new(false)),
            },
            done,
            Some(join),
            paths.root.clone(),
        ));
    }

    fn assert_offline_list_retry_cancel(
        supervisor: &Supervisor,
        storage: &Path,
        destination: [u8; 16],
    ) {
        let list_input = ListLxmfMessagesInput {
            peer: Some(destination),
            before: None,
            limit: 25,
        };
        let LxmfMessageListOutcome::Listed { messages } =
            list_lxmf_messages_with_supervisor(supervisor, storage, list_input.clone())
        else {
            panic!("the offline mailbox did not return its durable row");
        };
        assert!(matches!(
            messages[0].delivery_state,
            crate::contract::LxmfDeliveryState::Failed {
                failed_attempts: U64String(ref value),
                last_failure: crate::contract::LxmfDeliveryFailure::DeliveryTimedOut,
            } if value == "1"
        ));

        assert_eq!(
            retry_lxmf_message_with_supervisor(
                supervisor,
                storage,
                RetryLxmfMessageInput {
                    local_record_id: U64String::from(1),
                },
            ),
            RetryLxmfMessageOutcome::Accepted {
                local_record_id: U64String::from(1),
            }
        );
        let LxmfMessageListOutcome::Listed { messages } =
            list_lxmf_messages_with_supervisor(supervisor, storage, list_input.clone())
        else {
            panic!("the retried mailbox row was not listed");
        };
        assert_eq!(
            messages[0].delivery_state,
            crate::contract::LxmfDeliveryState::Queued {
                failed_attempts: U64String::from(1),
            }
        );

        assert_eq!(
            cancel_lxmf_message_with_supervisor(
                supervisor,
                storage,
                CancelLxmfMessageInput {
                    local_record_id: U64String::from(1),
                },
            ),
            CancelLxmfMessageOutcome::Cancelled {
                local_record_id: U64String::from(1),
            }
        );
        let LxmfMessageListOutcome::Listed { messages } =
            list_lxmf_messages_with_supervisor(supervisor, storage, list_input)
        else {
            panic!("the cancelled mailbox row was not listed");
        };
        assert!(matches!(
            messages[0].delivery_state,
            crate::contract::LxmfDeliveryState::Cancelled { .. }
        ));
    }

    async fn learn_pending_peer(
        service: &prns_lxmf::mailbox::DurableDirectLxmfService,
    ) -> [u8; 16] {
        use personal_rns::interfaces::InterfaceId;
        use personal_rns::routing::announce::{
            derive_single_destination_hash, AnnounceObservation,
        };
        use personal_rns::units::{HopCount, InstantMillis};

        let peer_material = PrivateIdentityMaterial::from_bytes(
            [0x52; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
        );
        let peer_destination = derive_single_destination_hash(
            &peer_material.identity_hash(),
            prns_lxmf::wire::LXMF_APP_NAME,
            prns_lxmf::wire::LXMF_DELIVERY_ASPECTS,
        )
        .expect("the fixed LXMF destination name is valid");
        let mut announce = [0_u8; 64];
        let announce_len =
            prns_lxmf::wire::encode_current_lxmf_announce(b"Pending peer", &mut announce)
                .expect("the small announce fits");
        let mut refresh = service.subscribe();
        assert_eq!(
            service
                .callbacks()
                .on_accepted_announce(AnnounceObservation {
                    destination: peer_destination,
                    announced_identity: peer_material.identity_hash(),
                    hops: HopCount(1),
                    source_interface: InterfaceId::new([4, 1, 2, 3, 4, 5, 6, 7]),
                    arrived_at: InstantMillis(4_200),
                    app_data: &announce[..announce_len],
                    is_path_response: false,
                }),
            prns_lxmf::direct::CallbackOutcome::Enqueued
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if service
                    .snapshot(prns_lxmf::mailbox::MailboxListRequest {
                        peer: None,
                        direction: None,
                        before: None,
                        limit: 25,
                    })
                    .await
                    .expect("the mailbox query succeeds")
                    .peers
                    .is_empty()
                {
                    refresh
                        .changed()
                        .await
                        .expect("the service remains running");
                } else {
                    break;
                }
            }
        })
        .await
        .expect("the peer observation is consumed");
        *peer_destination.as_bytes()
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
    fn pairing_request_timeout_has_a_distinct_failure_stage() {
        use personal_rns::engine::{
            RemoteControlControllerPairingRequestFailureCause, SendRequestFailure,
        };

        assert_eq!(
            classify_pairing_request_failure(
                RemoteControlControllerPairingRequestFailureCause::Request(
                    SendRequestFailure::Timeout,
                ),
            ),
            RemoteControlPairingFailureStage::Timeout
        );
        assert_eq!(
            classify_pairing_request_failure(
                RemoteControlControllerPairingRequestFailureCause::Request(
                    SendRequestFailure::WriteFailed,
                ),
            ),
            RemoteControlPairingFailureStage::Request
        );
        assert_eq!(
            classify_pairing_request_failure(
                RemoteControlControllerPairingRequestFailureCause::ResourceResponseUnsupported,
            ),
            RemoteControlPairingFailureStage::Request
        );
    }

    #[test]
    fn approval_failures_preserve_actionable_failure_stages() {
        use personal_rns::engine::{
            ApproveRemoteControlControllerPairingFailure,
            RemoteControlControllerPairingRequestBuildError,
            RemoteControlControllerPairingRequestFailure,
            RemoteControlControllerPairingRequestFailureCause, SendRequestFailure,
        };
        use personal_rns::remote_control::{
            FailRemoteControlControllerPairingRequestOutcome, RemoteControlControllerPairingAborted,
        };
        use personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure;

        let (attempt_id, context) = pairing_attempt_fixture(0xA4);
        let aborted = RemoteControlControllerPairingAborted::AwaitingCompletion {
            attempt_id,
            context,
        };
        let cases = [
            (
                ApproveRemoteControlControllerPairingControlFailure::Request(
                    RemoteControlControllerPairingRequestFailure {
                        cause: RemoteControlControllerPairingRequestFailureCause::Request(
                            SendRequestFailure::Timeout,
                        ),
                        exchange: FailRemoteControlControllerPairingRequestOutcome::Aborted {
                            aborted,
                        },
                    },
                ),
                RemoteControlPairingFailureStage::Timeout,
                "The node did not finish pairing before the approval timed out.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Request(
                    RemoteControlControllerPairingRequestFailure {
                        cause: RemoteControlControllerPairingRequestFailureCause::Request(
                            SendRequestFailure::LinkClosed,
                        ),
                        exchange: FailRemoteControlControllerPairingRequestOutcome::Aborted {
                            aborted,
                        },
                    },
                ),
                RemoteControlPairingFailureStage::Link,
                "The connection to the node closed before pairing finished.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Request(
                    RemoteControlControllerPairingRequestFailure {
                        cause: RemoteControlControllerPairingRequestFailureCause::ResourceResponseUnsupported,
                        exchange: FailRemoteControlControllerPairingRequestOutcome::NoActiveAttempt,
                    },
                ),
                RemoteControlPairingFailureStage::Request,
                "The node could not complete the pairing approval request.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Approve(
                    ApproveRemoteControlControllerPairingFailure::Expired {
                        expired: aborted,
                        retired_link: context.link_id(),
                    },
                ),
                RemoteControlPairingFailureStage::Expired,
                "The pairing attempt expired before approval completed.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Approve(
                    ApproveRemoteControlControllerPairingFailure::PersistenceInProgress {
                        attempt_id,
                    },
                ),
                RemoteControlPairingFailureStage::Persistence,
                "Pairing could not advance while authorization was being saved.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Request(
                    RemoteControlControllerPairingRequestFailure {
                        cause: RemoteControlControllerPairingRequestFailureCause::Request(
                            SendRequestFailure::WriteFailed,
                        ),
                        exchange:
                            FailRemoteControlControllerPairingRequestOutcome::PersistenceInProgress {
                                attempt_id,
                            },
                    },
                ),
                RemoteControlPairingFailureStage::Persistence,
                "Pairing could not advance while authorization was being saved.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Approve(
                    ApproveRemoteControlControllerPairingFailure::NoActiveAttempt,
                ),
                RemoteControlPairingFailureStage::Confirmation,
                "The active pairing confirmation is no longer available.",
            ),
            (
                ApproveRemoteControlControllerPairingControlFailure::Approve(
                    ApproveRemoteControlControllerPairingFailure::RequestBuild {
                        failure: RemoteControlControllerPairingRequestBuildError::Capacity {
                            required: 2,
                            maximum: 1,
                        },
                        rollback: FailRemoteControlControllerPairingRequestOutcome::NoActiveAttempt,
                    },
                ),
                RemoteControlPairingFailureStage::Request,
                "The node could not complete the pairing approval request.",
            ),
        ];

        for (failure, expected_stage, expected_detail) in cases {
            let stage = classify_pairing_approval_failure(&failure);
            assert_eq!(stage, expected_stage);
            assert_eq!(pairing_approval_failure_detail(stage), expected_detail);
            assert!(!expected_detail.contains("RemoteControl"));
            assert!(!expected_detail.contains("upstream"));
        }
    }

    fn pairing_attempt_fixture(
        fill: u8,
    ) -> (
        personal_rns::remote_control::RemoteControlPairingAttemptId,
        personal_rns::remote_control::RemoteControlPairingContext,
    ) {
        use personal_rns::identity::in_memory::InMemoryNodeIdentity;
        use personal_rns::identity::vault::IdentitySecretKey;
        use personal_rns::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
        use personal_rns::remote_control::{
            RemoteControlControllerIdentity, RemoteControlPairingAttemptId,
            RemoteControlPairingAttemptTimeout, RemoteControlPairingBegin,
            RemoteControlPairingContext, RemoteControlPairingIdentity,
            RemoteControlPairingInvitationCode, RemoteControlPairingPermissions,
            RemoteControlPairingPreparedOffer, RemoteControlRequestSet,
        };
        use personal_rns::routing::links::LinkId;
        use personal_rns::units::DurationMillis;

        let controller_signer = InMemoryNodeIdentity::from_secret_key_bytes(
            &IdentitySecretKey::new([fill; personal_rns::identity::IDENTITY_SECRET_KEY_LEN]),
        );
        let controller = RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: controller_signer.encryption_public_key(),
            signing: controller_signer.signing_public_key(),
        });
        let target_signer = InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
            [fill.wrapping_add(1); personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
        ));
        let endpoint = RemoteControlPairingIdentity::new(IdentityHash::new([fill; 16])).endpoint();
        let context =
            RemoteControlPairingContext::new(endpoint, LinkId::new([fill.wrapping_add(2); 16]));
        let begin = RemoteControlPairingBegin::new(
            controller,
            endpoint,
            RemoteControlPairingInvitationCode::from_value(u32::from(fill)),
        );
        let prepared = RemoteControlPairingPreparedOffer::new(
            &target_signer,
            context,
            &begin,
            RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::all())
                .expect("nonempty permissions"),
            RemoteControlPairingAttemptTimeout::try_from(DurationMillis(5_000))
                .expect("valid attempt timeout"),
        );
        (
            RemoteControlPairingAttemptId::from(prepared.transcript()),
            context,
        )
    }

    #[test]
    fn approval_wait_covers_the_longest_protocol_attempt() {
        let longest_attempt = Duration::from_millis(
            personal_rns::remote_control::MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT.0,
        );

        assert!(PAIRING_APPROVAL_TIMEOUT > longest_attempt);
        assert!(PAIRING_APPROVAL_TIMEOUT > COMMAND_TIMEOUT);
    }

    #[test]
    fn development_tcp_target_requires_an_explicit_nonzero_ip_port() {
        assert_eq!(validate_development_tcp_target(None), Ok(None));
        assert_eq!(
            validate_development_tcp_target(Some("192.0.2.1:4242")),
            Ok(Some("192.0.2.1:4242".to_owned()))
        );
        assert_eq!(
            validate_development_tcp_target(Some("[2001:db8::1]:4242")),
            Ok(Some("[2001:db8::1]:4242".to_owned()))
        );
        for invalid in ["example.com:4242", "192.0.2.1", "192.0.2.1:0", ""] {
            assert!(validate_development_tcp_target(Some(invalid)).is_err());
        }
    }

    #[test]
    fn apple_central_restoration_identifier_is_nonempty() {
        assert!(AppleBluetoothPreparation::CentralOnlyRestoration {
            central: "rs.reticulum.prns.dev.bluetooth-auto.central.v1".to_owned(),
        }
        .validate()
        .is_ok());
        assert!(AppleBluetoothPreparation::CentralOnlyRestoration {
            central: String::new(),
        }
        .validate()
        .is_err());
    }

    fn apple_owner_key(root: &str, central: &str, identity: u8) -> AppleBluetoothOwnerKey {
        AppleBluetoothOwnerKey {
            storage_root: PathBuf::from(root),
            preparation: AppleBluetoothPreparation::CentralOnlyRestoration {
                central: central.to_owned(),
            },
            identity: BleIdentity::new([identity; 16]),
        }
    }

    #[test]
    fn prepared_apple_bluetooth_handoff_is_exact_and_single_use() {
        let expected = apple_owner_key("/tmp/prns/development", "central", 0x41);
        let mut pending = Some(PreparedAppleBluetoothOwner {
            key: expected.clone(),
            prepared: "single owner",
        });

        for mismatch in [
            apple_owner_key("/different/prns/development", "central", 0x41),
            apple_owner_key("/tmp/prns/development", "different-central", 0x41),
            apple_owner_key("/tmp/prns/development", "central", 0x42),
        ] {
            assert!(take_matching_prepared_owner(&mut pending, &mismatch).is_err());
            assert!(
                pending.is_some(),
                "a mismatch must preserve the existing owner"
            );
        }

        assert_eq!(
            take_matching_prepared_owner(&mut pending, &expected),
            Ok(Some("single owner"))
        );
        assert!(pending.is_none());
        assert_eq!(
            take_matching_prepared_owner(&mut pending, &expected),
            Ok(None)
        );
    }

    #[test]
    fn stop_discard_helper_drops_a_pending_apple_bluetooth_owner() {
        let mut pending = Some(PreparedAppleBluetoothOwner {
            key: apple_owner_key("/tmp/prns/development", "central", 0x41),
            prepared: "pending owner",
        });

        discard_prepared_owner(&mut pending);

        assert!(pending.is_none());
    }

    #[test]
    fn invalid_early_restoration_configuration_fails_before_platform_work() {
        assert_eq!(
            prepare_apple_bluetooth_central_restoration(
                Path::new("/tmp/prns/development"),
                String::new(),
            ),
            AppleBluetoothRestorationPreparationOutcome::Failed {
                stage: AppleBluetoothRestorationPreparationFailureStage::Contract,
                detail: "central CoreBluetooth restoration identifier must not be empty".to_owned(),
            }
        );
    }

    #[test]
    fn invalid_restoration_configuration_cannot_overwrite_a_running_generation() {
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(SupervisorState::default()),
        };
        supervisor
            .snapshots
            .set_runtime(DevelopmentNodeRuntime::Running);
        let (commands, _commands_rx) = mpsc::channel(1);
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let (_done_tx, done) = std_mpsc::channel();
        supervisor.lock_state().worker = Some(test_worker(
            commands,
            ShutdownSignal {
                sender: shutdown_tx,
                requested: Arc::new(AtomicBool::new(false)),
            },
            done,
            None,
            PathBuf::from("/tmp/prns/running"),
        ));

        let outcome = start_configured_with_supervisor(
            &supervisor,
            Path::new("/tmp/prns/running"),
            DevelopmentNodeStartInput {
                development_tcp_target: None,
            },
            AppleBluetoothPreparation::CentralOnlyRestoration {
                central: String::new(),
            },
        );

        assert!(matches!(
            outcome,
            DevelopmentNodeStartOutcome::AlreadyRunning { .. }
        ));
        assert_eq!(
            supervisor.snapshots.read().runtime,
            DevelopmentNodeRuntime::Running
        );
        supervisor.lock_state().worker = None;
    }

    #[test]
    fn tracked_lxmf_send_tasks_are_bounded() {
        assert!(lxmf_send_has_capacity(0));
        assert!(lxmf_send_has_capacity(LXMF_SEND_TASK_CAPACITY - 1));
        assert!(!lxmf_send_has_capacity(LXMF_SEND_TASK_CAPACITY));
        assert!(!lxmf_send_has_capacity(LXMF_SEND_TASK_CAPACITY + 1));
    }

    #[test]
    fn admitted_send_waiter_requires_a_definitive_commit_outcome() {
        let (response, receiver) = std_mpsc::sync_channel(1);
        let waiter = std::thread::spawn(move || wait_for_admitted_lxmf_send(receiver));
        std::thread::sleep(Duration::from_millis(25));
        assert!(!waiter.is_finished());

        response
            .send(SendDirectTextOutcome::Accepted {
                local_record_id: U64String::from(7),
            })
            .expect("publish definitive commit outcome");
        assert_eq!(
            waiter.join().expect("waiter joins"),
            SendDirectTextOutcome::Accepted {
                local_record_id: U64String::from(7),
            }
        );
    }

    #[test]
    fn durable_mailbox_bypasses_the_generation_only_when_no_worker_exists() {
        assert_eq!(
            durable_mailbox_access(DevelopmentNodeRuntime::Running, true),
            DurableMailboxAccess::RunningGeneration
        );
        for runtime in [
            DevelopmentNodeRuntime::Stopped,
            DevelopmentNodeRuntime::Failed,
        ] {
            assert_eq!(
                durable_mailbox_access(runtime, false),
                DurableMailboxAccess::Offline
            );
        }
        for runtime in [
            DevelopmentNodeRuntime::Stopped,
            DevelopmentNodeRuntime::Starting,
            DevelopmentNodeRuntime::Running,
            DevelopmentNodeRuntime::Stopping,
            DevelopmentNodeRuntime::Failed,
        ] {
            assert_eq!(
                durable_mailbox_access(runtime, runtime != DevelopmentNodeRuntime::Running),
                DurableMailboxAccess::GenerationTransition
            );
        }
    }

    #[test]
    fn stopped_generation_lists_retries_and_cancels_through_the_store_owner() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");
        let paths = prepare_storage(&storage).expect("private storage");
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(SupervisorState::default()),
        };
        let destination = [0x42; 16];
        seed_failed_outbound_message(&supervisor, &paths, destination);
        assert_offline_list_retry_cancel(&supervisor, &storage, destination);

        supervisor
            .lock_state()
            .application_owner
            .take()
            .expect("offline operations opened one application owner")
            .close()
            .expect("application owner closes");
    }

    #[test]
    fn admitted_offline_mutations_wait_for_their_definitive_store_outcomes() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");
        let paths = prepare_storage(&storage).expect("private storage");
        let supervisor = Arc::new(Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(SupervisorState::default()),
        });
        let destination = [0x42; 16];
        seed_failed_outbound_message(&supervisor, &paths, destination);

        let (retry_entered_tx, retry_entered_rx) = std_mpsc::sync_channel(1);
        let (retry_release_tx, retry_release_rx) = std_mpsc::sync_channel(1);
        supervisor
            .lock_state()
            .application_owner
            .as_ref()
            .expect("application owner")
            .admit_test_barrier(retry_entered_tx, retry_release_rx)
            .expect("retry barrier admission");
        retry_entered_rx
            .recv()
            .expect("owner entered retry barrier");
        let retry_supervisor = Arc::clone(&supervisor);
        let retry_storage = storage.clone();
        let retry = std::thread::spawn(move || {
            retry_lxmf_message_with_supervisor(
                &retry_supervisor,
                &retry_storage,
                RetryLxmfMessageInput {
                    local_record_id: U64String::from(1),
                },
            )
        });
        std::thread::sleep(LXMF_QUERY_TIMEOUT + Duration::from_millis(25));
        assert!(!retry.is_finished());
        retry_release_tx.send(()).expect("release retry");
        assert_eq!(
            retry.join().expect("retry waiter joins"),
            RetryLxmfMessageOutcome::Accepted {
                local_record_id: U64String::from(1),
            }
        );

        let (cancel_entered_tx, cancel_entered_rx) = std_mpsc::sync_channel(1);
        let (cancel_release_tx, cancel_release_rx) = std_mpsc::sync_channel(1);
        supervisor
            .lock_state()
            .application_owner
            .as_ref()
            .expect("application owner")
            .admit_test_barrier(cancel_entered_tx, cancel_release_rx)
            .expect("cancel barrier admission");
        cancel_entered_rx
            .recv()
            .expect("owner entered cancel barrier");
        let cancel_supervisor = Arc::clone(&supervisor);
        let cancel_storage = storage.clone();
        let cancel = std::thread::spawn(move || {
            cancel_lxmf_message_with_supervisor(
                &cancel_supervisor,
                &cancel_storage,
                CancelLxmfMessageInput {
                    local_record_id: U64String::from(1),
                },
            )
        });
        std::thread::sleep(LXMF_QUERY_TIMEOUT + Duration::from_millis(25));
        assert!(!cancel.is_finished());
        cancel_release_tx.send(()).expect("release cancel");
        assert_eq!(
            cancel.join().expect("cancel waiter joins"),
            CancelLxmfMessageOutcome::Cancelled {
                local_record_id: U64String::from(1),
            }
        );

        let LxmfMessageListOutcome::Listed { messages } = list_lxmf_messages_with_supervisor(
            &supervisor,
            &storage,
            ListLxmfMessagesInput {
                peer: Some(destination),
                before: None,
                limit: 25,
            },
        ) else {
            panic!("the cancelled mailbox row was not listed");
        };
        assert!(matches!(
            messages[0].delivery_state,
            crate::contract::LxmfDeliveryState::Cancelled { .. }
        ));
        supervisor
            .lock_state()
            .application_owner
            .take()
            .expect("application owner")
            .close()
            .expect("application owner closes");
    }

    #[test]
    fn terminal_failed_worker_is_reaped_before_offline_mailbox_access() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");
        let paths = prepare_storage(&storage).expect("private storage");
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(true)),
            state: Mutex::new(SupervisorState::default()),
        };
        let destination = [0x42; 16];
        seed_failed_outbound_message(&supervisor, &paths, destination);

        supervisor
            .snapshots
            .set_runtime(DevelopmentNodeRuntime::Running);
        supervisor.snapshots.set_local_host(running_host_state());
        settle_worker_result(
            &supervisor.operation_admitted,
            &supervisor.snapshots,
            &Err((
                DevelopmentNodeStopStage::Node,
                "node stopped unexpectedly".to_owned(),
            )),
        );
        let failure_snapshot = supervisor.snapshots.read();
        let list_input = ListLxmfMessagesInput {
            peer: Some(destination),
            before: None,
            limit: 25,
        };

        install_finished_failed_worker(&supervisor, &paths);
        let LxmfMessageListOutcome::Listed { messages } =
            list_lxmf_messages_with_supervisor(&supervisor, &storage, list_input.clone())
        else {
            panic!("the failed generation stranded its durable row");
        };
        assert!(matches!(
            messages[0].delivery_state,
            crate::contract::LxmfDeliveryState::Failed { .. }
        ));
        assert!(supervisor.lock_state().worker.is_none());
        assert_eq!(supervisor.snapshots.read(), failure_snapshot);

        install_finished_failed_worker(&supervisor, &paths);
        assert_eq!(
            retry_lxmf_message_with_supervisor(
                &supervisor,
                &storage,
                RetryLxmfMessageInput {
                    local_record_id: U64String::from(1),
                },
            ),
            RetryLxmfMessageOutcome::Accepted {
                local_record_id: U64String::from(1),
            }
        );
        assert!(supervisor.lock_state().worker.is_none());
        assert_eq!(supervisor.snapshots.read(), failure_snapshot);

        install_finished_failed_worker(&supervisor, &paths);
        assert_eq!(
            cancel_lxmf_message_with_supervisor(
                &supervisor,
                &storage,
                CancelLxmfMessageInput {
                    local_record_id: U64String::from(1),
                },
            ),
            CancelLxmfMessageOutcome::Cancelled {
                local_record_id: U64String::from(1),
            }
        );
        assert!(supervisor.lock_state().worker.is_none());
        assert_eq!(supervisor.snapshots.read(), failure_snapshot);

        let LxmfMessageListOutcome::Listed { messages } =
            list_lxmf_messages_with_supervisor(&supervisor, &storage, list_input)
        else {
            panic!("the cancelled mailbox row was not listed");
        };
        assert!(matches!(
            messages[0].delivery_state,
            crate::contract::LxmfDeliveryState::Cancelled { .. }
        ));
        supervisor
            .lock_state()
            .application_owner
            .take()
            .expect("offline operations retained one application owner")
            .close()
            .expect("application owner closes");
    }

    #[test]
    fn terminal_failed_worker_does_not_block_a_direct_restart() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");
        let paths = prepare_storage(&storage).expect("private storage");
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(true)),
            state: Mutex::new(SupervisorState::default()),
        };
        {
            let mut state = supervisor.lock_state();
            identity_vault(&mut state, &paths)
                .expect("identity vault")
                .store(
                    &primary_label().expect("primary label"),
                    &[0x42; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
                )
                .expect("primary identity");
        }
        supervisor
            .snapshots
            .set_runtime(DevelopmentNodeRuntime::Running);
        supervisor.snapshots.set_local_host(running_host_state());
        settle_worker_result(
            &supervisor.operation_admitted,
            &supervisor.snapshots,
            &Err((
                DevelopmentNodeStopStage::Node,
                "node stopped unexpectedly".to_owned(),
            )),
        );
        install_finished_failed_worker(&supervisor, &paths);

        assert!(matches!(
            start_configured_with_supervisor(
                &supervisor,
                &storage,
                DevelopmentNodeStartInput {
                    development_tcp_target: None,
                },
                AppleBluetoothPreparation::WithoutRestoration,
            ),
            DevelopmentNodeStartOutcome::Started { .. }
        ));
        assert_eq!(
            supervisor.snapshots.read().runtime,
            DevelopmentNodeRuntime::Running
        );
        let mut state = supervisor.lock_state();
        assert_eq!(
            stop_locked(&supervisor, &mut state),
            DevelopmentNodeStopOutcome::Stopped
        );
        state
            .application_owner
            .take()
            .expect("restart retained the application owner")
            .close()
            .expect("application owner closes");
    }

    #[test]
    fn reset_preserves_the_owner_and_files_after_a_reported_stop_failure() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");
        let paths = prepare_storage(&storage).expect("private storage");
        let owner = DevelopmentStoreOwner::open(&paths.root, &paths.application)
            .expect("application owner");
        let (commands, _commands_rx) = mpsc::channel(1);
        let (shutdown_sender, _shutdown_rx) = watch::channel(false);
        let (done_tx, done) = std_mpsc::sync_channel(1);
        done_tx
            .send(Err((
                DevelopmentNodeStopStage::Worker,
                "reported worker failure".to_owned(),
            )))
            .expect("seed worker result");
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(test_supervisor_state(
                Some(test_worker(
                    commands,
                    ShutdownSignal {
                        sender: shutdown_sender,
                        requested: Arc::new(AtomicBool::new(false)),
                    },
                    done,
                    Some(std::thread::spawn(|| {})),
                    paths.root.clone(),
                )),
                Some(owner),
                None,
            )),
        };
        supervisor
            .snapshots
            .set_runtime(DevelopmentNodeRuntime::Running);

        assert!(matches!(
            reset_with_supervisor(&supervisor, &storage),
            DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Worker,
                ..
            }
        ));
        assert!(paths.application.exists());
        assert!(supervisor.lock_state().application_owner.is_some());
        assert!(supervisor
            .lock_state()
            .worker
            .as_ref()
            .is_some_and(|worker| worker.incomplete_stop.is_some() && worker.join.is_none()));
        assert_eq!(
            supervisor.snapshots.read().runtime,
            DevelopmentNodeRuntime::Stopping
        );

        assert_eq!(
            reset_with_supervisor(&supervisor, &storage),
            DevelopmentNodeStopOutcome::Stopped
        );
        assert!(!storage.exists());
        assert!(supervisor.lock_state().application_owner.is_none());
    }

    #[tokio::test]
    async fn durable_lxmf_send_accepts_and_queries_before_pending_proof() {
        let local_identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
            &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
        )
        .expect("the fixed LXMF destination name is valid");
        let root = tempfile::tempdir().expect("temporary application root");
        let database_path = root.path().join("application.redb");
        let owner = DevelopmentStoreOwner::open(root.path(), &database_path)
            .expect("the application owner opens");
        let network = Arc::new(PendingProofNetwork::default());
        let (pending, _) = prns_lxmf::mailbox::DurableDirectLxmfService::prepare(local_identity);
        let service = pending
            .start(
                Arc::clone(&network) as Arc<dyn prns_lxmf::direct::DirectNetwork>,
                owner.mailbox_submitter(),
                Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
            )
            .await
            .expect("the test owns a Tokio runtime");
        let peer_destination = learn_pending_peer(&service).await;

        let (send_response, send_result) = std_mpsc::sync_channel(1);
        let mut send_tasks = JoinSet::new();
        dispatch_lxmf_send(
            &service,
            &mut send_tasks,
            SendDirectTextInput {
                destination: peer_destination,
                title: "Proof gate".to_owned(),
                content: "Still sending".to_owned(),
            },
            send_response,
            1_700_000_000_000,
        );
        tokio::time::timeout(Duration::from_secs(1), network.send_entered.notified())
            .await
            .expect("the direct attempt reaches its pending proof gate");
        let accepted = send_result
            .recv_timeout(Duration::from_secs(1))
            .expect("queue acceptance does not await proof");
        let SendDirectTextOutcome::Accepted { local_record_id } = accepted else {
            panic!("the durable send was not accepted: {accepted:?}");
        };
        assert!(send_tasks
            .join_next()
            .await
            .expect("the tracked response wrapper exists")
            .is_ok());

        let (list_response, list_result) = std_mpsc::sync_channel(1);
        dispatch_lxmf_message_list(
            &service,
            prns_lxmf::mailbox::MailboxListRequest {
                peer: Some(peer_destination),
                direction: None,
                before: None,
                limit: 25,
            },
            list_response,
        )
        .await;
        let LxmfMessageListOutcome::Listed { messages } = list_result
            .try_recv()
            .expect("the query completes while proof remains pending")
        else {
            panic!("the pending message query did not return a list");
        };
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].delivery_state,
            crate::contract::LxmfDeliveryState::Sending {
                failed_attempts: U64String::from(0)
            }
        );
        assert_eq!(messages[0].local_record_id, local_record_id);

        network.release_send.notify_one();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = service
                    .snapshot(prns_lxmf::mailbox::MailboxListRequest {
                        peer: None,
                        direction: None,
                        before: None,
                        limit: 25,
                    })
                    .await
                    .expect("the mailbox query succeeds");
                if matches!(
                    snapshot.messages[0].delivery_state,
                    prns_lxmf::mailbox::DurableLxmfDeliveryState::Delivered { .. }
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("proof settlement is persisted");
        service.stop().await.expect("the service stops promptly");
        owner.close().expect("the owner closes after the service");
    }

    #[tokio::test]
    async fn admitted_insert_survives_stop_and_response_drain_boundaries() {
        let local_identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
            &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
        )
        .expect("the fixed LXMF destination name is valid");
        let root = tempfile::tempdir().expect("temporary application root");
        let database_path = root.path().join("application.redb");
        let owner = DevelopmentStoreOwner::open(root.path(), &database_path)
            .expect("the application owner opens");
        let blocking_submitter = Arc::new(BlockingInsertSubmitter::new(owner.mailbox_submitter()));
        let network = Arc::new(PendingProofNetwork::default());
        let (pending, _) = prns_lxmf::mailbox::DurableDirectLxmfService::prepare(local_identity);
        let service = pending
            .start(
                Arc::clone(&network) as Arc<dyn prns_lxmf::direct::DirectNetwork>,
                Arc::clone(&blocking_submitter) as Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
                Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
            )
            .await
            .expect("the test owns a Tokio runtime");
        let peer_destination = learn_pending_peer(&service).await;
        let (send_response, send_result) = std_mpsc::sync_channel(1);
        let mut send_tasks = JoinSet::new();
        dispatch_lxmf_send(
            &service,
            &mut send_tasks,
            SendDirectTextInput {
                destination: peer_destination,
                title: "Retain response".to_owned(),
                content: "Commit exactly once".to_owned(),
            },
            send_response,
            1_700_000_000_500,
        );
        tokio::time::timeout(
            Duration::from_secs(1),
            blocking_submitter.insert_entered.notified(),
        )
        .await
        .expect("the insert reaches its durable owner boundary");

        let mut shutdown = Box::pin(stop_lxmf_service_and_drain(&service, &mut send_tasks));
        let old_stop_and_drain_boundaries =
            prns_lxmf::direct::STOP_JOIN_TIMEOUT + LXMF_QUERY_TIMEOUT + Duration::from_millis(25);
        assert!(
            tokio::time::timeout(old_stop_and_drain_boundaries, &mut shutdown)
                .await
                .is_err()
        );
        assert!(matches!(
            send_result.try_recv(),
            Err(std_mpsc::TryRecvError::Empty)
        ));

        blocking_submitter.release_insert.notify_one();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), &mut shutdown)
                .await
                .expect("retained response wrapper and service complete"),
            Ok(())
        );
        drop(shutdown);
        assert_eq!(
            send_result
                .recv_timeout(Duration::from_secs(1))
                .expect("the committed insert reports its definitive outcome"),
            SendDirectTextOutcome::Accepted {
                local_record_id: U64String::from(1),
            }
        );
        assert_eq!(network.send_count.load(Ordering::Acquire), 0);

        let listed = owner
            .admit_mailbox(prns_lxmf::mailbox::MailboxRequest::List(
                prns_lxmf::mailbox::MailboxListRequest {
                    peer: None,
                    direction: None,
                    before: None,
                    limit: 25,
                },
            ))
            .expect("list admission")
            .recv()
            .expect("list response")
            .expect("list succeeds");
        let prns_lxmf::mailbox::MailboxReply::Listed { messages, .. } = listed else {
            panic!("unexpected mailbox reply");
        };
        assert_eq!(messages.len(), 1);
        assert!(matches!(
            messages[0].delivery_state,
            prns_lxmf::mailbox::DurableLxmfDeliveryState::Queued { .. }
        ));
        owner.close().expect("the owner closes after the service");
    }

    #[tokio::test]
    async fn async_mailbox_corruption_preserves_the_typed_reset_affordance() {
        let local_identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
            &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
        )
        .expect("the fixed LXMF destination name is valid");
        let root = tempfile::tempdir().expect("temporary application root");
        let database_path = root.path().join("application.redb");
        let owner = DevelopmentStoreOwner::open(root.path(), &database_path)
            .expect("the application owner opens");
        let submitter = Arc::new(ResetCompletionSubmitter {
            inner: owner.mailbox_submitter(),
        });
        let network = Arc::new(PendingProofNetwork::default());
        let (pending, _) = prns_lxmf::mailbox::DurableDirectLxmfService::prepare(local_identity);
        let service = pending
            .start(
                Arc::clone(&network) as Arc<dyn prns_lxmf::direct::DirectNetwork>,
                submitter as Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
                Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
            )
            .await
            .expect("the test owns a Tokio runtime");
        let peer_destination = learn_pending_peer(&service).await;

        assert_eq!(
            service
                .send_direct_text(
                    peer_destination,
                    1_700_000_000_700,
                    b"Corruption",
                    b"Preserve reset affordance",
                )
                .await,
            prns_lxmf::mailbox::DurableSendDirectTextOutcome::Accepted { local_record_id: 1 }
        );
        tokio::time::timeout(Duration::from_secs(1), network.send_entered.notified())
            .await
            .expect("the attempt reaches the proof boundary");
        network.release_send.notify_one();
        let failure = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = service
                    .snapshot(prns_lxmf::mailbox::MailboxListRequest {
                        peer: None,
                        direction: None,
                        before: None,
                        limit: 25,
                    })
                    .await
                    .expect("the read remains available");
                if let prns_lxmf::mailbox::MailboxProjectionHealth::ResetRequired(reason) =
                    snapshot.mailbox_health
                {
                    break prns_lxmf::mailbox::MailboxFailure::ResetRequired(reason);
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the asynchronous corruption becomes visible");
        assert_eq!(
            service
                .send_direct_text(
                    peer_destination,
                    1_700_000_000_701,
                    b"Later write",
                    b"Cannot clear corruption",
                )
                .await,
            prns_lxmf::mailbox::DurableSendDirectTextOutcome::Accepted { local_record_id: 2 }
        );
        assert!(matches!(
            service
                .snapshot(prns_lxmf::mailbox::MailboxListRequest {
                    peer: None,
                    direction: None,
                    before: None,
                    limit: 25,
                })
                .await
                .expect("the read remains available")
                .mailbox_health,
            prns_lxmf::mailbox::MailboxProjectionHealth::ResetRequired(ref reason)
                if reason == "mailbox record became unreadable"
        ));

        let snapshots = SnapshotStore::new();
        snapshots.set_runtime(DevelopmentNodeRuntime::Running);
        snapshots.set_local_host(running_host_state());
        assert_eq!(
            refresh_lxmf_health(&service, &snapshots).await,
            Err(failure.clone())
        );
        assert_eq!(
            publish_lxmf_storage_failure(&snapshots, &failure),
            "mailbox record became unreadable"
        );
        let failed = snapshots.read();
        assert_eq!(failed.runtime, DevelopmentNodeRuntime::Failed);
        assert_eq!(
            failed.local_host,
            LocalHostState::DevelopmentResetRequired {
                reason: "mailbox record became unreadable".to_owned(),
            }
        );
        assert_eq!(
            failed.failure,
            Some(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Storage,
                detail: "mailbox record became unreadable".to_owned(),
            })
        );
        service.stop().await.expect("the service stops promptly");
        owner.close().expect("the owner closes after the service");
    }

    #[tokio::test]
    async fn transient_mailbox_refresh_failures_degrade_and_retry_without_stopping() {
        let local_identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
            &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
        )
        .expect("the fixed LXMF destination name is valid");
        let root = tempfile::tempdir().expect("temporary application root");
        let database_path = root.path().join("application.redb");
        let owner = DevelopmentStoreOwner::open(root.path(), &database_path)
            .expect("the application owner opens");
        let submitter = Arc::new(TransientListFailureSubmitter {
            inner: owner.mailbox_submitter(),
            remaining_failures: std::sync::atomic::AtomicUsize::new(2),
            list_submissions: std::sync::atomic::AtomicUsize::new(0),
        });
        let network = Arc::new(PendingProofNetwork::default());
        let (pending, _) = prns_lxmf::mailbox::DurableDirectLxmfService::prepare(local_identity);
        let service = pending
            .start(
                network,
                submitter as Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
                Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
            )
            .await
            .expect("the test owns a Tokio runtime");
        let snapshots = SnapshotStore::new();
        snapshots.set_runtime(DevelopmentNodeRuntime::Running);
        snapshots.set_local_host(running_host_state());
        snapshots.refresh_lxmf(crate::contract::LxmfHealth {
            state: crate::contract::LxmfHealthState::Ready,
            inbound_overflow_count: U64String::from(7),
        });

        assert_eq!(
            refresh_running_lxmf_health(&service, &snapshots).await,
            Ok(true)
        );
        let busy = snapshots.read();
        assert_eq!(busy.runtime, DevelopmentNodeRuntime::Running);
        assert_eq!(busy.lxmf.state, crate::contract::LxmfHealthState::Degraded);
        assert_eq!(busy.lxmf.inbound_overflow_count, U64String::from(7));
        assert_eq!(busy.failure, None);

        assert_eq!(
            refresh_running_lxmf_health(&service, &snapshots).await,
            Ok(true)
        );
        let unavailable = snapshots.read();
        assert_eq!(unavailable.runtime, DevelopmentNodeRuntime::Running);
        assert_eq!(unavailable.revision, busy.revision);
        assert_eq!(unavailable.failure, None);

        assert_eq!(
            refresh_running_lxmf_health(&service, &snapshots).await,
            Ok(false)
        );
        let recovered = snapshots.read();
        assert_eq!(recovered.runtime, DevelopmentNodeRuntime::Running);
        assert_eq!(
            recovered.lxmf.state,
            crate::contract::LxmfHealthState::Ready
        );
        assert_eq!(recovered.lxmf.inbound_overflow_count, U64String::from(0));
        assert_eq!(recovered.failure, None);

        service.stop().await.expect("the service stops promptly");
        owner.close().expect("the owner closes after the service");
    }

    #[tokio::test(start_paused = true)]
    async fn delayed_mailbox_health_retry_coalesces_feedback_and_services_actor_input() {
        let local_identity = prns_lxmf::direct::LocalLxmfIdentity::from_secret_bytes(
            &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
        )
        .expect("the fixed LXMF destination name is valid");
        let root = tempfile::tempdir().expect("temporary application root");
        let database_path = root.path().join("application.redb");
        let owner = DevelopmentStoreOwner::open(root.path(), &database_path)
            .expect("the application owner opens");
        let submitter = Arc::new(TransientListFailureSubmitter {
            inner: owner.mailbox_submitter(),
            remaining_failures: std::sync::atomic::AtomicUsize::new(2),
            list_submissions: std::sync::atomic::AtomicUsize::new(0),
        });
        let network = Arc::new(PendingProofNetwork::default());
        let (pending, _) = prns_lxmf::mailbox::DurableDirectLxmfService::prepare(local_identity);
        let service = pending
            .start(
                network,
                Arc::clone(&submitter) as Arc<dyn prns_lxmf::mailbox::MailboxSubmitter>,
                Arc::new(prns_lxmf::mailbox::SystemMailboxClock),
            )
            .await
            .expect("the test owns a Tokio runtime");
        let snapshots = SnapshotStore::new();
        snapshots.set_runtime(DevelopmentNodeRuntime::Running);
        snapshots.set_local_host(running_host_state());
        snapshots.refresh_lxmf(crate::contract::LxmfHealth {
            state: crate::contract::LxmfHealthState::Ready,
            inbound_overflow_count: U64String::from(7),
        });
        let mut refresh = service.subscribe();
        let mut retry_timer = tokio::time::interval(LXMF_HEALTH_RETRY_DELAY);
        retry_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut retry_pending = false;

        refresh_running_lxmf_health_after_change(
            &service,
            &snapshots,
            &mut retry_pending,
            &mut retry_timer,
        )
        .await
        .expect("a transient failure does not stop the actor");
        assert!(retry_pending);
        assert_eq!(submitter.list_submissions.load(Ordering::Acquire), 1);

        let (actor_input, mut actor_commands) = tokio::sync::mpsc::channel(1);
        actor_input
            .send(())
            .await
            .expect("the command lane is open");
        let mut command_processed = false;
        for _ in 0..2 {
            tokio::select! {
                biased;
                changed = refresh.changed() => {
                    changed.expect("the service remains running");
                    refresh_running_lxmf_health_after_change(
                        &service,
                        &snapshots,
                        &mut retry_pending,
                        &mut retry_timer,
                    ).await.expect("feedback is coalesced");
                }
                command = actor_commands.recv() => {
                    command_processed = command.is_some();
                    break;
                }
            }
        }
        assert!(command_processed);
        assert_eq!(submitter.list_submissions.load(Ordering::Acquire), 1);

        tokio::time::advance(LXMF_HEALTH_RETRY_DELAY).await;
        retry_timer.tick().await;
        retry_pending = refresh_running_lxmf_health(&service, &snapshots)
            .await
            .expect("the unavailable retry remains nonfatal");
        assert!(retry_pending);
        assert_eq!(submitter.list_submissions.load(Ordering::Acquire), 2);
        refresh
            .changed()
            .await
            .expect("the service remains running");
        refresh_running_lxmf_health_after_change(
            &service,
            &snapshots,
            &mut retry_pending,
            &mut retry_timer,
        )
        .await
        .expect("retry feedback is coalesced");
        assert_eq!(submitter.list_submissions.load(Ordering::Acquire), 2);

        tokio::time::advance(LXMF_HEALTH_RETRY_DELAY).await;
        retry_timer.tick().await;
        retry_pending = refresh_running_lxmf_health(&service, &snapshots)
            .await
            .expect("the delayed retry recovers");
        assert!(!retry_pending);
        assert_eq!(submitter.list_submissions.load(Ordering::Acquire), 3);
        let recovered = snapshots.read();
        assert_eq!(recovered.runtime, DevelopmentNodeRuntime::Running);
        assert_eq!(
            recovered.lxmf.state,
            crate::contract::LxmfHealthState::Ready
        );
        assert_eq!(recovered.failure, None);

        service.stop().await.expect("the service stops promptly");
        owner.close().expect("the owner closes after the service");
    }

    #[test]
    fn fatal_pairing_commands_remove_only_the_selected_discovery_candidate() {
        let endpoint = personal_rns::remote_control::RemoteControlPairingIdentity::new(
            personal_rns::identity::IdentityHash::new([0x42; 16]),
        )
        .endpoint();
        let other_endpoint = personal_rns::remote_control::RemoteControlPairingIdentity::new(
            personal_rns::identity::IdentityHash::new([0x43; 16]),
        )
        .endpoint();
        let mut controls = PairingControls {
            candidates: vec![
                crate::pairing::PairingCandidateControl {
                    candidate_id: "candidate".to_owned(),
                    endpoint,
                    display_name: Some("Candidate".to_owned()),
                    observed_at: personal_rns::units::InstantMillis(1),
                    expires_at: personal_rns::units::InstantMillis(10),
                },
                crate::pairing::PairingCandidateControl {
                    candidate_id: "other".to_owned(),
                    endpoint: other_endpoint,
                    display_name: Some("Other".to_owned()),
                    observed_at: personal_rns::units::InstantMillis(2),
                    expires_at: personal_rns::units::InstantMillis(10),
                },
            ],
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
        assert_eq!(controls.candidates.len(), 1);
        assert_eq!(controls.candidates[0].candidate_id, "other");
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
    fn worker_failure_during_explicit_stop_retains_the_stopping_transition() {
        let snapshots = SnapshotStore::new();
        snapshots.set_runtime(DevelopmentNodeRuntime::Running);
        let local_host = running_host_state();
        snapshots.set_local_host(local_host.clone());
        snapshots.begin_stop(U64String::from(42));
        let admitted = AtomicBool::new(true);

        settle_worker_result(
            &admitted,
            &snapshots,
            &Err((
                DevelopmentNodeStopStage::Worker,
                "shutdown did not settle".to_owned(),
            )),
        );

        assert!(!admitted.load(Ordering::Acquire));
        let snapshot = snapshots.read();
        assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Stopping);
        assert_eq!(snapshot.local_host, local_host);
        assert_eq!(
            snapshot.failure,
            Some(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Runtime,
                detail: "shutdown did not settle".to_owned(),
            })
        );
        assert_eq!(
            snapshot.active_operation,
            Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::Shutdown,
                started_at_millis: U64String::from(42),
            })
        );
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
        let mut state = test_supervisor_state(
            Some(test_worker(
                commands,
                shutdown,
                done,
                Some(join),
                PathBuf::from("/tmp/prns/development"),
            )),
            None,
            None,
        );

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
        supervisor.lock_state().worker = Some(test_worker(
            commands,
            shutdown,
            done,
            Some(join),
            PathBuf::from("/tmp/prns/development"),
        ));

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
        supervisor.lock_state().worker = Some(test_worker(
            old_commands,
            ShutdownSignal {
                sender: shutdown_sender,
                requested: Arc::new(AtomicBool::new(false)),
            },
            done,
            None,
            paths.root.clone(),
        ));

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
        supervisor.lock_state().worker = Some(test_worker(
            new_commands,
            ShutdownSignal {
                sender: new_shutdown_sender,
                requested: Arc::new(AtomicBool::new(false)),
            },
            new_done,
            None,
            paths.root,
        ));
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
        supervisor.lock_state().worker = Some(test_worker(
            commands,
            ShutdownSignal {
                sender: shutdown_sender,
                requested: Arc::new(AtomicBool::new(false)),
            },
            done,
            None,
            paths.root.clone(),
        ));

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
        let mut state = test_supervisor_state(
            None,
            Some(
                DevelopmentStoreOwner::open(&first_paths.root, &first_paths.application)
                    .expect("first application owner"),
            ),
            None,
        );

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
        let mut state = test_supervisor_state(
            Some(test_worker(
                commands,
                ShutdownSignal {
                    sender: shutdown_sender,
                    requested: Arc::new(AtomicBool::new(false)),
                },
                done,
                None,
                first_paths.root.clone(),
            )),
            Some(
                DevelopmentStoreOwner::open(&first_paths.root, &first_paths.application)
                    .expect("first application owner"),
            ),
            Some(IdentityOwner {
                root: first_paths.root.clone(),
                vault: FileVault::new(&first_paths.identities),
            }),
        );

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
        let mut state = test_supervisor_state(
            Some(test_worker(
                commands,
                shutdown,
                done,
                Some(join),
                PathBuf::from("/tmp/prns/development"),
            )),
            None,
            None,
        );

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
    fn incomplete_stop_remains_stopping_and_blocks_start_until_reset() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");
        let paths = prepare_storage(&storage).expect("private storage");
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(true)),
            state: Mutex::new(SupervisorState::default()),
        };
        supervisor
            .snapshots
            .set_runtime(DevelopmentNodeRuntime::Running);
        let local_host = running_host_state();
        supervisor.snapshots.set_local_host(local_host.clone());

        let (commands, _command_rx) = mpsc::channel(1);
        let (shutdown_sender, _shutdown_rx) = watch::channel(false);
        let shutdown = ShutdownSignal {
            sender: shutdown_sender,
            requested: Arc::new(AtomicBool::new(false)),
        };
        let (done_tx, done) = std_mpsc::sync_channel(1);
        done_tx
            .send(Err((
                DevelopmentNodeStopStage::Worker,
                "shutdown did not settle".to_owned(),
            )))
            .expect("terminal worker result");
        let join = std::thread::spawn(|| {});
        supervisor.lock_state().worker = Some(test_worker(
            commands,
            shutdown,
            done,
            Some(join),
            paths.root,
        ));

        {
            let mut state = supervisor.lock_state();
            assert_eq!(
                stop_locked(&supervisor, &mut state),
                DevelopmentNodeStopOutcome::Failed {
                    stage: DevelopmentNodeStopStage::Worker,
                    detail: "shutdown did not settle".to_owned(),
                }
            );
            assert!(state
                .worker
                .as_ref()
                .is_some_and(|worker| worker.incomplete_stop.is_some() && worker.join.is_none()));
        }
        assert!(!supervisor.operation_admitted.load(Ordering::Acquire));
        let incomplete = snapshot_with_supervisor(&supervisor);
        assert_eq!(incomplete.runtime, DevelopmentNodeRuntime::Stopping);
        assert_eq!(incomplete.local_host, local_host);
        assert_eq!(
            incomplete.failure,
            Some(DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Runtime,
                detail: "shutdown did not settle".to_owned(),
            })
        );
        assert!(matches!(
            incomplete.active_operation,
            Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::Shutdown,
                ..
            })
        ));

        assert!(matches!(
            start_configured_with_supervisor(
                &supervisor,
                &storage,
                DevelopmentNodeStartInput {
                    development_tcp_target: None,
                },
                AppleBluetoothPreparation::WithoutRestoration,
            ),
            DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::Runtime,
                ..
            }
        ));
        assert_eq!(
            supervisor.snapshots.read().runtime,
            DevelopmentNodeRuntime::Stopping
        );

        let mut state = supervisor.lock_state();
        assert_eq!(
            stop_locked(&supervisor, &mut state),
            DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Worker,
                detail: "shutdown did not settle".to_owned(),
            }
        );
        assert!(state.worker.is_some());
        assert_eq!(
            supervisor.snapshots.read().runtime,
            DevelopmentNodeRuntime::Stopping
        );
        drop(state);

        assert_eq!(
            reset_with_supervisor(&supervisor, &storage),
            DevelopmentNodeStopOutcome::Stopped
        );
        assert!(supervisor.lock_state().worker.is_none());
        assert_eq!(
            supervisor.snapshots.read().runtime,
            DevelopmentNodeRuntime::Stopped
        );
    }

    #[test]
    fn timed_out_stop_can_settle_successfully_when_retried() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage = temporary.path().join("prns").join("development");
        let paths = prepare_storage(&storage).expect("private storage");
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(SupervisorState::default()),
        };
        let detail = "The native worker did not finish bounded shutdown.".to_owned();
        supervisor.snapshots.incomplete_stop(
            DevelopmentNodeFailure {
                stage: DevelopmentNodeFailureStage::Runtime,
                detail: detail.clone(),
            },
            U64String::from(42),
        );

        let (commands, _command_rx) = mpsc::channel(1);
        let (shutdown_sender, _shutdown_rx) = watch::channel(false);
        let (done_tx, done) = std_mpsc::sync_channel(1);
        done_tx.send(Ok(())).expect("terminal worker result");
        let mut worker = test_worker(
            commands,
            ShutdownSignal {
                sender: shutdown_sender,
                requested: Arc::new(AtomicBool::new(true)),
            },
            done,
            Some(std::thread::spawn(|| {})),
            paths.root,
        );
        worker.incomplete_stop = Some((DevelopmentNodeStopStage::Worker, detail));
        supervisor.lock_state().worker = Some(worker);

        assert!(matches!(
            start_configured_with_supervisor(
                &supervisor,
                &storage,
                DevelopmentNodeStartInput {
                    development_tcp_target: None,
                },
                AppleBluetoothPreparation::WithoutRestoration,
            ),
            DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::Runtime,
                ..
            }
        ));

        let mut state = supervisor.lock_state();
        assert_eq!(
            stop_locked(&supervisor, &mut state),
            DevelopmentNodeStopOutcome::Stopped
        );
        assert!(state.worker.is_none());
        assert_eq!(
            supervisor.snapshots.read().runtime,
            DevelopmentNodeRuntime::Stopped
        );
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
        let mut state = test_supervisor_state(
            Some(test_worker(
                commands,
                shutdown,
                done,
                Some(join),
                PathBuf::from("/tmp/prns/development"),
            )),
            None,
            None,
        );

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
    fn stop_reaps_a_disconnected_terminal_worker_and_preserves_its_failure() {
        let supervisor = Supervisor {
            snapshots: Arc::new(SnapshotStore::new()),
            operation_admitted: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(SupervisorState::default()),
        };
        let prior_failure = DevelopmentNodeFailure {
            stage: DevelopmentNodeFailureStage::Node,
            detail: "node stopped unexpectedly".to_owned(),
        };
        supervisor.snapshots.terminal_fail(prior_failure.clone());

        let (commands, _command_rx) = mpsc::channel(1);
        let (shutdown_sender, _shutdown_rx) = watch::channel(false);
        let (_done_tx, done) = std_mpsc::sync_channel::<WorkerResult>(1);
        drop(_done_tx);
        let join = std::thread::spawn(|| {});
        let mut state = test_supervisor_state(
            Some(test_worker(
                commands,
                ShutdownSignal {
                    sender: shutdown_sender,
                    requested: Arc::new(AtomicBool::new(false)),
                },
                done,
                Some(join),
                PathBuf::from("/tmp/prns/development"),
            )),
            None,
            None,
        );

        assert!(matches!(
            stop_locked(&supervisor, &mut state),
            DevelopmentNodeStopOutcome::Failed {
                stage: DevelopmentNodeStopStage::Worker,
                ..
            }
        ));
        assert!(state.worker.is_none());
        let snapshot = supervisor.snapshots.read();
        assert_eq!(snapshot.runtime, DevelopmentNodeRuntime::Failed);
        assert_eq!(snapshot.failure, Some(prior_failure));
        assert!(snapshot.active_operation.is_none());
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
        assert_eq!(running.lxmf.state, crate::contract::LxmfHealthState::Ready);
        assert_eq!(
            list_lxmf_peers(),
            LxmfPeerListOutcome::Listed { peers: vec![] }
        );
        assert_eq!(
            list_lxmf_messages(
                &storage,
                ListLxmfMessagesInput {
                    peer: None,
                    before: None,
                    limit: 25,
                },
            ),
            LxmfMessageListOutcome::Listed { messages: vec![] }
        );
        assert_eq!(
            measure_lxmf_text(MeasureLxmfTextInput {
                title: String::new(),
                content: "x".repeat(319),
            }),
            MeasureLxmfTextOutcome::Measured {
                wire_bytes: 431,
                remaining_bytes: 0,
            }
        );
        assert_eq!(
            send_direct_text(SendDirectTextInput {
                destination: [0x99; 16],
                title: "Unknown peer".to_owned(),
                content: "No message should be sent".to_owned(),
            }),
            SendDirectTextOutcome::PeerIdentityUnavailable
        );
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
        #[cfg(any(feature = "apple", feature = "host-test"))]
        {
            assert!(matches!(
                start_configured(
                    &storage,
                    DevelopmentNodeStartInput {
                        development_tcp_target: Some("127.0.0.1:9".to_owned()),
                    },
                ),
                DevelopmentNodeStartOutcome::Started { .. }
            ));
            let tcp_snapshot = snapshot();
            let LocalHostState::Running { host } = tcp_snapshot.local_host else {
                panic!("the TCP generation did not publish its Host snapshot");
            };
            assert!(host.backend.supports(Capability::TcpClient));
            assert!(host.backend.supports_interface(InterfaceKind::TcpClient));
            assert_eq!(
                std::fs::read(&bluetooth_path).unwrap_or_default(),
                bluetooth_record
            );
            assert_eq!(stop(), DevelopmentNodeStopOutcome::Stopped);
        }
        #[cfg(not(any(feature = "apple", feature = "host-test")))]
        {
            assert!(matches!(
                start_configured(
                    &storage,
                    DevelopmentNodeStartInput {
                        development_tcp_target: Some("127.0.0.1:9".to_owned()),
                    },
                ),
                DevelopmentNodeStartOutcome::Failed {
                    stage: DevelopmentNodeFailureStage::Contract,
                    ..
                }
            ));
            assert!(matches!(
                start(&storage),
                DevelopmentNodeStartOutcome::Started { .. }
            ));
            assert_eq!(
                std::fs::read(&bluetooth_path).unwrap_or_default(),
                bluetooth_record
            );
            assert_eq!(stop(), DevelopmentNodeStopOutcome::Stopped);
        }
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
