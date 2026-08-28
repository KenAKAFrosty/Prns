mod persistence;
mod sideband_join;
mod worker;

use core::fmt::Write as _;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use personal_hopspot_core::{
    card_label, CardKind, CardLabel, MobileEngineFailure, MobileEngineState,
};
use personal_rns::bluetooth_auto::BluetoothAutoStatus;
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::identity::IdentityHash;
use personal_rns::interfaces::bluetooth_auto::{group_tag, BleIdentity, GROUP_NAME};
use personal_rns::interfaces::{InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus};
use personal_rns::manifold::tokio::TokioInterfaceStatus;
use personal_rns::runtime::{PrnsNodeHandle, RuntimeHealth};
use personal_rns::shared_instance::rns_rpc::RpcAuthenticationKey;
use personal_rns::wifi_auto::AutoWifiStatus;
use personal_rns::wifi_aware::WifiAwareStatus;
use personal_rns::wifi_direct::WifiDirectStatus;
use personal_rns::wire::DestinationHash;
use tokio::sync::oneshot;

use crate::bluetooth_auto::AndroidBleBridge;
use crate::bridge::AndroidUsbBridge;
use crate::service_discovery::AndroidServiceDiscoveryBridge;
use crate::wifi_aware::AndroidWifiAwareBridge;
use crate::wifi_direct::AndroidWifiDirectBridge;
use persistence::{PersistenceHealth, PersistenceSnapshot};

const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 8]);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) const ANDROID_PORT: &str = "android-usb";
pub(super) const LOCAL_RNS_PORT: u16 = 37428;
pub(super) const RPC_PORT: u16 = LOCAL_RNS_PORT + 1;
pub(super) const ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";
pub(super) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";
pub(super) const WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(4);
pub(super) const BLE_DISCOVERY_GROUP_STORAGE: &str = "ble_discovery_group";
const BLE_DISCOVERY_GROUP_CYCLE: &[&str] = &["reticulum", "mt-leg-a", "mt-leg-b"];

struct BleDiscoveryGroupState {
    path: PathBuf,
    group_id: String,
}

fn ble_discovery_group_state() -> &'static Mutex<Option<BleDiscoveryGroupState>> {
    static STATE: OnceLock<Mutex<Option<BleDiscoveryGroupState>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

#[derive(Clone, Copy)]
pub(super) struct EnginePorts {
    pub(super) local: u16,
    pub(super) rpc: u16,
}

impl EnginePorts {
    const ANDROID: Self = Self {
        local: LOCAL_RNS_PORT,
        rpc: RPC_PORT,
    };

    #[cfg(test)]
    const EPHEMERAL: Self = Self { local: 0, rpc: 0 };
}

pub(super) struct EngineResources {
    pub(super) started_at: Instant,
    pub(super) node_identity_hash: IdentityHash,
    pub(super) ble_identity: BleIdentity,
    pub(super) usb_status: TokioInterfaceStatus,
    pub(super) wifi_status: AutoWifiStatus,
    pub(super) ble_status: BluetoothAutoStatus,
    pub(super) wd_status: WifiDirectStatus,
    pub(super) wa_status: WifiAwareStatus,
    pub(super) handle: PrnsNodeHandle,
    pub(super) destination: DestinationHash,
    pub(super) node_page_destination: DestinationHash,
    pub(super) rpc_key: RpcAuthenticationKey,
    pub(super) ports: EnginePorts,
    pub(super) persistence: PersistenceHealth,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct EngineIdentitySnapshot {
    pub(crate) node_identity_hash: IdentityHash,
    pub(crate) ble_identity: BleIdentity,
    pub(crate) delivery_destination: DestinationHash,
    pub(crate) node_page_destination: DestinationHash,
}

#[derive(Clone)]
pub(super) struct PlatformLinks {
    pub(super) usb: AndroidUsbBridge,
    pub(super) ble: AndroidBleBridge,
    pub(super) wifi_direct: AndroidWifiDirectBridge,
    pub(super) wifi_aware: AndroidWifiAwareBridge,
    pub(super) service_discovery: AndroidServiceDiscoveryBridge,
}

impl PlatformLinks {
    fn new() -> Self {
        Self {
            usb: AndroidUsbBridge::new(),
            ble: AndroidBleBridge::new(),
            wifi_direct: AndroidWifiDirectBridge::new(),
            wifi_aware: AndroidWifiAwareBridge::new(),
            service_discovery: AndroidServiceDiscoveryBridge::new(),
        }
    }
}

struct EngineProcess {
    generation: u64,
    resources: Option<EngineResources>,
    platform: PlatformLinks,
    shutdown_tx: Option<oneshot::Sender<()>>,
    stopped_rx: Receiver<WorkerExit>,
    worker: Option<thread::JoinHandle<()>>,
}

struct EngineManager {
    state: MobileEngineState,
    last_failure: MobileEngineFailure,
    storage_dir: Option<PathBuf>,
    generation: u64,
    process: Option<EngineProcess>,
}

impl EngineManager {
    fn new() -> Self {
        Self {
            state: MobileEngineState::Stopped,
            last_failure: MobileEngineFailure::None,
            storage_dir: None,
            generation: 0,
            process: None,
        }
    }

