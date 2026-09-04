use core::time::Duration;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as sync_mpsc;
use std::sync::Arc;

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
    AdvertisingMode, BleBackend, BleEvent, DialOutcome, Origin, RadioMode, ScanningMode,
};
use prns_core::interfaces::bluetooth_auto::{BleAddress, BleIdentity, Control, Psm};

use super::central::{
    discover_prns_services, is_system_connected, CentralDelegate, CentralDialCandidate,
    CentralPeerSession, DialCommand, DialCompletion, DialRejection,
    CENTRAL_CONTROL_INBOUND_CAPACITY,
};
use super::discovery::PeripheralLinkState;
use super::gatt_link::{gatt_inbound_channel, ControlPlane, GattLink};
use super::peripheral::PeripheralDelegate;
#[cfg(target_os = "ios")]
use super::{
    central_manager_options, legacy_restoration_identifiers, peripheral_manager_options,
    CoreBluetoothRestorationIdentifiers,
};
use super::{
    manager_signal_channel, start_scan, CoreBluetoothPeerId, L2capPublicationState, MacosBleError,
    ManagerSignals, PublicationState, SendCentralDelegate, SendCentralManager, SendPeripheral,
    SendPeripheralDelegate, Sighting,
};

const POWER_ON_TIMEOUT: Duration = Duration::from_secs(10);
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
const RADIO_TRANSITION_TIMEOUT: Duration = Duration::from_secs(2);
/// Maximum recovery latency for a CoreBluetooth scan that claims to be active but has stopped
/// delivering callbacks. Any discovery callback renews the scan lease without touching the radio.
const RADIO_LIVENESS_INTERVAL: Duration = Duration::from_secs(60);
const SIGHTING_INGRESS_PER_PEER: usize = 4;

pub(super) const fn central_peripheral_capacity(max_peers: usize) -> usize {
    max_peers.saturating_mul(SIGHTING_INGRESS_PER_PEER)
}

pub(super) struct BoundedRecentSet<T> {
    capacity: usize,
    entries: VecDeque<T>,
}

impl<T: Eq> BoundedRecentSet<T> {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::new(),
        }
    }

    /// Returns true for a newly retained value. Existing values refresh their LRU position but are
    /// not reported again; after eviction a later observation is intentionally new again.
    pub(super) fn insert(&mut self, value: T) -> bool {
        if self.capacity == 0 {
            return false;
        }
        if let Some(position) = self.entries.iter().position(|stored| *stored == value) {
            self.entries.remove(position);
            self.entries.push_back(value);
            return false;
        }
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(value);
        true
    }

    pub(super) fn pop_front(&mut self) -> Option<T> {
        self.entries.pop_front()
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

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
    /// CoreBluetooth restored a pending connection. Reattach the session and let the existing
    /// request complete through `didConnect` instead of issuing a duplicate request.
    AwaitRestoredConnection,
    /// The restored peripheral is already disconnecting or reports an unknown state. Do not
    /// attach protocol state to a connection whose completion semantics are unavailable.
    RejectRestoredConnection,
    YieldToSystemConnection,
    /// The target peer already owns an inbound peripheral session. Dialing that same peer as a
    /// central would create the dual-role link that handshake policy is trying to eliminate.
    YieldToInboundSession,
}

pub(super) const fn dial_admission(
    already_system_connected: bool,
    target_has_inbound_session: bool,
    restored_state: Option<PeripheralLinkState>,
) -> DialAdmission {
    if target_has_inbound_session {
        DialAdmission::YieldToInboundSession
    } else if let Some(state) = restored_state {
        match state {
            PeripheralLinkState::Connected => DialAdmission::ResumeRestoredSession,
            PeripheralLinkState::Connecting => DialAdmission::AwaitRestoredConnection,
            PeripheralLinkState::Disconnected => DialAdmission::AttachCentralSession,
            PeripheralLinkState::Disconnecting | PeripheralLinkState::Unknown => {
                DialAdmission::RejectRestoredConnection
            }
        }
    } else if already_system_connected {
        DialAdmission::YieldToSystemConnection
    } else {
        DialAdmission::AttachCentralSession
    }
}

fn cancel_connection(central: &SendCentralManager, peripheral: &SendPeripheral) {
    // SAFETY: both retained objects remain alive through this call and are messaged only on the
    // CoreBluetooth serial dispatch queue.
    unsafe { central.0.cancelPeripheralConnection(&peripheral.0) };
}

