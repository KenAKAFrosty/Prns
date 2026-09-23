pub(crate) mod admission;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

#[cfg(all(feature = "apple", target_os = "ios"))]
use personal_rns::bluetooth_auto::{AutoBle, CoreBluetoothRestorationIdentifiers};
#[cfg(any(test, all(feature = "apple", target_os = "ios")))]
use personal_rns::interfaces::bluetooth_auto::BleIdentity;
use personal_rns::node_introspection::DestinationIdentityQuery;
use personal_rns::prelude::{
    InitiateRemoteControlControllerPairing, PrnsNodeHandle,
    RemoteControlControllerPairingInitiationControl, RemoteControlPairingControl,
};
use personal_rns::remote_control::{
    RemoteControlInitialControllerGrants, RemoteControlPairingInvitationCode,
    RemoteControlSelfAnnouncement, RemoteControlService,
};
use personal_rns::runtime::{
    LocalIdentityFileError, RemoteControlIdentityDirectory, RemoteControlPairingControlError,
};
use personal_rns::wire::DestinationHash;
use prns_core::identity::vault::{
    FileVault, FileVaultError, IdentityLabel, IdentitySecretKey, IdentityVault,
};
use prns_core::identity::PrivateIdentityMaterial;
#[cfg(any(test, feature = "apple", feature = "android", feature = "host-test"))]
use prns_host::InterfaceKind;
use prns_host_native::owner::{HostClient, OwnedSession};
#[cfg(any(feature = "apple", feature = "android", feature = "host-test"))]
use prns_host_native::NativePreparedAttachment;
use prns_host_native::{ApplicationEventDispatch, NativeEmbedding};
#[cfg(all(feature = "apple", target_os = "ios"))]
use prns_interfaces_tokio::bluetooth_auto::PreparedAutoBle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;

use crate::contract::*;
use crate::development_store::{
    DevelopmentStoreFailure, DevelopmentStoreOwner, MailboxStoreReply, StoreReply,
};
use crate::directory::{DirectoryRequest, DirectoryResponse};
use crate::node::{prepare_storage, reset_storage, NodeStoragePaths};
use crate::pairing::{
    apply_event, apply_overflow_failure, attempt_id_string, expire_candidates,
    publish_candidate_resolution, remove_selected_candidate, send_event, AppliedNodeEvent,
    OwnedNodeEvent, PairingCandidateResolution, PairingControls, EVENT_LANE_CAPACITY,
};
use crate::snapshot::SnapshotStore;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
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
    Restoration { central: String, peripheral: String },
}

