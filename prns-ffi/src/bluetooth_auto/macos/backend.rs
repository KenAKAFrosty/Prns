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
use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tokio::task::JoinSet;

use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, DialOutcome, Origin, ScanningMode,
};
use prns_core::interfaces::bluetooth_auto::{BleAddress, BleIdentity, Control, Psm};

use super::central::{
    is_system_connected, CentralDelegate, CentralPeerSession, DialCommand, DialCompletion,
};
use super::gatt_link::{gatt_inbound_channel, ControlPlane, GattLink};
use super::peripheral::PeripheralDelegate;
#[cfg(target_os = "ios")]
use super::{central_manager_options, peripheral_manager_options};
use super::{
    start_scan, Event, MacosBleError, PeripheralTable, RestoredPeripherals, SendCentralDelegate,
    SendCentralManager, SendPeripheral, SendPeripheralDelegate,
};

const POWER_ON_TIMEOUT: Duration = Duration::from_secs(10);
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum recovery latency for a CoreBluetooth scan that claims to be active but has stopped
/// delivering callbacks. Any discovery callback renews the scan lease without touching the radio.
const RADIO_LIVENESS_INTERVAL: Duration = Duration::from_secs(60);

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

#[derive(Default)]
pub(super) struct StartupReadiness {
    central_powered: bool,
    gatt_service_published: bool,
    l2cap_psm: Option<Psm>,
}

impl StartupReadiness {
    pub(super) fn note_central_powered(&mut self) {
        self.central_powered = true;
    }

    pub(super) fn note_gatt_service_published(&mut self) {
        self.gatt_service_published = true;
    }

    pub(super) fn note_l2cap_published(&mut self, psm: u16) -> Result<(), MacosBleError> {
        self.l2cap_psm = Some(Psm::new(psm).ok_or(MacosBleError::PublishFailed)?);
        Ok(())
    }