    fn reap_finished(&mut self) {
        let finished = match self.process.as_ref() {
            Some(process) => match process.stopped_rx.try_recv() {
                Ok(exit) => Some(exit),
                Err(TryRecvError::Disconnected) => {
                    Some(WorkerExit::Failed(MobileEngineFailure::WorkerStopped))
                }
                Err(TryRecvError::Empty) => None,
            },
            None => None,
        };
        let Some(mut exit) = finished else {
            return;
        };
        let Some(mut process) = self.process.take() else {
            return;
        };
        if process
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err())
        {
            exit = WorkerExit::Failed(MobileEngineFailure::WorkerStopped);
        }
        match exit {
            WorkerExit::Stopped => {
                if self.state != MobileEngineState::Stopped {
                    self.state = MobileEngineState::Failed;
                    self.last_failure = MobileEngineFailure::WorkerStopped;
                }
            }
            WorkerExit::Failed(failure) => {
                self.state = MobileEngineState::Failed;
                self.last_failure = failure;
            }
        }
    }
}

static ENGINE_MANAGER: OnceLock<Mutex<EngineManager>> = OnceLock::new();

fn lock_manager() -> MutexGuard<'static, EngineManager> {
    ENGINE_MANAGER
        .get_or_init(|| Mutex::new(EngineManager::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineStartError {
    StorageConfiguration,
    PersistenceWrite,
    WorkerSpawn,
    RuntimeBuild,
    LocalListenerBind,
    RpcListenerBind,
    StartupTimeout,
    WorkerStopped,
}

impl EngineStartError {
    #[must_use]
    pub(crate) const fn failure(self) -> MobileEngineFailure {
        match self {
            Self::StorageConfiguration => MobileEngineFailure::StorageConfiguration,
            Self::PersistenceWrite => MobileEngineFailure::PersistenceWrite,
            Self::WorkerSpawn => MobileEngineFailure::WorkerSpawn,
            Self::RuntimeBuild => MobileEngineFailure::RuntimeBuild,
            Self::LocalListenerBind => MobileEngineFailure::LocalListenerBind,
            Self::RpcListenerBind => MobileEngineFailure::RpcListenerBind,
            Self::StartupTimeout => MobileEngineFailure::StartupTimeout,
            Self::WorkerStopped => MobileEngineFailure::WorkerStopped,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineStopError {
    PersistenceWrite,
    ShutdownTimeout,
    WorkerStopped,
}

impl EngineStopError {
    #[must_use]
    pub(crate) const fn failure(self) -> MobileEngineFailure {
        match self {
            Self::PersistenceWrite => MobileEngineFailure::PersistenceWrite,
            Self::ShutdownTimeout => MobileEngineFailure::ShutdownTimeout,
            Self::WorkerStopped => MobileEngineFailure::WorkerStopped,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum WorkerExit {
    Stopped,
    Failed(MobileEngineFailure),
}

pub(crate) fn start(storage_dir: PathBuf) -> Result<(), EngineStartError> {
    start_with_ports(storage_dir, EnginePorts::ANDROID)
}

fn start_with_ports(storage_dir: PathBuf, ports: EnginePorts) -> Result<(), EngineStartError> {
    let storage_dir = prepare_storage_dir(storage_dir)?;
    let platform = PlatformLinks::new();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (stopped_tx, stopped_rx) = mpsc::channel();

    let generation = {
        let mut manager = lock_manager();
        manager.reap_finished();
        if let Some(configured) = manager.storage_dir.as_ref() {
            if configured != &storage_dir {
                manager.last_failure = MobileEngineFailure::StorageConfiguration;
                return Err(EngineStartError::StorageConfiguration);
            }
        }
        if manager.state == MobileEngineState::Running {
            return Ok(());
        }
        if manager.process.is_some() {
            return Err(EngineStartError::WorkerStopped);
        }
        manager.storage_dir = Some(storage_dir.clone());
        manager.generation = manager.generation.wrapping_add(1);
        manager.state = MobileEngineState::Starting;
        manager.last_failure = MobileEngineFailure::None;
        let generation = manager.generation;
        let worker_platform = platform.clone();
        let worker = thread::Builder::new()
            .name("hopspot-engine".into())
            .spawn(move || {
                worker::run(worker::WorkerInput {
                    storage_dir,
                    platform: worker_platform,
                    ready_tx,
                    shutdown_rx,
                    stopped_tx,
                    ports,
                })
            })
            .map_err(|_| {
                manager.state = MobileEngineState::Failed;
                manager.last_failure = MobileEngineFailure::WorkerSpawn;
                EngineStartError::WorkerSpawn
            })?;
        manager.process = Some(EngineProcess {
            generation,
            resources: None,
            platform,
            shutdown_tx: Some(shutdown_tx),
            stopped_rx,
            worker: Some(worker),
        });
        generation
    };

    let ready = ready_rx.recv_timeout(STARTUP_TIMEOUT);
    let mut manager = lock_manager();
    if manager.generation != generation {
        return Err(EngineStartError::WorkerStopped);
    }
    let Some(process) = manager.process.as_mut() else {
        return Err(EngineStartError::WorkerStopped);
    };
    if process.generation != generation {
        return Err(EngineStartError::WorkerStopped);
    }
    match ready {
        Ok(Ok(resources)) => {
            process.resources = Some(resources);
            manager.state = MobileEngineState::Running;
            manager.last_failure = MobileEngineFailure::None;
            Ok(())
        }
        Ok(Err(error)) => {
            manager.state = MobileEngineState::Failed;
            manager.last_failure = error.failure();
            Err(error)
        }
        Err(RecvTimeoutError::Timeout) => {
            if let Some(shutdown_tx) = process.shutdown_tx.take() {
                let _ = shutdown_tx.send(());
            }
            manager.state = MobileEngineState::Failed;
            manager.last_failure = MobileEngineFailure::StartupTimeout;
            Err(EngineStartError::StartupTimeout)
        }
        Err(RecvTimeoutError::Disconnected) => {
            manager.state = MobileEngineState::Failed;
            manager.last_failure = MobileEngineFailure::WorkerStopped;
            Err(EngineStartError::WorkerStopped)
        }
    }
}

fn prepare_storage_dir(storage_dir: PathBuf) -> Result<PathBuf, EngineStartError> {
    if !storage_dir.is_absolute() {
        record_configuration_failure();
        return Err(EngineStartError::StorageConfiguration);
    }
    if std::fs::create_dir_all(&storage_dir).is_err() {
        record_configuration_failure();
        return Err(EngineStartError::StorageConfiguration);
    }
    storage_dir.canonicalize().map_err(|_| {
        record_configuration_failure();
        EngineStartError::StorageConfiguration
    })
}

fn record_configuration_failure() {
    let mut manager = lock_manager();
    if manager.state != MobileEngineState::Running {
        manager.state = MobileEngineState::Failed;
    }
    manager.last_failure = MobileEngineFailure::StorageConfiguration;
}

pub(crate) fn stop() -> Result<(), EngineStopError> {
    let mut process = {
        let mut manager = lock_manager();
        manager.reap_finished();
        let Some(process) = manager.process.take() else {
            manager.state = MobileEngineState::Stopped;
            return Ok(());
        };
        manager.generation = manager.generation.wrapping_add(1);
        process
    };

    if let Some(shutdown_tx) = process.shutdown_tx.take() {
        let _ = shutdown_tx.send(());
    }
    let exit = process.stopped_rx.recv_timeout(SHUTDOWN_TIMEOUT);
    match exit {
        Ok(exit) => finish_stopped_process(process, exit),
        Err(RecvTimeoutError::Disconnected) => finish_stopped_process(
            process,
            WorkerExit::Failed(MobileEngineFailure::WorkerStopped),
        ),
        Err(RecvTimeoutError::Timeout) => {
            let mut manager = lock_manager();
            manager.state = MobileEngineState::Failed;
            manager.last_failure = MobileEngineFailure::ShutdownTimeout;
            manager.process = Some(process);
            Err(EngineStopError::ShutdownTimeout)
        }
    }
}

fn finish_stopped_process(
    mut process: EngineProcess,
    mut exit: WorkerExit,
) -> Result<(), EngineStopError> {
    if process
        .worker
        .take()
        .is_some_and(|worker| worker.join().is_err())
    {
        exit = WorkerExit::Failed(MobileEngineFailure::WorkerStopped);
    }
    let mut manager = lock_manager();
    match exit {
        WorkerExit::Stopped => {
            manager.state = MobileEngineState::Stopped;
            Ok(())
        }
        WorkerExit::Failed(failure) => {
            manager.state = MobileEngineState::Failed;
            manager.last_failure = failure;
            if failure == MobileEngineFailure::PersistenceWrite {
                Err(EngineStopError::PersistenceWrite)
            } else {
                Err(EngineStopError::WorkerStopped)
            }
        }
    }
}

pub(crate) fn engine_state() -> MobileEngineState {
    let mut manager = lock_manager();
    manager.reap_finished();
    manager.state
}

pub(crate) fn last_failure() -> MobileEngineFailure {
    let mut manager = lock_manager();
    manager.reap_finished();
    manager.last_failure
}

pub(crate) fn interface_snapshots() -> Vec<InterfaceSnapshot> {
    let mut manager = lock_manager();
    manager.reap_finished();
    manager
        .process
        .as_ref()
        .and_then(|process| process.resources.as_ref())
        .map(|resources| resources.handle.interfaces())
        .unwrap_or_default()
}

pub(crate) fn runtime_health() -> Option<RuntimeHealth> {
    let mut manager = lock_manager();
    manager.reap_finished();
    let resources = manager.process.as_ref()?.resources.as_ref()?;
    Some(RuntimeHealth::from_snapshots(
        resources.started_at.elapsed(),
        &resources.handle.interfaces(),
    ))
}

pub(crate) fn persistence_snapshot() -> Option<PersistenceSnapshot> {
    let mut manager = lock_manager();
    manager.reap_finished();
    let resources = manager.process.as_ref()?.resources.as_ref()?;
    Some(resources.persistence.snapshot())
}

pub(crate) fn rpc_key_hex() -> Option<String> {
    let mut manager = lock_manager();
    manager.reap_finished();
    let resources = manager.process.as_ref()?.resources.as_ref()?;
    Some(hex(resources.rpc_key.as_bytes()))
}

pub(crate) fn sideband_join_config() -> Option<String> {
    let mut manager = lock_manager();
    manager.reap_finished();
    let resources = manager.process.as_ref()?.resources.as_ref()?;
    Some(sideband_join::render(&resources.rpc_key, resources.ports))
}

pub(crate) fn identity_snapshot() -> Option<EngineIdentitySnapshot> {
    let mut manager = lock_manager();
    manager.reap_finished();
    let resources = manager.process.as_ref()?.resources.as_ref()?;
    Some(EngineIdentitySnapshot {
        node_identity_hash: resources.node_identity_hash,
        ble_identity: resources.ble_identity,
        delivery_destination: resources.destination,
        node_page_destination: resources.node_page_destination,
    })
}

pub(crate) fn node_identity_hash_hex() -> Option<String> {
    identity_snapshot().map(|identity| hex(identity.node_identity_hash.as_bytes()))
}

pub(crate) fn ble_identity_hex() -> Option<String> {
    identity_snapshot().map(|identity| hex(identity.ble_identity.as_bytes()))
}

pub(crate) fn ble_discovery_group() -> Option<String> {
    ble_discovery_group_state()
        .lock()
        .ok()?
        .as_ref()
        .map(|state| state.group_id.clone())
}

pub(crate) fn set_ble_discovery_group(group_id: &str) -> bool {
    let group_id = group_id.trim();
    if group_id.is_empty() || group_id.len() > 64 {
        return false;
    }
    let Ok(mut slot) = ble_discovery_group_state().lock() else {
        return false;
    };
    let Some(state) = slot.as_mut() else {
        return false;
    };
    if std::fs::write(&state.path, group_id.as_bytes()).is_err() {
        return false;
    }
    state.group_id = group_id.to_string();
    let bridge = ble_bridge();
    bridge.set_local_group_tag(group_tag(group_id.as_bytes()));
    drop(slot);
    {
        let manager = lock_manager();
        if let Some(resources) = manager
            .process
            .as_ref()
            .and_then(|process| process.resources.as_ref())
        {
            let _ = resources
                .handle
                .set_interface_group_id(resources.ble_status.id(), group_id);
        }
    }
    republish_ble_discovery_group();
    true
}

pub(crate) fn cycle_ble_discovery_group() -> Option<String> {
    let current = ble_discovery_group().unwrap_or_else(|| GROUP_NAME.to_string());
    let next = BLE_DISCOVERY_GROUP_CYCLE
        .iter()
        .position(|candidate| *candidate == current.as_str())
        .map(|index| BLE_DISCOVERY_GROUP_CYCLE[(index + 1) % BLE_DISCOVERY_GROUP_CYCLE.len()])
        .unwrap_or(BLE_DISCOVERY_GROUP_CYCLE[0]);
    if set_ble_discovery_group(next) {
        Some(next.to_string())
    } else {
        None
    }
}

fn republish_ble_discovery_group() {
    let mut manager = lock_manager();
    manager.reap_finished();
    let Some(resources) = manager
        .process
        .as_ref()
        .and_then(|process| process.resources.as_ref())
    else {
        return;
    };
    // Evict peers + refresh discovery group without cycling the radio. A disable/enable bounce
    // races the Android radio pump (PSM cleared in Rust while Kotlin keeps the L2CAP server).
    resources.ble_status.reset_peers();
}

pub(super) fn install_ble_discovery_group(storage_dir: &std::path::Path, group_id: String) {
    let path = storage_dir.join(BLE_DISCOVERY_GROUP_STORAGE);
    if let Ok(mut slot) = ble_discovery_group_state().lock() {
        *slot = Some(BleDiscoveryGroupState { path, group_id });
    }
}

pub(super) fn load_ble_discovery_group(storage_dir: &std::path::Path) -> String {
    let path = storage_dir.join(BLE_DISCOVERY_GROUP_STORAGE);
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| GROUP_NAME.to_string())
}

pub(crate) fn delivery_destination_hex() -> Option<String> {
    identity_snapshot().map(|identity| hex(identity.delivery_destination.as_bytes()))
}

pub(crate) fn node_page_destination_hex() -> Option<String> {
    identity_snapshot().map(|identity| hex(identity.node_page_destination.as_bytes()))
}

pub(crate) fn wifi_aware_failure_reason() -> Option<&'static str> {
    let mut manager = lock_manager();
    manager.reap_finished();
    let resources = manager.process.as_ref()?.resources.as_ref()?;
    resources.wa_status.failure_reason()
}

pub(crate) fn wifi_direct_failure_reason() -> Option<&'static str> {
    let mut manager = lock_manager();
    manager.reap_finished();
    let resources = manager.process.as_ref()?.resources.as_ref()?;
    resources.wd_status.failure_reason()
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub(crate) fn toggle_interface(id: InterfaceId) {
    let mut manager = lock_manager();
    manager.reap_finished();
    let Some(resources) = manager
        .process
        .as_ref()
        .and_then(|process| process.resources.as_ref())
    else {
        return;
    };
    if id == USB_INTERFACE_ID {
        resources.usb_status.toggle_enabled();
    } else if id == resources.wifi_status.id() {
        resources.wifi_status.toggle_enabled();
    } else if id.kind() == Some(InterfaceKind::BluetoothAuto) {
        resources.ble_status.toggle_enabled();
    } else if id.kind() == Some(InterfaceKind::WifiDirect) {
        resources.wd_status.toggle_enabled();
    } else if id.kind() == Some(InterfaceKind::WifiAware) {
        resources.wa_status.toggle_enabled();
    }
}

pub(crate) fn sleep_interfaces() {
    set_interfaces_enabled(false);
}

pub(crate) fn wake_interfaces() {
    set_interfaces_enabled(true);
}

fn set_interfaces_enabled(enabled: bool) {
    let mut manager = lock_manager();
    manager.reap_finished();
    let Some(resources) = manager
        .process
        .as_ref()
        .and_then(|process| process.resources.as_ref())
    else {
        return;
    };
    if enabled {
        resources.usb_status.enable();
        resources.wifi_status.enable();
        resources.ble_status.enable();
        resources.wd_status.enable();
        resources.wa_status.enable();
    } else {
        resources.usb_status.disable();
        resources.wifi_status.disable();
        resources.ble_status.disable();
        resources.wd_status.disable();
        resources.wa_status.disable();
    }
}

pub(crate) fn announce() {
    let mut manager = lock_manager();
    manager.reap_finished();
    let Some(resources) = manager
        .process
        .as_ref()
        .and_then(|process| process.resources.as_ref())
    else {
        return;
    };
    log::info!("hopspot: manual announce -> nomadnetwork.node on every interface");
    let _ = resources
        .handle
        .issue(PrnsCommand::AnnounceNow(AnnounceNow {
            destination: resources.node_page_destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        }));
}

pub(crate) fn usb_bridge() -> AndroidUsbBridge {
    lock_manager()
        .process
        .as_ref()
        .map(|process| process.platform.usb.clone())
        .unwrap_or_default()
}

pub(crate) fn ble_bridge() -> AndroidBleBridge {
    lock_manager()
        .process
        .as_ref()
        .map(|process| process.platform.ble.clone())
        .unwrap_or_default()
}

pub(crate) fn wd_bridge() -> AndroidWifiDirectBridge {
    lock_manager()
        .process
        .as_ref()
        .map(|process| process.platform.wifi_direct.clone())
        .unwrap_or_default()
}

pub(crate) fn wa_bridge() -> AndroidWifiAwareBridge {
    lock_manager()
        .process
        .as_ref()
        .map(|process| process.platform.wifi_aware.clone())
        .unwrap_or_default()
}

pub(crate) fn service_discovery_bridge() -> AndroidServiceDiscoveryBridge {
    lock_manager()
        .process
        .as_ref()
        .map(|process| process.platform.service_discovery.clone())
        .unwrap_or_default()
}

pub(crate) fn classify(id: InterfaceId) -> Option<(CardKind, CardLabel)> {
    if id == USB_INTERFACE_ID {
        return Some((CardKind::Usb, card_label("USB")));
    }
    match id.kind() {
        Some(InterfaceKind::AutoWifi) => Some((CardKind::Wifi, card_label("LAN"))),
        Some(InterfaceKind::LocalServer) => Some((CardKind::SharedInstance, card_label("Local"))),
        Some(InterfaceKind::LocalClient) => Some((CardKind::Peer, card_label("App"))),
        Some(InterfaceKind::BluetoothAuto) => Some((CardKind::Ble, card_label("BLE"))),
        Some(InterfaceKind::WifiDirect) => Some((CardKind::Wifi, card_label("Direct"))),
        Some(InterfaceKind::WifiAware) => Some((CardKind::Wifi, card_label("Aware"))),
        kind => {
            let bytes = id.as_bytes();
            let (card_kind, tag) = match kind {
                Some(InterfaceKind::BluetoothPeer) => (CardKind::Ble, "BLE"),
                Some(InterfaceKind::WifiDirectPeer) => (CardKind::Wifi, "WD"),
                Some(InterfaceKind::WifiAwarePeer) => (CardKind::Wifi, "P2P"),
                _ => (CardKind::Peer, "Peer"),
            };
            let mut label = CardLabel::new();
            let _ = write!(label, "{tag} {:02x}{:02x}", bytes[1], bytes[2]);
            Some((card_kind, label))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_hopspot_core::card_label_max_chars;

    #[test]
    fn wifi_direct_is_presented_as_a_lab_transport() {
        let id = InterfaceId::from_channel_tag(InterfaceKind::WifiDirect, b"wifi-direct");
        let (_, label) = classify(id).expect("Wi-Fi Direct has a card");
        assert_eq!(label.as_str(), "Direct");
    }

    #[test]
    fn every_android_interface_label_fits_its_card() {
        let kinds = [
            InterfaceKind::AutoWifi,
            InterfaceKind::LocalServer,
            InterfaceKind::LocalClient,
            InterfaceKind::BluetoothAuto,
            InterfaceKind::WifiDirect,
            InterfaceKind::WifiAware,
            InterfaceKind::BluetoothPeer,
            InterfaceKind::WifiDirectPeer,
            InterfaceKind::WifiAwarePeer,
        ];
        for kind in kinds {
            let id = InterfaceId::from_channel_tag(kind, b"label-fit");
            let (card_kind, label) = classify(id).expect("known interface has a card");
            assert!(
                label.chars().count() <= card_label_max_chars(card_kind),
                "{kind:?} label {label:?} exceeds its card"
            );
        }
    }

    #[test]
    fn engine_stops_and_restarts_with_durable_identity_and_runtime_state() {
        let storage_dir =
            std::env::temp_dir().join(format!("personal-hopspot-android-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&storage_dir);

        start_with_ports(storage_dir.clone(), EnginePorts::EPHEMERAL).unwrap();
        assert_eq!(engine_state(), MobileEngineState::Running);
        let first_key = rpc_key_hex().unwrap();
        assert_eq!(first_key.len(), 64);
        let first_sideband_config = sideband_join_config().unwrap();
        assert!(first_sideband_config.contains(&format!("rpc_key = {first_key}")));
        let first_identity = identity_snapshot().unwrap();
        assert_eq!(persistence_snapshot().unwrap().successful_flushes, 1);
        stop().unwrap();
        assert_eq!(engine_state(), MobileEngineState::Stopped);
        for region in ["timebase", "routing_table", "tunnels", "known_destinations"] {
            assert!(storage_dir.join("prns").join(region).is_file());
        }

        start_with_ports(storage_dir.clone(), EnginePorts::EPHEMERAL).unwrap();
        assert_eq!(engine_state(), MobileEngineState::Running);
        assert_eq!(rpc_key_hex().unwrap(), first_key);
        assert_eq!(sideband_join_config().unwrap(), first_sideband_config);
        assert_eq!(identity_snapshot().unwrap(), first_identity);
        let persistence = persistence_snapshot().unwrap();
        assert_eq!(persistence.restore.refused, 0);
        assert_eq!(persistence.restore.dropped, 0);
        assert_eq!(persistence.restore.ratchets, 1);
        assert_eq!(persistence.successful_flushes, 1);
        stop().unwrap();
        assert_eq!(engine_state(), MobileEngineState::Stopped);

        std::fs::write(storage_dir.join("prns").join("routing_table"), b"damaged").unwrap();
        start_with_ports(storage_dir.clone(), EnginePorts::EPHEMERAL).unwrap();
        let persistence = persistence_snapshot().unwrap();
        assert_eq!(persistence.restore.refused, 1);
        assert_eq!(persistence.successful_flushes, 1);
        stop().unwrap();
        assert_eq!(engine_state(), MobileEngineState::Stopped);

        std::fs::remove_dir_all(storage_dir.join("prns")).unwrap();
        std::fs::write(storage_dir.join("prns"), b"not a directory").unwrap();
        assert_eq!(
            start_with_ports(storage_dir.clone(), EnginePorts::EPHEMERAL),
            Err(EngineStartError::StorageConfiguration)
        );
        assert_eq!(last_failure(), MobileEngineFailure::StorageConfiguration);
        assert_eq!(stop(), Err(EngineStopError::WorkerStopped));
        std::fs::remove_dir_all(storage_dir).unwrap();
    }
}
