use core::time::Duration;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as sync_mpsc;
use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
#[cfg(not(target_os = "ios"))]
use objc2::runtime::AnyObject;
use objc2::runtime::ProtocolObject;
use objc2::AnyThread;
use objc2_core_bluetooth::{CBCentralManager, CBPeripheralManager};
#[cfg(not(target_os = "ios"))]
use objc2_foundation::{NSDictionary, NSString};
use tokio::sync::{mpsc as tokio_mpsc, oneshot, watch};
use tokio::task::JoinSet;

use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, DialOutcome, Origin, ScanningMode,
};
use prns_core::interfaces::bluetooth_auto::{BleAddress, BleIdentity, Control, Psm};

use super::central::{
    discover_prns_services, is_system_connected, CentralDelegate, CentralPeerSession, DialCommand,
    DialCompletion, CENTRAL_CONTROL_INBOUND_CAPACITY,
};
use super::gatt_link::{gatt_inbound_channel, ControlPlane, GattLink};
use super::peripheral::PeripheralDelegate;
#[cfg(target_os = "ios")]
use super::{
    central_manager_options, legacy_restoration_identifiers, peripheral_manager_options,
    CoreBluetoothRestorationIdentifiers,
};
use super::{
    manager_signal_channel, start_scan, CoreBluetoothPeerId, L2capPublicationState, MacosBleError,
    ManagerSignals, PeripheralTable, PublicationState, RestoredPeripherals, SendCentralDelegate,
    SendCentralManager, SendPeripheral, SendPeripheralDelegate, Sighting,
};

const POWER_ON_TIMEOUT: Duration = Duration::from_secs(10);
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum recovery latency for a CoreBluetooth scan that claims to be active but has stopped
/// delivering callbacks. Any discovery callback renews the scan lease without touching the radio.
const RADIO_LIVENESS_INTERVAL: Duration = Duration::from_secs(60);
const SIGHTING_INGRESS_PER_PEER: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScanLease {
    Inactive,
    Renewed,
    Expired,
}