impl AppleBluetoothPreparation {
    fn validate(&self) -> Result<(), String> {
        let Self::Restoration {
            central,
            peripheral,
        } = self
        else {
            return Ok(());
        };
        for (role, identifier) in [("central", central), ("peripheral", peripheral)] {
            if identifier.is_empty() {
                return Err(format!(
                    "{role} CoreBluetooth restoration identifier must not be empty"
                ));
            }
            if identifier.len() > crate::input::MAX_RESTORATION_IDENTIFIER_BYTES {
                return Err(format!(
                    "{role} CoreBluetooth restoration identifier exceeds 1024 bytes"
                ));
            }
        }
        if central == peripheral {
            return Err("CoreBluetooth restoration identifiers must be distinct".to_owned());
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
    #[cfg(all(feature = "android", target_os = "android"))]
    android: crate::android::BluetoothSession,
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
    runtime: Arc<OnceLock<tokio::runtime::Handle>>,
    host: Arc<OnceLock<HostClient>>,
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

/// Dropping a caller closes only its response; admitted writes remain actor-owned.
type Reply<T> = oneshot::Sender<T>;

enum Command {
    RemoteRead(management::ReadCommand),
    RemoteChange(RemoteChangeOperation, tokio::time::Instant),
    RemoteWifi(wifi::WifiCommand),
    AnnounceSelf(RemoteControlAnnounceOperation),
    Snapshot(oneshot::Sender<DevelopmentNodeSnapshot>),
    Initiate(
        InitiateRemoteControlPairingInput,
        Reply<RemoteControlPairingCommandOutcome>,
    ),
    Approve(
        RemoteControlPairingDecisionInput,
        Reply<RemoteControlPairingCommandOutcome>,
    ),
    Reject(
        RemoteControlPairingDecisionInput,
        Reply<RemoteControlPairingCommandOutcome>,
    ),
    Describe(DescribeCommand),
    ObservedIdentity([u8; 16], Reply<Result<Option<[u8; 16]>, String>>),
    ListLxmfPeers(Reply<LxmfPeerListOutcome>),
    ListLxmfMessages(
        prns_lxmf::mailbox::MailboxListRequest,
        Reply<LxmfMessageListOutcome>,
    ),
    RetryLxmfMessage(u64, Reply<RetryLxmfMessageOutcome>),
    CancelLxmfMessage(u64, u64, Reply<CancelLxmfMessageOutcome>),
    MeasureLxmfText(MeasureLxmfTextInput, Reply<MeasureLxmfTextOutcome>),
    AnnounceLxmf(Reply<AnnounceLxmfOutcome>),
    SendDirectText(SendDirectTextInput, Reply<SendDirectTextOutcome>),
}

struct DescribeCommand {
    input: DescribeRemoteControlTargetInput,
    response: Reply<RemoteControlDescribeOutcome>,
    deadline: tokio::time::Instant,
    caller: oneshot::Receiver<()>,
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
                        };
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
    if !crate::input::bounded_text(&[target]) {
        return Err("developmentTcpTarget exceeds the native input size limit".to_owned());
    }
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

pub(crate) fn prepare_apple_bluetooth_restoration(
    storage_root: &Path,
    central_identifier: String,
    peripheral_identifier: String,
) -> AppleBluetoothRestorationPreparationOutcome {
    crate::ios_restoration_probe::install();
    let preparation = AppleBluetoothPreparation::Restoration {
        central: central_identifier,
        peripheral: peripheral_identifier,
    };
    if let Err(detail) = preparation.validate() {
        return apple_bluetooth_preparation_failed(
            AppleBluetoothRestorationPreparationFailureStage::Contract,
            detail,
        );
    }

    prepare_apple_bluetooth_restoration_with_supervisor(supervisor(), storage_root, preparation)
}

#[cfg(any(test, all(feature = "apple", target_os = "ios")))]
fn apple_bluetooth_restoration_storage(
    state: &mut SupervisorState,
    storage_root: &Path,
) -> Result<NodeStoragePaths, AppleBluetoothRestorationPreparationOutcome> {
    // Process launch is also used to restore scene-based apps, where UIKit no
    // longer supplies Bluetooth launch options. Only an existing installation
    // identity may admit automatic restoration; onboarding remains explicit.
    match std::fs::metadata(storage_root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Storage,
                "The existing private application directory is not a directory.",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Identity,
                "Create or import a primary identity before restoring the local node.",
            ));
        }
        Err(error) => {
            return Err(apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Storage,
                format!("could not inspect the private application directory: {error}"),
            ));
        }
    }
    let paths = prepare_storage(storage_root).map_err(|detail| {
        apple_bluetooth_preparation_failed(
            AppleBluetoothRestorationPreparationFailureStage::Storage,
            detail,
        )
    })?;
    let detail = match inspect_identity_locked(state, storage_root) {
        PrimaryIdentityState::Present { .. } => return Ok(paths),
        PrimaryIdentityState::Missing => {
            "Create or import a primary identity before restoring the local node.".to_owned()
        }
        PrimaryIdentityState::Unavailable { detail } => detail,
        PrimaryIdentityState::DevelopmentResetRequired { reason } => reason,
    };
    Err(apple_bluetooth_preparation_failed(
        AppleBluetoothRestorationPreparationFailureStage::Identity,
        detail,
    ))
}