fn schedule_failed_dial_cleanup(
    queue: &DispatchRetained<DispatchQueue>,
    delegate: &SendCentralDelegate,
    peer_id: CoreBluetoothPeerId,
) {
    let delegate = SendCentralDelegate(delegate.0.clone());
    queue.exec_async(move || {
        let delegate = delegate;
        delegate.0.fail_peer(peer_id);
    });
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

fn enqueue_radio_transition(
    queue: &DispatchRetained<DispatchQueue>,
    central: &SendCentralManager,
    central_delegate: &SendCentralDelegate,
    peripheral_delegate: &SendPeripheralDelegate,
    enabled: bool,
    completion: Option<oneshot::Sender<()>>,
) {
    let central = SendCentralManager(central.0.clone());
    let central_delegate = SendCentralDelegate(central_delegate.0.clone());
    let peripheral_delegate = SendPeripheralDelegate(peripheral_delegate.0.clone());
    queue.exec_async(move || {
        let central = central;
        let central_delegate = central_delegate;
        let peripheral_delegate = peripheral_delegate;
        central_delegate.0.set_radio_enabled(&central.0, enabled);
        peripheral_delegate.0.set_radio_enabled(enabled);
        if let Some(completion) = completion {
            let _ = completion.send(());
        }
    });
}

fn begin_dial(command: DialCommand, target_has_inbound_session: bool, restored_connection: bool) {
    let DialCommand {
        central,
        delegate,
        peripheral,
        peer_id,
        session,
    } = command;
    // SAFETY: this exact retained peripheral is queried on its CoreBluetooth serial queue.
    let restored_state =
        restored_connection.then(|| PeripheralLinkState::from(unsafe { peripheral.state() }));
    // The system-wide query remains only a guard for an ordinary observation. Restored admission
    // uses the exact peripheral's state because Apple's restoration array also includes pending
    // connections and may coexist with connections owned by another app.
    let already_system_connected = !restored_connection && is_system_connected(&central, peer_id);
    let admission = dial_admission(
        already_system_connected,
        target_has_inbound_session,
        restored_state,
    );
    let start = match admission {
        DialAdmission::YieldToSystemConnection => {
            crate::diagnostic_log::debug!(
                "bluetooth: yielding dial to {:02x?} — peer is already connected system-wide outside this manager's restored state",
                peer_id.address().octets()
            );
            delegate.discard_peer(peer_id);
            session.reject(DialRejection::YieldToSystemConnection);
            return;
        }
        DialAdmission::YieldToInboundSession => {
            crate::diagnostic_log::debug!(
                "bluetooth: yielding dial to {:02x?} — this peer already owns an inbound peripheral session",
                peer_id.address().octets()
            );
            if restored_connection {
                cancel_connection(
                    &SendCentralManager(central.clone()),
                    &SendPeripheral(peripheral.clone()),
                );
            }
            delegate.discard_peer(peer_id);
            session.reject(DialRejection::YieldToInboundSession);
            return;
        }
        DialAdmission::RejectRestoredConnection => {
            crate::diagnostic_log::warn!(
                "bluetooth: restored connection to {:02x?} is not usable in state {restored_state:?}",
                peer_id.address().octets()
            );
            delegate.discard_peer(peer_id);
            session.reject(DialRejection::RestoredConnectionUnavailable);
            return;
        }
        DialAdmission::AttachCentralSession => DialStart::Connect,
        DialAdmission::ResumeRestoredSession => DialStart::Discover,
        DialAdmission::AwaitRestoredConnection => DialStart::AwaitConnection,
    };
    // SAFETY: both retained Objective-C objects stay alive for the delegate assignment, which runs
    // on the CoreBluetooth serial dispatch queue.
    unsafe {
        peripheral.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    }
    if !delegate.begin_session(&central, peer_id, session) {
        return;
    }
    match start {
        DialStart::Discover => {
            crate::diagnostic_log::debug!(
                "bluetooth: resumed restored connection to {:02x?}, discovering Prns service",
                peer_id.address().octets()
            );
            discover_prns_services(&peripheral);
        }
        DialStart::AwaitConnection => {
            crate::diagnostic_log::debug!(
                "bluetooth: resumed pending connection to {:02x?}, awaiting CoreBluetooth completion",
                peer_id.address().octets()
            );
        }
        DialStart::Connect => {
            // SAFETY: the retained manager and peripheral are owned by this queue-confined command,
            // and CoreBluetooth connection calls are serialized on their dispatch queue.
            unsafe { central.connectPeripheral_options(&peripheral, None) };
        }
    }
}

enum DialStart {
    Connect,
    AwaitConnection,
    Discover,
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
    sighting_events: tokio_mpsc::Receiver<()>,
    psm: Psm,
    seen: BoundedRecentSet<[u8; 6]>,
    central: SendCentralManager,
    central_delegate: SendCentralDelegate,
    peripheral_delegate: SendPeripheralDelegate,
    dial_failures: BoundedRecentSet<BleAddress>,
    dials: JoinSet<DialTaskOutcome>,
    queue: DispatchRetained<DispatchQueue>,
    scan_enabled: bool,
    advertise_enabled: bool,
    scan_activity: Arc<AtomicBool>,
    radio_enabled: Arc<AtomicBool>,
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
    sighting_events: tokio_mpsc::Receiver<()>,
    scan_activity: Arc<AtomicBool>,
    radio_enabled: Arc<AtomicBool>,
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
        let peripheral_capacity = central_peripheral_capacity(Self::MAX_PEERS);
        let (sighting_wake, sighting_events) = tokio_mpsc::channel::<()>(1);
        let (keepalive, shutdown_rx) = sync_mpsc::channel::<()>();
        let (handles_tx, handles_rx) = oneshot::channel::<Handles>();
        let scan_activity = Arc::new(AtomicBool::new(false));
        // CoreBluetooth restoration can arrive during preparation, before the runtime's first
        // set_radio_mode(On), so preparation begins logically enabled.
        let radio_enabled = Arc::new(AtomicBool::new(true));
        let central_manager_signals = manager_signals_tx.clone();
        let scan_activity_for_thread = Arc::clone(&scan_activity);
        let radio_enabled_for_central = Arc::clone(&radio_enabled);
        let radio_enabled_for_peripheral = Arc::clone(&radio_enabled);

        let join = std::thread::Builder::new()
            .name("prns-corebluetooth".into())
            .spawn(move || {
                let queue = DispatchQueue::new("com.personal.prns.ble", None);

                let central_delegate = CentralDelegate::new(
                    central_manager_signals,
                    sighting_wake,
                    scan_activity_for_thread,
                    radio_enabled_for_central,
                    peripheral_capacity,
                    Self::MAX_PEERS,
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
                    radio_enabled_for_peripheral,
                    Self::MAX_PEERS,
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
            sighting_events,
            scan_activity,
            radio_enabled,
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

    fn offer_restored(&self) -> Option<CoreBluetoothPeerId> {
        let (result_tx, result_rx) = sync_mpsc::sync_channel(1);
        let delegate = SendCentralDelegate(self.central_delegate.0.clone());
        self.queue.exec_sync(move || {
            let delegate = delegate;
            let _ = result_tx.try_send(delegate.0.offer_restored());
        });
        result_rx.try_recv().ok().flatten()
    }

    fn pop_sighting(&self) -> Option<Sighting> {
        let (result_tx, result_rx) = sync_mpsc::sync_channel(1);
        let delegate = SendCentralDelegate(self.central_delegate.0.clone());
        self.queue.exec_sync(move || {
            let delegate = delegate;
            let _ = result_tx.try_send(delegate.0.pop_sighting());
        });
        result_rx.try_recv().ok().flatten()
    }

    fn claim(&self, address: BleAddress) -> CentralDialCandidate<SendPeripheral> {
        let (result_tx, result_rx) = sync_mpsc::sync_channel(1);
        let central = SendCentralManager(self.central.0.clone());
        let delegate = SendCentralDelegate(self.central_delegate.0.clone());
        self.queue.exec_sync(move || {
            let central = central;
            let delegate = delegate;
            let candidate = delegate.0.claim(&central.0, address);
            let _ = result_tx.try_send(candidate);
        });
        result_rx
            .try_recv()
            .unwrap_or(CentralDialCandidate::Missing)
    }

    fn clear_local_radio_state(&mut self) {
        while let Ok(link) = self.inbound.try_recv() {
            drop(link);
        }
        while self.sighting_events.try_recv().is_ok() {}
        self.seen.clear();
        self.dial_failures.clear();
        self.scan_activity.store(false, Ordering::Release);
        self.scan_liveness_at = tokio::time::Instant::now() + RADIO_LIVENESS_INTERVAL;
        self.advertising_reconcile_at = tokio::time::Instant::now() + RADIO_LIVENESS_INTERVAL;
    }

    async fn transition_radio(&mut self, enabled: bool) -> Result<(), MacosBleError> {
        // The atomic gate closes synchronously, before any already-enqueued CoreBluetooth callback
        // can publish fresh async work. Queue-owned maps and framework connections are then cleaned
        // on their sole owning queue.
        if !enabled {
            self.radio_enabled.store(false, Ordering::Release);
            self.scan_enabled = false;
            self.advertise_enabled = false;
            self.clear_local_radio_state();
        }

        let (completion_tx, completion_rx) = oneshot::channel();
        enqueue_radio_transition(
            &self.queue,
            &self.central,
            &self.central_delegate,
            &self.peripheral_delegate,
            enabled,
            Some(completion_tx),
        );

        let mut dials = (!enabled).then(|| core::mem::take(&mut self.dials));
        if let Some(dials) = dials.as_mut() {
            dials.abort_all();
        }
        let transition = async move {
            if let Some(mut dials) = dials {
                dials.shutdown().await;
            }
            completion_rx.await.map_err(|_| MacosBleError::Closed)
        };
        let result = match tokio::time::timeout(RADIO_TRANSITION_TIMEOUT, transition).await {
            Ok(result) => result,
            Err(_) => {
                crate::diagnostic_log::error!(
                    "bluetooth: timed out waiting for queue-confined radio transition"
                );
                Err(MacosBleError::RadioTransitionTimeout)
            }
        };
        if !enabled {
            // Callbacks ordered before the queue cleanup may already have emitted bounded wakeups
            // or links. Once the acknowledgement arrives, the atomic gate prevents replacements.
            self.clear_local_radio_state();
        }
        result
    }

    pub async fn next_sighting(&mut self) -> Option<BleAddress> {
        loop {
            if let Some(peer_id) = self.offer_restored() {
                return Some(peer_id.address());
            }
            tokio::select! {
                changed = self.manager_signals.changed(), if self.manager_signals_open => {
                    if changed.is_err() {
                        self.manager_signals_open = false;
                    } else {
                        self.reconcile_manager_signals();
                    }
                }
                wake = self.sighting_events.recv() => {
                    wake?;
                    let Some(Sighting { address, .. }) = self.pop_sighting() else {
                        continue;
                    };
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
            sighting_events: self.sighting_events,
            psm,
            seen: BoundedRecentSet::new(central_peripheral_capacity(MacosBleBackend::MAX_PEERS)),
            central,
            central_delegate,
            peripheral_delegate,
            dial_failures: BoundedRecentSet::new(central_peripheral_capacity(
                MacosBleBackend::MAX_PEERS,
            )),
            dials: JoinSet::new(),
            queue,
            scan_enabled: false,
            advertise_enabled: false,
            scan_activity: self.scan_activity,
            radio_enabled: self.radio_enabled,
            scan_liveness_at: tokio::time::Instant::now() + RADIO_LIVENESS_INTERVAL,
            advertising_reconcile_at: tokio::time::Instant::now() + RADIO_LIVENESS_INTERVAL,
        })
    }
}

impl BleBackend<{ MacosBleBackend::MAX_PEERS }> for MacosBleBackend {
    type Error = MacosBleError;
    type Link = GattLink;

    async fn set_radio_mode(&mut self, mode: RadioMode) -> Result<(), MacosBleError> {
        let enabled = mode.is_on();
        let result = self.transition_radio(enabled).await;
        if result.is_ok() {
            crate::diagnostic_log::debug!(
                "bluetooth: CoreBluetooth logical radio resources {}",
                if enabled { "up" } else { "down" }
            );
        }
        result
    }

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
        let radio_enabled = Arc::clone(&self.radio_enabled);
        self.queue.exec_async(move || {
            if mode.is_on() && !radio_enabled.load(Ordering::Acquire) {
                return;
            }
            apply_scanning(central, mode.is_on(), restart);
        });
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<GattLink> {
        loop {
            if let Some(peer_id) = self.offer_restored() {
                return BleEvent::Sighting {
                    address: peer_id.address(),
                    rssi: None,
                };
            }
            if let Some(address) = self.dial_failures.pop_front() {
                return BleEvent::DialFailed { address };
            }
            let pending_dials = !self.dials.is_empty();
            tokio::select! {
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
                wake = self.sighting_events.recv() => match wake {
                    Some(()) => {
                        let Some(Sighting { address, rssi }) = self.pop_sighting() else {
                            continue;
                        };
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
        if !self.radio_enabled.load(Ordering::Acquire) {
            return DialOutcome::RadioOff;
        }
        let token = *address.octets();
        let candidate = self.claim(address);
        let (peer_id, peripheral, peer_rssi, restored_connection) = match candidate {
            CentralDialCandidate::Ready {
                peer_id,
                peripheral,
                rssi,
                restored,
            } => (peer_id, peripheral, rssi, restored),
            CentralDialCandidate::Busy => {
                self.dial_failures.insert(address);
                return DialOutcome::Started;
            }
            CentralDialCandidate::Missing => {
                crate::diagnostic_log::warn!(
                    "bluetooth: dial to {token:02x?} — peripheral not yet sighted"
                );
                self.dial_failures.insert(address);
                return DialOutcome::Started;
            }
        };
        let (control_tx, control_rx) =
            tokio_mpsc::channel::<Control>(CENTRAL_CONTROL_INBOUND_CAPACITY);
        let (completion_tx, completion_rx) = oneshot::channel::<DialCompletion>();
        let (data_inbound_tx, data_inbound_rx) = gatt_inbound_channel();
        let command = DialCommand {
            central: self.central.0.clone(),
            delegate: self.central_delegate.0.clone(),
            peripheral: peripheral.0.clone(),
            peer_id,
            session: CentralPeerSession::new(address, control_tx, completion_tx, data_inbound_tx),
        };
        crate::diagnostic_log::debug!("bluetooth: dialing {token:02x?} over LE (central role)");
        let peripheral_for_admission = SendPeripheralDelegate(self.peripheral_delegate.0.clone());
        self.queue.exec_async(move || {
            let target_has_inbound_session = peripheral_for_admission.has_inbound_session(peer_id);
            begin_dial(command, target_has_inbound_session, restored_connection);
        });
        let send_peripheral = peripheral;
        let send_peripheral_manager = SendPeripheralDelegate(self.peripheral_delegate.0.clone());
        let delegate = SendCentralDelegate(self.central_delegate.0.clone());
        let queue = self.queue.clone();
        self.dials.spawn(async move {
            let chars = match tokio::time::timeout(DIAL_TIMEOUT, completion_rx).await {
                Ok(Ok(DialCompletion::Ready(chars))) => chars,
                Ok(Ok(DialCompletion::Rejected(rejection))) => {
                    crate::diagnostic_log::debug!(
                        "bluetooth: dial to {token:02x?} rejected: {rejection:?}"
                    );
                    return DialTaskOutcome::Failed { address };
                }
                Ok(Ok(DialCompletion::Failed)) => {
                    crate::diagnostic_log::warn!(
                        "bluetooth: dial to {token:02x?} did not reach control-ready"
                    );
                    return DialTaskOutcome::Failed { address };
                }
                Ok(Err(_)) => {
                    crate::diagnostic_log::warn!(
                        "bluetooth: dial to {token:02x?} closed before reaching control-ready"
                    );
                    schedule_failed_dial_cleanup(&queue, &delegate, peer_id);
                    return DialTaskOutcome::Failed { address };
                }
                Err(_) => {
                    crate::diagnostic_log::warn!(
                        "bluetooth: dial to {token:02x?} timed out before reaching control-ready"
                    );
                    schedule_failed_dial_cleanup(&queue, &delegate, peer_id);
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
        let central = SendCentralManager(self.central.0.clone());
        let delegate = SendCentralDelegate(self.central_delegate.0.clone());
        self.queue.exec_async(move || {
            let central = central;
            let delegate = delegate;
            delegate.0.reap_closed_sessions(&central.0);
        });
        self.peripheral_delegate.0.clear_closed_peer(address);
    }
}

impl Drop for MacosBleBackend {
    fn drop(&mut self) {
        // Drop can run from an arbitrary Tokio or CoreBluetooth-adjacent thread. Close callback
        // ingress immediately, abort owned tasks, and enqueue retained cleanup without synchronously
        // entering the serial dispatch queue.
        self.radio_enabled.store(false, Ordering::Release);
        self.dials.abort_all();
        enqueue_radio_transition(
            &self.queue,
            &self.central,
            &self.central_delegate,
            &self.peripheral_delegate,
            false,
            None,
        );
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