pub(super) const fn scan_lease(enabled: bool, activity_observed: bool) -> ScanLease {
    match (enabled, activity_observed) {
        (false, _) => ScanLease::Inactive,
        (true, true) => ScanLease::Renewed,
        (true, false) => ScanLease::Expired,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScanOp {
    Start,
    Restart,
    Stop,
    None,
}

pub(super) const fn scan_op(enabled: bool, is_scanning: bool, restart: bool) -> ScanOp {
    if enabled {
        if is_scanning {
            if restart {
                ScanOp::Restart
            } else {
                ScanOp::None
            }
        } else {
            ScanOp::Start
        }
    } else if is_scanning {
        ScanOp::Stop
    } else {
        ScanOp::None
    }
}

pub(super) fn manager_readiness(signals: ManagerSignals) -> Result<Option<Psm>, MacosBleError> {
    if signals.gatt == PublicationState::Failed {
        crate::diagnostic_log::error!("bluetooth: GATT service publication failed at startup");
        return Err(MacosBleError::PublishFailed);
    }
    if signals.l2cap == L2capPublicationState::Failed {
        crate::diagnostic_log::error!("bluetooth: L2CAP publication failed at startup");
        return Err(MacosBleError::PublishFailed);
    }
    let L2capPublicationState::Published(psm) = signals.l2cap else {
        return Ok(None);
    };
    if signals.central_powered_generation == 0 || signals.gatt != PublicationState::Published {
        return Ok(None);
    }
    Ok(Some(Psm::new(psm).ok_or(MacosBleError::PublishFailed)?))
}

async fn wait_for_readiness(
    signals: &mut watch::Receiver<ManagerSignals>,
) -> Result<(Psm, u64), MacosBleError> {
    loop {
        let current = *signals.borrow_and_update();
        if let Some(psm) = manager_readiness(current)? {
            return Ok((psm, current.central_powered_generation));
        }
        signals.changed().await.map_err(|_| MacosBleError::Closed)?;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DialAdmission {
    AttachCentralSession,
    /// CoreBluetooth restored this central-role connection for the application. Reattach the
    /// delegate and session, then resume discovery without issuing another connection request.
    ResumeRestoredSession,
    YieldToSystemConnection,
    /// The target peer already owns an inbound peripheral session. Dialing that same peer as a
    /// central would create the dual-role link that handshake policy is trying to eliminate.
    YieldToInboundSession,
}

pub(super) const fn dial_admission(
    already_system_connected: bool,
    target_has_inbound_session: bool,
    restored_connection: bool,
) -> DialAdmission {
    if target_has_inbound_session {
        DialAdmission::YieldToInboundSession
    } else if already_system_connected {
        if restored_connection {
            DialAdmission::ResumeRestoredSession
        } else {
            DialAdmission::YieldToSystemConnection
        }
    } else {
        DialAdmission::AttachCentralSession
    }
}

fn cancel_connection(central: &SendCentralManager, peripheral: &SendPeripheral) {
    // SAFETY: both retained objects remain alive through this call and are messaged only on the
    // CoreBluetooth serial dispatch queue.
    unsafe { central.0.cancelPeripheralConnection(&peripheral.0) };
}

fn apply_scanning(central: SendCentralManager, enabled: bool, restart: bool) {
    // SAFETY: this authoritative CoreBluetooth state query runs on the retained manager's
    // serial dispatch queue.
    let is_scanning = unsafe { central.0.isScanning() };
    match scan_op(enabled, is_scanning, restart) {
        ScanOp::Restart => {
            // SAFETY: the retained central manager is only messaged on its serial dispatch queue.
            unsafe { central.0.stopScan() };
            start_scan(&central.0);
            crate::diagnostic_log::debug!(
                "bluetooth: restarted Prns scan so late-arriving peers can be sighted"
            );
        }
        ScanOp::Start => {
            start_scan(&central.0);
            crate::diagnostic_log::debug!("bluetooth: scanning for Prns peers");
        }
        ScanOp::Stop => {
            // SAFETY: the retained central manager is only messaged on its serial dispatch queue.
            unsafe { central.0.stopScan() };
            crate::diagnostic_log::debug!("bluetooth: scanning stopped — at connection capacity");
        }
        ScanOp::None => {}
    }
}

fn begin_dial(command: DialCommand, target_has_inbound_session: bool, restored_connection: bool) {
    let DialCommand {
        central,
        delegate,
        peripheral,
        peer_id,
        session,
    } = command;
    let admission = dial_admission(
        is_system_connected(&central, peer_id),
        target_has_inbound_session,
        restored_connection,
    );
    let resume_restored = match admission {
        DialAdmission::YieldToSystemConnection => {
            crate::diagnostic_log::debug!(
                "bluetooth: yielding dial to {:02x?} — peer is already connected system-wide outside this manager's restored state",
                peer_id.address().octets()
            );
            delegate.discard_restored_callbacks(peer_id);
            session.reject();
            return;
        }
        DialAdmission::YieldToInboundSession => {
            crate::diagnostic_log::debug!(
                "bluetooth: yielding dial to {:02x?} — this peer already owns an inbound peripheral session",
                peer_id.address().octets()
            );
            delegate.discard_restored_callbacks(peer_id);
            session.reject();
            return;
        }
        DialAdmission::AttachCentralSession => false,
        DialAdmission::ResumeRestoredSession => true,
    };
    // SAFETY: both retained Objective-C objects stay alive for the delegate assignment, which runs
    // on the CoreBluetooth serial dispatch queue.
    unsafe {
        peripheral.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    }
    if !delegate.begin_session(peer_id, session) {
        return;
    }
    if resume_restored {
        crate::diagnostic_log::debug!(
            "bluetooth: resumed restored connection to {:02x?}, discovering Prns service",
            peer_id.address().octets()
        );
        discover_prns_services(&peripheral);
    } else {
        // SAFETY: the retained manager and peripheral are owned by this queue-confined command,
        // and CoreBluetooth connection calls are serialized on their dispatch queue.
        unsafe { central.connectPeripheral_options(&peripheral, None) };
    }
}

struct Handles {
    central: SendCentralManager,
    central_delegate: SendCentralDelegate,
    peripheral_delegate: SendPeripheralDelegate,
    queue: DispatchRetained<DispatchQueue>,
}

enum DialTaskOutcome {
    Ready {
        link: GattLink,
        peer_rssi: Option<i8>,
    },
    Failed {
        address: BleAddress,
    },
}

pub struct MacosBleBackend {
    _native_thread: NativeThread,
    manager_signals: watch::Receiver<ManagerSignals>,
    manager_signals_open: bool,
    central_powered_generation: u64,
    inbound: tokio_mpsc::Receiver<GattLink>,
    sightings: tokio_mpsc::Receiver<Sighting>,
    psm: Psm,
    seen: HashSet<[u8; 6]>,
    central: SendCentralManager,
    central_delegate: SendCentralDelegate,
    peripheral_delegate: SendPeripheralDelegate,
    peripherals: PeripheralTable,
    restored: RestoredPeripherals,
    /// Restored peers whose synthesized sighting has been handed to the Host. The first dial
    /// consumes the marker so later system-owned connections keep the ordinary admission policy.
    restored_connections: HashSet<CoreBluetoothPeerId>,
    dials: JoinSet<DialTaskOutcome>,
    queue: DispatchRetained<DispatchQueue>,
    scan_enabled: bool,
    advertise_enabled: bool,
    scan_activity: Arc<AtomicBool>,
    scan_liveness_at: tokio::time::Instant,
    advertising_reconcile_at: tokio::time::Instant,
}

struct NativeThread {
    keepalive: Option<sync_mpsc::Sender<()>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for NativeThread {
    fn drop(&mut self) {
        self.keepalive.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Prepared CoreBluetooth managers whose delegates and serial queue already exist, while radio
/// authorization, service publication, and L2CAP readiness remain asynchronous.
pub struct PreparedMacosBleBackend {
    native_thread: NativeThread,
    manager_signals: watch::Receiver<ManagerSignals>,
    inbound: tokio_mpsc::Receiver<GattLink>,
    sightings: tokio_mpsc::Receiver<Sighting>,
    peripherals: PeripheralTable,
    restored: RestoredPeripherals,
    scan_activity: Arc<AtomicBool>,
    handles: Handles,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ManagerPreparation {
    #[cfg(target_os = "ios")]
    RestorationAware(CoreBluetoothRestorationIdentifiers),
    #[cfg(not(target_os = "ios"))]
    PlatformDefault,
    WithoutRestoration,
}

impl ManagerPreparation {
    #[cfg(target_os = "ios")]
    fn restoration_identifiers(&self) -> Option<&CoreBluetoothRestorationIdentifiers> {
        match self {
            Self::RestorationAware(identifiers) => Some(identifiers),
            Self::WithoutRestoration => None,
        }
    }
}

impl MacosBleBackend {
    #[cfg(target_os = "ios")]
    pub const MAX_PEERS: usize = 7;
    #[cfg(target_os = "macos")]
    pub const MAX_PEERS: usize = 8;

    /// Creates CoreBluetooth managers with the existing iOS restoration identifiers.
    ///
    /// Applications using this path are responsible for the matching background modes and
    /// restoration lifecycle. Use [`Self::prepare_without_restoration`] when the owner
    /// intentionally has no CoreBluetooth state-restoration contract.
    pub async fn prepare(identity: BleIdentity) -> Result<PreparedMacosBleBackend, MacosBleError> {
        #[cfg(target_os = "ios")]
        let manager_preparation =
            ManagerPreparation::RestorationAware(legacy_restoration_identifiers());
        #[cfg(not(target_os = "ios"))]
        let manager_preparation = ManagerPreparation::PlatformDefault;
        Self::prepare_with(identity, manager_preparation).await
    }

    /// Creates CoreBluetooth managers with stable restoration identifiers supplied by the
    /// application that owns their lifecycle.
    ///
    /// The containing application must declare both matching CoreBluetooth background modes and
    /// recreate the managers with these exact identifiers during an iOS restoration launch.
    #[cfg(target_os = "ios")]
    pub async fn prepare_with_restoration(
        identity: BleIdentity,
        identifiers: CoreBluetoothRestorationIdentifiers,
    ) -> Result<PreparedMacosBleBackend, MacosBleError> {
        Self::prepare_with(identity, ManagerPreparation::RestorationAware(identifiers)).await
    }

    /// Creates CoreBluetooth managers without opting into iOS state restoration.
    ///
    /// Central and peripheral operation remain available while the application is running, but
    /// CoreBluetooth will not preserve or restore these managers after process termination.
    pub async fn prepare_without_restoration(
        identity: BleIdentity,
    ) -> Result<PreparedMacosBleBackend, MacosBleError> {
        Self::prepare_with(identity, ManagerPreparation::WithoutRestoration).await
    }

    async fn prepare_with(
        identity: BleIdentity,
        manager_preparation: ManagerPreparation,
    ) -> Result<PreparedMacosBleBackend, MacosBleError> {
        #[cfg(target_os = "ios")]
        let restoration_identifiers = manager_preparation.restoration_identifiers().cloned();
        #[cfg(not(target_os = "ios"))]
        let _ = manager_preparation;
        let (manager_signals_tx, manager_signals_rx) = manager_signal_channel();
        let (inbound_tx, inbound_rx) = tokio_mpsc::channel::<GattLink>(Self::MAX_PEERS);
        let (sightings_tx, sightings_rx) =
            tokio_mpsc::channel::<Sighting>(Self::MAX_PEERS * SIGHTING_INGRESS_PER_PEER);
        let (keepalive, shutdown_rx) = sync_mpsc::channel::<()>();
        let (handles_tx, handles_rx) = oneshot::channel::<Handles>();
        let peripherals: PeripheralTable = Arc::new(Mutex::new(HashMap::new()));
        let restored: RestoredPeripherals = Arc::new(Mutex::new(VecDeque::new()));
        let scan_activity = Arc::new(AtomicBool::new(false));
        let central_manager_signals = manager_signals_tx.clone();
        let peripherals_for_thread = peripherals.clone();
        let restored_for_thread = restored.clone();
        let scan_activity_for_thread = Arc::clone(&scan_activity);

        let join = std::thread::Builder::new()
            .name("prns-corebluetooth".into())
            .spawn(move || {
                let queue = DispatchQueue::new("com.personal.prns.ble", None);

                let central_delegate = CentralDelegate::new(
                    central_manager_signals,
                    sightings_tx,
                    peripherals_for_thread,
                    restored_for_thread,
                    scan_activity_for_thread,
                );
                let central_proto = ProtocolObject::from_ref(&*central_delegate);
                #[cfg(target_os = "ios")]
                let central_options = restoration_identifiers
                    .as_ref()
                    .map(|identifiers| central_manager_options(identifiers.central()));
                #[cfg(not(target_os = "ios"))]
                let central_options: Option<
                    Retained<NSDictionary<NSString, AnyObject>>,
                > = None;
                // SAFETY: the delegate and dispatch queue are retained for at least as long as the
                // manager, and every Objective-C argument has the framework-declared type.
                let central: Retained<CBCentralManager> = unsafe {
                    CBCentralManager::initWithDelegate_queue_options(
                        CBCentralManager::alloc(),
                        Some(central_proto),
                        Some(&queue),
                        central_options.as_deref(),
                    )
                };

                let peripheral_delegate = PeripheralDelegate::new(
                    manager_signals_tx,
                    inbound_tx,
                    queue.clone(),
                    identity,
                );
                let peripheral_proto = ProtocolObject::from_ref(&*peripheral_delegate);
                #[cfg(target_os = "ios")]
                let peripheral_options = restoration_identifiers
                    .as_ref()
                    .map(|identifiers| peripheral_manager_options(identifiers.peripheral()));
                #[cfg(not(target_os = "ios"))]
                let peripheral_options: Option<
                    Retained<NSDictionary<NSString, AnyObject>>,
                > = None;
                // SAFETY: the delegate and dispatch queue are retained for at least as long as the
                // manager, and every Objective-C argument has the framework-declared type.
                let peripheral: Retained<CBPeripheralManager> = unsafe {
                    CBPeripheralManager::initWithDelegate_queue_options(
                        CBPeripheralManager::alloc(),
                        Some(peripheral_proto),
                        Some(&queue),
                        peripheral_options.as_deref(),
                    )
                };

                let _ = handles_tx.send(Handles {
                    central: SendCentralManager(central.clone()),
                    central_delegate: SendCentralDelegate(central_delegate.clone()),
                    peripheral_delegate: SendPeripheralDelegate(peripheral_delegate.clone()),
                    queue: queue.clone(),
                });

                let _ = shutdown_rx.recv();
                let _hold = (central, central_delegate, peripheral_delegate, peripheral);
            })
            .map_err(|_| MacosBleError::Closed)?;
        let native_thread = NativeThread {
            keepalive: Some(keepalive),
            join: Some(join),
        };

        let handles = handles_rx.await.map_err(|_| MacosBleError::Closed)?;
        // Manager creation has completed. Do not await radio authorization or publication here:
        // callers use `ready` after installing the rest of their lifecycle supervision.
        Ok(PreparedMacosBleBackend {
            native_thread,
            manager_signals: manager_signals_rx,
            inbound: inbound_rx,
            sightings: sightings_rx,
            peripherals,
            restored,
            scan_activity,
            handles,
        })
    }

    pub async fn new(identity: BleIdentity) -> Result<Self, MacosBleError> {
        Self::prepare(identity).await?.ready().await
    }

    pub fn psm(&self) -> Psm {
        self.psm
    }

    fn reconcile_manager_signals(&mut self) {
        let generation = self
            .manager_signals
            .borrow_and_update()
            .central_powered_generation;
        let powered_again = generation != self.central_powered_generation;
        self.central_powered_generation = generation;
        if powered_again && self.scan_enabled {
            let central = SendCentralManager(self.central.0.clone());
            self.queue.exec_async(move || {
                apply_scanning(central, true, true);
            });
        }
    }

    pub async fn next_sighting(&mut self) -> Option<BleAddress> {
        loop {
            tokio::select! {
                biased;
                changed = self.manager_signals.changed(), if self.manager_signals_open => {
                    if changed.is_err() {
                        self.manager_signals_open = false;
                    } else {
                        self.reconcile_manager_signals();
                    }
                }
                sighting = self.sightings.recv() => {
                    let Sighting { address, .. } = sighting?;
                    if self.seen.insert(*address.octets()) {
                        return Some(address);
                    }
                }
            }
        }
    }
}

impl PreparedMacosBleBackend {
    pub async fn ready(mut self) -> Result<MacosBleBackend, MacosBleError> {
        let Handles {
            central,
            central_delegate,
            peripheral_delegate,
            queue,
        } = self.handles;
        let readiness = tokio::time::timeout(
            POWER_ON_TIMEOUT,
            wait_for_readiness(&mut self.manager_signals),
        )
        .await;
        let (psm, central_powered_generation) = match readiness {
            Ok(result) => result?,
            Err(_) => {
                crate::diagnostic_log::error!(
                    "bluetooth: timed out waiting for central power, GATT publication, and L2CAP publication — is Bluetooth on and permission granted?"
                );
                return Err(MacosBleError::PowerOnTimeout);
            }
        };
        crate::diagnostic_log::debug!(
            "bluetooth: central powered, GATT service published, L2CAP listener on PSM {:#06x}",
            psm.get()
        );
        Ok(MacosBleBackend {
            _native_thread: self.native_thread,
            manager_signals: self.manager_signals,
            manager_signals_open: true,
            central_powered_generation,
            inbound: self.inbound,
            sightings: self.sightings,
            psm,
            seen: HashSet::new(),
            central,
            central_delegate,
            peripheral_delegate,
            peripherals: self.peripherals,
            restored: self.restored,
            restored_connections: HashSet::new(),
            dials: JoinSet::new(),
            queue,
            scan_enabled: false,
            advertise_enabled: false,
            scan_activity: self.scan_activity,
            scan_liveness_at: tokio::time::Instant::now() + RADIO_LIVENESS_INTERVAL,
            advertising_reconcile_at: tokio::time::Instant::now() + RADIO_LIVENESS_INTERVAL,
        })
    }
}

impl BleBackend<{ MacosBleBackend::MAX_PEERS }> for MacosBleBackend {
    type Error = MacosBleError;
    type Link = GattLink;

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), MacosBleError> {
        self.advertise_enabled = mode.is_on();
        self.advertising_reconcile_at = tokio::time::Instant::now() + RADIO_LIVENESS_INTERVAL;
        self.peripheral_delegate.0.set_advertising(mode);
        Ok(())
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), MacosBleError> {
        self.scan_enabled = mode.is_on();
        self.scan_activity.store(false, Ordering::Relaxed);
        self.scan_liveness_at = tokio::time::Instant::now() + RADIO_LIVENESS_INTERVAL;
        let restart = cfg!(target_os = "macos") && self.scan_enabled;
        let central = SendCentralManager(self.central.0.clone());
        self.queue.exec_async(move || {
            apply_scanning(central, mode.is_on(), restart);
        });
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<GattLink> {
        loop {
            if let Some(peer_id) = self
                .restored
                .lock()
                .ok()
                .and_then(|mut queue| queue.pop_front())
            {
                self.restored_connections.insert(peer_id);
                return BleEvent::Sighting {
                    address: peer_id.address(),
                    rssi: None,
                };
            }
            let pending_dials = !self.dials.is_empty();
            tokio::select! {
                biased;
                changed = self.manager_signals.changed(), if self.manager_signals_open => {
                    if changed.is_err() {
                        self.manager_signals_open = false;
                    } else {
                        self.reconcile_manager_signals();
                    }
                    continue;
                }
                inbound = self.inbound.recv() => match inbound {
                    Some(link) => return BleEvent::Inbound(link),
                    None => core::future::pending().await,
                },
                Some(done) = self.dials.join_next(), if pending_dials => {
                    match done {
                        Ok(DialTaskOutcome::Ready { link, peer_rssi }) => {
                            return BleEvent::LinkReady {
                                link,
                                origin: Origin::Dialed,
                                peer_rssi,
                            };
                        }
                        Ok(DialTaskOutcome::Failed { address }) => {
                            return BleEvent::DialFailed { address };
                        }
                        Err(_) => continue,
                    }
                }
                sighting = self.sightings.recv() => match sighting {
                    Some(Sighting { address, rssi }) => {
                        crate::diagnostic_log::debug!(
                            "bluetooth: sighted Prns peer {:02x?} rssi={rssi:?}",
                            address.octets()
                        );
                        return BleEvent::Sighting { address, rssi };
                    }
                    None => core::future::pending().await,
                },
                _ = tokio::time::sleep_until(self.scan_liveness_at),
                    if cfg!(target_os = "macos") && self.scan_enabled => {
                    let scan_activity = self.scan_activity.swap(false, Ordering::Relaxed);
                    if scan_lease(self.scan_enabled, scan_activity) == ScanLease::Expired {
                        let central = SendCentralManager(self.central.0.clone());
                        self.queue.exec_async(move || {
                            apply_scanning(central, true, true);
                        });
                    }
                    self.scan_liveness_at =
                        tokio::time::Instant::now() + RADIO_LIVENESS_INTERVAL;
                    continue;
                }
                _ = tokio::time::sleep_until(self.advertising_reconcile_at),
                    if cfg!(target_os = "macos") && self.advertise_enabled => {
                    // Reconcile desired advertising with CoreBluetooth's authoritative state.
                    // Healthy advertising remains untouched; a false state is restarted without
                    // bouncing live inbound sessions.
                    self.peripheral_delegate
                        .0
                        .set_advertising(AdvertisingMode::On);
                    self.advertising_reconcile_at =
                        tokio::time::Instant::now() + RADIO_LIVENESS_INTERVAL;
                    continue;
                }
            }
        }
    }

    async fn dial(&mut self, address: BleAddress) -> DialOutcome {
        let token = *address.octets();
        let Some((peer_id, peripheral, peer_rssi)) = self.peripherals.lock().ok().and_then(|map| {
            map.iter()
                .find(|(peer_id, _)| peer_id.address().octets() == &token)
                .map(|(peer_id, (peripheral, rssi))| (*peer_id, peripheral.0.clone(), *rssi))
        }) else {
            crate::diagnostic_log::warn!(
                "bluetooth: dial to {token:02x?} — peripheral not yet sighted"
            );
            self.dials
                .spawn(async move { DialTaskOutcome::Failed { address } });
            return DialOutcome::Started;
        };
        let (control_tx, control_rx) =
            tokio_mpsc::channel::<Control>(CENTRAL_CONTROL_INBOUND_CAPACITY);
        let (completion_tx, completion_rx) = oneshot::channel::<DialCompletion>();
        let (data_inbound_tx, data_inbound_rx) = gatt_inbound_channel();
        let command = DialCommand {
            central: self.central.0.clone(),
            delegate: self.central_delegate.0.clone(),
            peripheral: peripheral.clone(),
            peer_id,
            session: CentralPeerSession::new(address, control_tx, completion_tx, data_inbound_tx),
        };
        let restored_connection = self.restored_connections.remove(&peer_id);
        crate::diagnostic_log::debug!("bluetooth: dialing {token:02x?} over LE (central role)");
        let peripheral_for_admission = SendPeripheralDelegate(self.peripheral_delegate.0.clone());
        self.queue.exec_async(move || {
            let target_has_inbound_session = peripheral_for_admission.has_inbound_session(peer_id);
            begin_dial(command, target_has_inbound_session, restored_connection);
        });
        let send_peripheral = SendPeripheral(peripheral);
        let send_peripheral_manager = SendPeripheralDelegate(self.peripheral_delegate.0.clone());
        let central = SendCentralManager(self.central.0.clone());
        let delegate = SendCentralDelegate(self.central_delegate.0.clone());
        let queue = self.queue.clone();
        self.dials.spawn(async move {
            let chars = match tokio::time::timeout(DIAL_TIMEOUT, completion_rx).await {
                Ok(Ok(DialCompletion::Ready(chars))) => chars,
                Ok(Ok(DialCompletion::Rejected)) => {
                    return DialTaskOutcome::Failed { address };
                }
                Ok(Ok(DialCompletion::Failed)) | Ok(Err(_)) | Err(_) => {
                    crate::diagnostic_log::warn!(
                        "bluetooth: dial to {token:02x?} did not reach control-ready"
                    );
                    queue.exec_async(move || {
                        let central = central;
                        let delegate = delegate;
                        let peripheral = send_peripheral;
                        delegate.0.remove_session(peer_id);
                        cancel_connection(&central, &peripheral);
                    });
                    return DialTaskOutcome::Failed { address };
                }
            };
            DialTaskOutcome::Ready {
                link: GattLink {
                    peer_protocol: chars.peer_protocol,
                    peer_identity: chars.peer_identity,
                    control: ControlPlane::Central {
                        peer_id,
                        peripheral: send_peripheral,
                        characteristic: chars.control,
                        data_characteristic: chars.data,
                        central_delegate: SendCentralDelegate(delegate.0.clone()),
                        queue: queue.clone(),
                        peripheral_manager: send_peripheral_manager,
                    },
                    control_rx,
                    address,
                    data_inbound_rx: Some(data_inbound_rx),
                    l2cap_pending: None,
                },
                peer_rssi,
            }
        });
        DialOutcome::Started
    }

    async fn on_link_closed(&mut self, address: BleAddress) {
        let token = *address.octets();
        if let Some((peer_id, peripheral)) = self.peripherals.lock().ok().and_then(|map| {
            map.iter()
                .find(|(peer_id, _)| peer_id.address().octets() == &token)
                .map(|(peer_id, (peripheral, _))| (*peer_id, peripheral.0.clone()))
        }) {
            let peripheral = SendPeripheral(peripheral);
            let central = SendCentralManager(self.central.0.clone());
            let delegate = SendCentralDelegate(self.central_delegate.0.clone());
            self.queue.exec_async(move || {
                let central = central;
                let delegate = delegate;
                let peripheral = peripheral;
                if delegate.0.remove_closed_session(peer_id) {
                    cancel_connection(&central, &peripheral);
                }
            });
        }
        self.peripheral_delegate.0.clear_closed_peer(address);
    }
}

#[cfg(test)]
mod native_thread_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[cfg(target_os = "macos")]
    #[test]
    fn preparation_without_restoration_is_distinct_from_the_platform_default() {
        assert_ne!(
            ManagerPreparation::WithoutRestoration,
            ManagerPreparation::PlatformDefault
        );
    }

    #[test]
    fn dropping_owner_stops_and_joins_native_thread() {
        let exited = Arc::new(AtomicBool::new(false));
        let exited_on_thread = exited.clone();
        let (keepalive, shutdown_rx) = sync_mpsc::channel::<()>();
        let join = std::thread::spawn(move || {
            let _ = shutdown_rx.recv();
            exited_on_thread.store(true, Ordering::Release);
        });
        let owner = NativeThread {
            keepalive: Some(keepalive),
            join: Some(join),
        };

        drop(owner);
        assert!(exited.load(Ordering::Acquire));
    }
}