#[cfg(all(feature = "apple", target_os = "ios"))]
fn prepare_apple_bluetooth_restoration_with_supervisor(
    supervisor: &Supervisor,
    storage_root: &Path,
    preparation: AppleBluetoothPreparation,
) -> AppleBluetoothRestorationPreparationOutcome {
    let mut state = supervisor.lock_state();
    reap_completed_worker_locked(supervisor, &mut state);

    let paths = match apple_bluetooth_restoration_storage(&mut state, storage_root) {
        Ok(paths) => paths,
        Err(outcome) => return outcome,
    };
    let identity = match personal_rns::load_or_create_ble_identity(&paths.bluetooth_identity) {
        Ok(identity) => identity,
        Err(error) => {
            return apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Identity,
                format!("could not load the installation Bluetooth identity: {error}"),
            );
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

    let AppleBluetoothPreparation::Restoration {
        central,
        peripheral,
    } = preparation
    else {
        return apple_bluetooth_preparation_failed(
            AppleBluetoothRestorationPreparationFailureStage::Contract,
            "CoreBluetooth restoration preparation requires restoration identifiers.",
        );
    };
    let identifiers = match CoreBluetoothRestorationIdentifiers::new(central, peripheral) {
        Ok(identifier) => identifier,
        Err(error) => {
            return apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Contract,
                error.to_string(),
            );
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
            );
        }
    };
    let prepared = match runtime.block_on(AutoBle::prepare_with_restoration(identity, identifiers))
    {
        Ok(prepared) => prepared,
        Err(error) => {
            return apple_bluetooth_preparation_failed(
                AppleBluetoothRestorationPreparationFailureStage::Runtime,
                format!("could not create the CoreBluetooth restoration managers: {error:?}"),
            );
        }
    };
    state.pending_apple_bluetooth = Some(PreparedAppleBluetoothOwner {
        key: requested,
        prepared,
    });
    AppleBluetoothRestorationPreparationOutcome::Prepared
}