    pub(super) fn ready_psm(&self) -> Option<Psm> {
        (self.central_powered && self.gatt_service_published)
            .then_some(self.l2cap_psm)
            .flatten()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DialAdmission {
    AttachCentralSession,
    YieldToSystemConnection,
    /// The target peer already owns an inbound peripheral session. Dialing that same peer as a
    /// central would create the dual-role link that handshake policy is trying to eliminate.
    YieldToInboundSession,
}

pub(super) const fn dial_admission(
    already_system_connected: bool,
    target_has_inbound_session: bool,
) -> DialAdmission {
    if already_system_connected {
        DialAdmission::YieldToSystemConnection
    } else if target_has_inbound_session {
        DialAdmission::YieldToInboundSession
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

fn begin_dial(command: DialCommand, target_has_inbound_session: bool) {
    let DialCommand {
        central,
        delegate,
        peripheral,
        peer_id,
        session,
    } = command;
    match dial_admission(
        is_system_connected(&central, peer_id),
        target_has_inbound_session,
    ) {
        DialAdmission::YieldToSystemConnection => {
            crate::diagnostic_log::debug!(
                "bluetooth: yielding dial to {:02x?} — peer is already connected system-wide; inbound session retains connection ownership",
                peer_id.address().octets()
            );
            session.reject();
            return;
        }
        DialAdmission::YieldToInboundSession => {
            crate::diagnostic_log::debug!(
                "bluetooth: yielding dial to {:02x?} — this peer already owns an inbound peripheral session",
                peer_id.address().octets()
            );
            session.reject();
            return;
        }
        DialAdmission::AttachCentralSession => {}
    }
    // SAFETY: both retained Objective-C objects stay alive for the delegate assignment, which runs
    // on the CoreBluetooth serial dispatch queue.
    unsafe {
        peripheral.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    }
    if !delegate.begin_session(peer_id, session) {
        return;
    }
    // SAFETY: the retained manager and peripheral are owned by this queue-confined command, and
    // CoreBluetooth connection calls are serialized on their dispatch queue.
    unsafe { central.connectPeripheral_options(&peripheral, None) };
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
    events: tokio_mpsc::UnboundedReceiver<Event>,
    psm: Psm,
    seen: HashSet<[u8; 6]>,
    central: SendCentralManager,
    central_delegate: SendCentralDelegate,
    peripheral_delegate: SendPeripheralDelegate,
    peripherals: PeripheralTable,
    restored: RestoredPeripherals,
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

/// Restoration-aware CoreBluetooth managers whose delegates and serial queue already exist, while
/// radio authorization, service publication, and L2CAP readiness remain asynchronous.
pub struct PreparedMacosBleBackend {
    native_thread: NativeThread,
    events: tokio_mpsc::UnboundedReceiver<Event>,
    peripherals: PeripheralTable,
    restored: RestoredPeripherals,
    scan_activity: Arc<AtomicBool>,
    handles: Handles,
}

impl MacosBleBackend {
    #[cfg(target_os = "ios")]
    pub const MAX_PEERS: usize = 7;
    #[cfg(target_os = "macos")]
    pub const MAX_PEERS: usize = 8;

    pub async fn prepare(identity: BleIdentity) -> Result<PreparedMacosBleBackend, MacosBleError> {
        let (events_tx, events_rx) = tokio_mpsc::unbounded_channel::<Event>();
        let (keepalive, shutdown_rx) = sync_mpsc::channel::<()>();
        let (handles_tx, handles_rx) = oneshot::channel::<Handles>();
        let peripherals: PeripheralTable = Arc::new(Mutex::new(HashMap::new()));
        let restored: RestoredPeripherals = Arc::new(Mutex::new(VecDeque::new()));
        let scan_activity = Arc::new(AtomicBool::new(false));
        let central_events = events_tx.clone();
        let peripherals_for_thread = peripherals.clone();
        let restored_for_thread = restored.clone();
        let scan_activity_for_thread = Arc::clone(&scan_activity);

        let join = std::thread::Builder::new()
            .name("prns-corebluetooth".into())
            .spawn(move || {
                let queue = DispatchQueue::new("com.personal.prns.ble", None);

                let central_delegate = CentralDelegate::new(
                    central_events,
                    peripherals_for_thread,
                    restored_for_thread,
                    scan_activity_for_thread,
                );
                let central_proto = ProtocolObject::from_ref(&*central_delegate);
                #[cfg(target_os = "ios")]
                let central_options = Some(central_manager_options());
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

                let peripheral_delegate =
                    PeripheralDelegate::new(events_tx, queue.clone(), identity);
                let peripheral_proto = ProtocolObject::from_ref(&*peripheral_delegate);
                #[cfg(target_os = "ios")]
                let peripheral_options = Some(peripheral_manager_options());
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
            events: events_rx,
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

    pub async fn next_sighting(&mut self) -> Option<BleAddress> {
        loop {
            match self.events.recv().await? {
                Event::Sighting { address, .. } => {
                    if self.seen.insert(*address.octets()) {
                        return Some(address);
                    }
                }
                _ => continue,
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

        let readiness = tokio::time::timeout(POWER_ON_TIMEOUT, async {
            let mut readiness = StartupReadiness::default();
            loop {
                match self.events.recv().await {
                    Some(Event::CentralPowered) => readiness.note_central_powered(),
                    Some(Event::GattServicePublished) => {
                        readiness.note_gatt_service_published();
                    }
                    Some(Event::L2capPublished { psm }) => {
                        readiness.note_l2cap_published(psm)?;
                    }
                    Some(Event::GattServicePublishFailed) => {
                        crate::diagnostic_log::error!(
                            "bluetooth: GATT service publication failed at startup"
                        );
                        return Err(MacosBleError::PublishFailed);
                    }
                    Some(Event::L2capPublishFailed) => {
                        crate::diagnostic_log::error!(
                            "bluetooth: L2CAP publication failed at startup"
                        );
                        return Err(MacosBleError::PublishFailed);
                    }
                    Some(_) => {}
                    None => return Err(MacosBleError::Closed),
                }
                if let Some(psm) = readiness.ready_psm() {
                    return Ok(psm);
                }
            }
        })
        .await;
        let psm = match readiness {
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
            events: self.events,
            psm,
            seen: HashSet::new(),
            central,
            central_delegate,
            peripheral_delegate,
            peripherals: self.peripherals,
            restored: self.restored,
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
                return BleEvent::Sighting {
                    address: peer_id.address(),
                    rssi: None,
                };
            }
            let pending_dials = !self.dials.is_empty();
            tokio::select! {
                event = self.events.recv() => match event {
                    Some(Event::Sighting { address, rssi }) => {
                        crate::diagnostic_log::debug!(
                            "bluetooth: sighted Prns peer {:02x?} rssi={rssi:?}",
                            address.octets()
                        );
                        return BleEvent::Sighting { address, rssi };
                    }
                    Some(Event::Inbound(link)) => return BleEvent::Inbound(link),
                    Some(Event::CentralPowered) if self.scan_enabled => {
                        let central = SendCentralManager(self.central.0.clone());
                        self.queue.exec_async(move || {
                            apply_scanning(central, true, true);
                        });
                        continue;
                    }
                    Some(_) => continue,
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
        let (control_tx, control_rx) = tokio_mpsc::channel::<Control>(8);
        let (completion_tx, completion_rx) = oneshot::channel::<DialCompletion>();
        let (data_inbound_tx, data_inbound_rx) = gatt_inbound_channel();
        let command = DialCommand {
            central: self.central.0.clone(),
            delegate: self.central_delegate.0.clone(),
            peripheral: peripheral.clone(),
            peer_id,
            session: CentralPeerSession::new(address, control_tx, completion_tx, data_inbound_tx),
        };
        crate::diagnostic_log::debug!("bluetooth: dialing {token:02x?} over LE (central role)");
        let peripheral_for_admission = SendPeripheralDelegate(self.peripheral_delegate.0.clone());
        self.queue.exec_async(move || {
            let target_has_inbound_session = peripheral_for_admission.has_inbound_session(peer_id);
            begin_dial(command, target_has_inbound_session);
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