#[cfg(not(all(feature = "apple", target_os = "ios")))]
fn prepare_apple_bluetooth_restoration_with_supervisor(
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

pub(crate) fn start_configured_with_apple_bluetooth_restoration(
    storage_root: &Path,
    input: DevelopmentNodeStartInput,
    central_identifier: String,
    peripheral_identifier: String,
) -> DevelopmentNodeStartOutcome {
    crate::ios_restoration_probe::install();
    start_configured_with_supervisor(
        supervisor(),
        storage_root,
        input,
        AppleBluetoothPreparation::Restoration {
            central: central_identifier,
            peripheral: peripheral_identifier,
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
    #[cfg(all(feature = "android", target_os = "android"))]
    let android_bluetooth = match crate::android::take_prepared_bluetooth() {
        Some(session) => session,
        None => {
            return DevelopmentNodeStartOutcome::Failed {
                stage: DevelopmentNodeFailureStage::Contract,
                detail:
                    "The Android service must reserve its Bluetooth bridge before node startup."
                        .to_owned(),
            };
        }
    };
    #[cfg(not(all(feature = "apple", target_os = "ios")))]
    let worker_bluetooth_preparation = WorkerBluetoothPreparation {
        preparation: bluetooth_preparation,
        #[cfg(all(feature = "android", target_os = "android"))]
        android: android_bluetooth,
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
    let runtime = Arc::new(OnceLock::new());
    let worker_runtime = Arc::clone(&runtime);
    let host = Arc::new(OnceLock::new());
    let worker_host = Arc::clone(&host);
    let join = std::thread::Builder::new()
        .name("prns-app-native".to_owned())
        .spawn(move || {
            let result = run_worker(
                worker_runtime,
                worker_host,
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
        runtime,
        host,
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

/// A runtime-neutral future for the actor's full snapshot query.
pub async fn snapshot_async() -> DevelopmentNodeSnapshot {
    snapshot_async_with_supervisor(supervisor()).await
}

async fn snapshot_async_with_supervisor(supervisor: &Supervisor) -> DevelopmentNodeSnapshot {
    // Start/Stop hold this mutex while completing bounded native transitions.
    // A JSI caller must never wait for that lock on the JavaScript thread.
    let (commands, runtime, generation) = {
        let state = match supervisor.state.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return supervisor.snapshots.read(),
        };
        let current = supervisor.snapshots.read();
        if current.runtime != DevelopmentNodeRuntime::Running
            || current.active_operation.as_ref().is_some_and(|operation| {
                matches!(
                    operation.kind,
                    DevelopmentNodeOperationKind::RemoteRead
                        | DevelopmentNodeOperationKind::RemoteChange
                        | DevelopmentNodeOperationKind::RemoteWifi
                )
            })
            || current
                .last_announcement
                .as_ref()
                .is_some_and(|operation| operation.status == RemoteControlAnnounceStatus::Pending)
        {
            return current;
        }
        let Some(worker) = &state.worker else {
            return current;
        };
        let Some(runtime) = worker.runtime.get() else {
            return current;
        };
        (
            worker.commands.clone(),
            runtime.clone(),
            current.generation_id,
        )
    };
    let (response, receiver) = oneshot::channel();
    if commands.try_send(Command::Snapshot(response)).is_err() {
        supervisor
            .snapshots
            .set_local_host_unavailable_for_generation(
                generation,
                "The local Host snapshot command could not be admitted.".to_owned(),
            );
        return supervisor.snapshots.read();
    }
    // The timer and waiter run on the node's existing runtime. Aborting the
    // exported future drops this task's receiver, retiring a queued read.
    let mut waiter = CancelOnDrop(
        runtime.spawn(async move { tokio::time::timeout(SNAPSHOT_TIMEOUT, receiver).await }),
    );
    match (&mut waiter.0).await {
        Ok(Ok(Ok(snapshot))) => snapshot,
        _ => {
            supervisor
                .snapshots
                .set_local_host_unavailable_for_generation(
                    generation,
                    "The local Host snapshot command exceeded its bounded wait.".to_owned(),
                );
            supervisor.snapshots.read()
        }
    }
}

struct CancelOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for CancelOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn announce_self_admitted(
    supervisor: &Supervisor,
    state: &SupervisorState,
    input: AnnounceRemoteControlTargetInput,
) -> RemoteControlAnnounceOutcome {
    static NEXT_OPERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    if input.target_identity_fingerprint.len() != 16 {
        return RemoteControlAnnounceOutcome::Failed {
            stage: RemoteControlAnnounceFailureStage::Input,
        };
    }
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
        operation_id: id,
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
            started_at_millis: wall_clock_millis(),
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

const fn lxmf_send_has_capacity(active: usize) -> bool {
    active < LXMF_SEND_TASK_CAPACITY
}

async fn dispatch_lxmf_message_list(
    service: &prns_lxmf::mailbox::DurableDirectLxmfService,
    request: prns_lxmf::mailbox::MailboxListRequest,
    response: Reply<LxmfMessageListOutcome>,
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
    response: Reply<SendDirectTextOutcome>,
    timestamp: u64,
) {
    if !crate::input::bounded_text(&[&input.title, &input.content]) {
        let _ = response.send(SendDirectTextOutcome::DevelopmentUnavailable {
            detail: "The text input exceeds the native size limit.".to_owned(),
        });
        return;
    }
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

    let stop_started_at_millis = wall_clock_millis();
    let terminal_snapshot = supervisor.snapshots.read();
    let preserve_terminal_failure = terminal_snapshot.runtime == DevelopmentNodeRuntime::Failed;
    let prior_terminal_failure = terminal_snapshot.failure;
    supervisor.snapshots.begin_stop(stop_started_at_millis);
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

/// Borrow the current host without bypassing the app's service-draining owner.
pub fn shared_host() -> Option<HostClient> {
    supervisor()
        .lock_state()
        .worker
        .as_ref()?
        .host
        .get()
        .cloned()
}

impl Supervisor {
    fn lock_state(&self) -> MutexGuard<'_, SupervisorState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
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
    published_runtime: Arc<OnceLock<tokio::runtime::Handle>>,
    published_host: Arc<OnceLock<HostClient>>,
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
    let _ = published_runtime.set(runtime.handle().clone());
    runtime.block_on(run_generation(
        published_host,
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
    published_host: Arc<OnceLock<HostClient>>,
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
                match AutoBle::prepare_without_restoration(bluetooth_identity).await {
                    Ok(prepared) => prepared,
                    Err(_) => AutoBle::unavailable_without_restoration(bluetooth_identity),
                }
            }
            AppleBluetoothPreparation::Restoration {
                central,
                peripheral,
            } => {
                let restoration = CoreBluetoothRestorationIdentifiers::new(central, peripheral)
                    .map_err(|error| {
                        boot_failure(
                            &ready,
                            &snapshots,
                            DevelopmentNodeFailureStage::Contract,
                            error.to_string(),
                        )
                    })?;
                match AutoBle::prepare_with_restoration(bluetooth_identity, restoration.clone())
                    .await
                {
                    Ok(prepared) => prepared,
                    Err(_) => {
                        AutoBle::unavailable_with_restoration(bluetooth_identity, restoration)
                    }
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
    let (lxmf_identity, lxmf_destination) = prns_lxmf::direct::prepare_host_lxmf_destination(
        &primary_identity_secret,
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
    let host_config = prns_host::HostConfig {
        identity: prns_host::IdentityConfig::Existing(prns_host::IdentitySecret::new(
            *primary_identity_secret,
        )),
        persistence: prns_host::PersistenceConfig::Directory {
            path: paths.network.to_string_lossy().into_owned(),
        },
        role: prns_host::HostRole::Endpoint,
        destinations: vec![lxmf_destination],
        required_capabilities: Vec::new(),
        limits: prns_host::PrnsLimits::balanced(),
    };
    let (pending_lxmf, lxmf_callbacks) =
        prns_lxmf::mailbox::DurableDirectLxmfService::prepare(lxmf_identity);
    let (event_tx, event_rx) = mpsc::channel(EVENT_LANE_CAPACITY);
    let overflowed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let event_overflowed = Arc::clone(&overflowed);
    let lxmf_events = lxmf_callbacks.clone();
    #[cfg(all(feature = "android", target_os = "android"))]
    let android_bluetooth_session = Arc::new(bluetooth_preparation.android);
    #[cfg(all(feature = "android", target_os = "android"))]
    let prepared_android = android_bluetooth_session.interface(bluetooth_identity);
    let bluetooth_attached = cfg!(any(
        all(
            feature = "apple",
            any(target_os = "ios", target_os = "macos")
        ),
        all(feature = "android", target_os = "android"),
    ));
    let embedding = NativeEmbedding {
        remote_control_config: None,
        remote_control,
        plan_context: Some(
            personal_rns::PlanRuntimeContext::default().with_ble_identity(bluetooth_identity),
        ),
        application_events: ApplicationEventDispatch::NativeCallback,
        on_event: Some(Box::new(move |event| {
            let _lxmf_outcome = lxmf_events.on_prns_event(&event);
            send_event(&event_tx, &event_overflowed, event);
        })),
        accepted_announces: Some(Box::new(lxmf_callbacks.authenticated_announce_observer())),
        prepare_interfaces: Some(Box::new(move |_client| {
            #[allow(unused_mut)]
            let mut attachments = Vec::new();
            #[cfg(all(feature = "apple", any(target_os = "ios", target_os = "macos")))]
            {
                let attached = _client.protocols().attach(prepared_bluetooth);
                attachments.push(NativePreparedAttachment::Registered {
                    interface: attached.id(),
                    kind: InterfaceKind::AutomaticBluetoothLe,
                });
            }
            #[cfg(all(feature = "android", target_os = "android"))]
            attachments.push(NativePreparedAttachment::Supervisor {
                attachment: _client.protocols().supervise(prepared_android),
                kind: InterfaceKind::AutomaticBluetoothLe,
            });
            #[cfg(any(feature = "apple", feature = "android", feature = "host-test"))]
            if let Some(target) = development_tcp_target {
                attachments.push(NativePreparedAttachment::Interface {
                    attachment: _client
                        .protocols()
                        .attach(personal_rns::tcp::TcpClientInterface::new(target)),
                    kind: InterfaceKind::TcpClient,
                });
            }
            #[cfg(not(any(feature = "apple", feature = "android", feature = "host-test")))]
            if development_tcp_target.is_some() {
                return Err(prns_host_native::NativeStartError::Runtime(
                    "this native build does not include the development TCP fixture".into(),
                ));
            }
            Ok(attachments)
        })),
    };
    let owner = OwnedSession::open_with_embedding(host_config, embedding)
        .await
        .map_err(|error| {
            boot_failure(
                &ready,
                &snapshots,
                DevelopmentNodeFailureStage::Node,
                format!("could not start the shared PRNS host: {error:?}"),
            )
        })?;
    // Joining the host belongs to this generation even if service startup fails.
    // A foreign finalizer's scheduled cleanup is insufficient for app reset safety.
    let result = async {
        let host_client = owner.client();
        let services = host_client.native_services().map_err(|error| boot_failure(
            &ready, &snapshots, DevelopmentNodeFailureStage::Node, format!("host unavailable: {error:?}")))?;
        let handle = services.protocols().clone();
        let clock = services.clock();
        let _ = published_host.set(host_client.clone());

        let lxmf_service = pending_lxmf
            .start_paused(
                Arc::new(prns_lxmf::direct::PrnsDirectNetwork::new(host_client.clone())),
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

        snapshots.update(|snapshot| {
            snapshot.controller_identity_fingerprint = Some(controller_identity_fingerprint);
            if !bluetooth_attached {
                snapshot.pairing = RemoteControlPairingState::BluetoothUnavailable;
            }
        });

        let actor = run_actor(
            handle,
            lxmf_service,
            lxmf_refresh,
            commands,
            event_rx,
            overflowed,
            host_client.clone(),
            clock,
            Arc::clone(&snapshots),
            Arc::clone(&operation_admitted),
            ready,
            shutdown_rx,
        );
        tokio::pin!(actor);
        let host_events = host_client.events();
        tokio::select! {
            actor_result = &mut actor => {
                actor_result
            }
            terminal = host_events.wait_terminal() => {
                shutdown.request();
                let actor_result = actor.await;
                actor_result.and(Err((DevelopmentNodeStopStage::Node,
                    format!("shared host stopped before app shutdown completed: {:?}", terminal.state))))
            }
        }
    }.await;
    let joined = owner.stop().await.map_err(map_host_stop_error);
    let result = joined.and(result);
    settle_worker_result(&operation_admitted, &snapshots, &result);
    result
}

fn map_host_stop_error(
    error: prns_host_native::owner::SessionError,
) -> (DevelopmentNodeStopStage, String) {
    use personal_rns::runtime::NodeRunError;
    use prns_host_native::{owner::SessionError, NativeStopError};
    let stage = match &error {
        SessionError::Stop(NativeStopError::NodeFailed(
            NodeRunError::PersistenceFailed
            | NodeRunError::PersistenceWorkerStopped
            | NodeRunError::RemoteControlAuthorizationPersistenceFailed(_),
        )) => DevelopmentNodeStopStage::Persistence,
        _ => DevelopmentNodeStopStage::Node,
    };
    let detail = match error {
        SessionError::Stop(NativeStopError::NodeFailed(error)) => {
            format!("shared host shutdown failed: {error}")
        }
        error => format!("shared host shutdown failed: {error:?}"),
    };
    (stage, detail)
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
    snapshots.interrupt_pending_operations();
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

fn describe_timed_out() -> RemoteControlDescribeOutcome {
    RemoteControlDescribeOutcome::Failed {
        stage: RemoteControlDescribeFailureStage::Timeout,
        detail: "The native Describe command exceeded its bounded wait.".to_owned(),
    }
}

struct DescribeAdmission<'a> {
    snapshots: &'a SnapshotStore,
    admitted: &'a AtomicBool,
}

impl Drop for DescribeAdmission<'_> {
    fn drop(&mut self) {
        if self
            .snapshots
            .read()
            .active_operation
            .is_some_and(|operation| operation.kind == DevelopmentNodeOperationKind::Describe)
        {
            self.snapshots.update(|snapshot| {
                if snapshot.active_operation.as_ref().is_some_and(|operation| {
                    operation.kind == DevelopmentNodeOperationKind::Describe
                }) {
                    snapshot.active_operation = None;
                }
            });
        }
        self.admitted.store(false, Ordering::Release);
    }
}

/// Describe is a read, unlike the separately retained AnnounceSelf operation.
/// Drop its owned future when the caller leaves, its admission deadline expires,
/// or shutdown wins. Return true when the actor must continue shutdown directly.
async fn run_describe_command<F>(
    command: DescribeCommand,
    snapshots: &SnapshotStore,
    admitted: &AtomicBool,
    shutdown: &mut watch::Receiver<bool>,
    operation: impl FnOnce(DescribeRemoteControlTargetInput) -> F,
) -> bool
where
    F: std::future::Future<Output = RemoteControlDescribeOutcome>,
{
    let DescribeCommand {
        input,
        response,
        deadline,
        mut caller,
    } = command;
    let admission = DescribeAdmission {
        snapshots,
        admitted,
    };
    let stopping = async {
        loop {
            if *shutdown.borrow() || shutdown.changed().await.is_err() {
                return;
            }
        }
    };
    let (outcome, stop) = tokio::select! {
        biased;
        () = stopping => (RemoteControlDescribeOutcome::Failed {
            stage: RemoteControlDescribeFailureStage::Node,
            detail: "The native Prns node stopped while Describe was running.".to_owned(),
        }, true),
        _ = &mut caller => (describe_timed_out(), false),
        () = tokio::time::sleep_until(deadline) => (describe_timed_out(), false),
        // Keep invocation lazy: an already expired or cancelled queued command
        // must not start a route request or any other network operation.
        outcome = async { operation(input).await } => (outcome, false),
    };
    drop(admission);
    let _ = response.send(outcome);
    stop
}

#[allow(clippy::too_many_arguments)]
async fn run_actor(
    handle: PrnsNodeHandle,
    lxmf_service: prns_lxmf::mailbox::DurableDirectLxmfService,
    mut lxmf_refresh: watch::Receiver<u64>,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Receiver<OwnedNodeEvent>,
    overflowed: Arc<std::sync::atomic::AtomicBool>,
    host_client: HostClient,
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
        &host_client,
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
    host_client: &HostClient,
    clock: personal_rns::manifold::tokio::TokioClock,
    snapshots: &SnapshotStore,
    operation_admitted: &AtomicBool,
    ready: &std_mpsc::SyncSender<ReadyResult>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> WorkerResult {
    let mut controls = PairingControls::default();
    // The paused service already accepts inbound callbacks. Keep this fallible
    // startup read inside run_actor's guaranteed service-stop/drain scope.
    refresh_lxmf_health(lxmf_service, snapshots)
        .await
        .map(|_| ())
        .map_err(|failure| {
            let detail = lxmf_storage_failure_detail(snapshots, &failure);
            boot_failure(
                ready,
                snapshots,
                DevelopmentNodeFailureStage::Storage,
                detail,
            )
        })?;
    let startup = async {
        loop {
            let Some(event) = events.recv().await else {
                return Err("The native event lane closed before persistence restoration.");
            };
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
    refresh_host_snapshot(host_client, snapshots).await;
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
                Some(Command::Snapshot(mut response)) => {
                    if !response.is_closed() {
                        tokio::select! {
                            biased;
                            () = response.closed() => {},
                            () = refresh_host_snapshot(
                                host_client, snapshots,
                            ) => { let _ = response.send(snapshots.read()); }
                        }
                    }
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
                Some(Command::Describe(command)) => {
                    let deadline = command.deadline;
                    let stop = run_describe_command(
                        command,
                        snapshots,
                        operation_admitted,
                        &mut shutdown_rx,
                        |input| async {
                            if controls.pairing_in_progress() {
                                RemoteControlDescribeOutcome::Busy
                            } else {
                                crate::remote_control::describe(handle, snapshots, input, deadline).await
                            }
                        },
                    ).await;
                    if stop {
                        commands.close();
                        snapshots.set_runtime(DevelopmentNodeRuntime::Stopping);
                        return Ok(());
                    }
                }
                Some(Command::RemoteRead(command)) => {
                    let pairing = controls.pairing_in_progress();
                    if management::run_read(
                        command, snapshots, operation_admitted, &mut shutdown_rx,
                        |input, deadline| async move {
                            if pairing {
                                ReadRemoteNodeOutcome::Busy
                            } else {
                                crate::remote_control::management::read(handle, input, deadline).await
                            }
                        },
                    ).await {
                        commands.close();
                        snapshots.set_runtime(DevelopmentNodeRuntime::Stopping);
                        return Ok(());
                    }
                }
                Some(Command::RemoteChange(operation, deadline)) => {
                    if management::run_change(
                        operation, deadline, snapshots, operation_admitted,
                        &mut shutdown_rx, handle, controls.pairing_in_progress(),
                    ).await {
                        commands.close();
                        snapshots.set_runtime(DevelopmentNodeRuntime::Stopping);
                        return Ok(());
                    }
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
                Some(Command::RemoteWifi(command)) => {
                    if wifi::run(command, snapshots, operation_admitted, &mut shutdown_rx, handle, controls.pairing_in_progress()).await {
                        commands.close();
                        snapshots.set_runtime(DevelopmentNodeRuntime::Stopping);
                        return Ok(());
                    }
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

async fn refresh_host_snapshot(host: &HostClient, snapshots: &SnapshotStore) {
    match tokio::time::timeout(HOST_INSPECTION_TIMEOUT, host.snapshot()).await {
        Ok(Ok(host)) => snapshots.set_local_host(LocalHostState::Running {
            host: Box::new(host),
        }),
        Ok(Err(error)) => snapshots
            .set_local_host_unavailable_if_running(format!("Host inspection failed: {error:?}")),
        Err(_) => snapshots.set_local_host_unavailable_if_running(
            "The local host inspection lane exceeded its bounded wait.".into(),
        ),
    }
}

async fn initiate_pairing(
    handle: &PrnsNodeHandle,
    controls: &mut PairingControls,
    snapshots: &SnapshotStore,
    now: personal_rns::units::InstantMillis,
    input: InitiateRemoteControlPairingInput,
) -> RemoteControlPairingCommandOutcome {
    if !crate::input::bounded_text(&[&input.candidate_id, &input.invitation_code]) {
        return pairing_failed(
            RemoteControlPairingFailureStage::Input,
            "The pairing input exceeds the native size limit.",
        );
    }
    let invitation_code = match parse_invitation_code(&input.invitation_code) {
        Some(code) => code,
        None => {
            return pairing_failed(
                RemoteControlPairingFailureStage::Input,
                "The invitation code must be exactly eight uppercase hexadecimal characters.",
            );
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
            );
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
            started_at_millis: wall_clock_millis(),
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
    if !crate::input::bounded_text(&[&input.attempt_id]) {
        return pairing_failed(
            RemoteControlPairingFailureStage::Input,
            "The pairing decision exceeds the native size limit.",
        );
    }
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
    if !crate::input::bounded_text(&[&input.attempt_id]) {
        return pairing_failed(
            RemoteControlPairingFailureStage::Input,
            "The pairing decision exceeds the native size limit.",
        );
    }
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
mod tests;

mod management;
mod wifi;
