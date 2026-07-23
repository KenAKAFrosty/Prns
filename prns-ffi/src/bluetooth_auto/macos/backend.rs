use core::time::Duration;
use std::collections::{HashMap, HashSet, VecDeque};
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

use super::central::{CentralDelegate, CentralPeerSession, DialCommand, DialCompletion};
use super::gatt_link::{ControlPlane, GattLink};
use super::peripheral::PeripheralDelegate;
#[cfg(target_os = "ios")]
use super::{central_manager_options, peripheral_manager_options};
use super::{
    start_scan, Event, MacosBleError, PeripheralTable, RestoredPeripherals, SendCentralDelegate,
    SendCentralManager, SendPeripheral, SendPeripheralDelegate,
};

const POWER_ON_TIMEOUT: Duration = Duration::from_secs(10);
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);

fn cancel_connection(central: &SendCentralManager, peripheral: &SendPeripheral) {
    // SAFETY: both retained objects remain alive through this call and are messaged only on the
    // CoreBluetooth serial dispatch queue.
    unsafe { central.0.cancelPeripheralConnection(&peripheral.0) };
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
    _keepalive: sync_mpsc::Sender<()>,
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
}

impl MacosBleBackend {
    #[cfg(target_os = "ios")]
    pub const MAX_PEERS: usize = 7;
    #[cfg(target_os = "macos")]
    pub const MAX_PEERS: usize = 8;

    pub async fn new(identity: BleIdentity) -> Result<Self, MacosBleError> {
        let (events_tx, mut events_rx) = tokio_mpsc::unbounded_channel::<Event>();
        let (keepalive, shutdown_rx) = sync_mpsc::channel::<()>();
        let (handles_tx, handles_rx) = oneshot::channel::<Handles>();
        let peripherals: PeripheralTable = Arc::new(Mutex::new(HashMap::new()));
        let restored: RestoredPeripherals = Arc::new(Mutex::new(VecDeque::new()));
        let central_events = events_tx.clone();
        let peripherals_for_thread = peripherals.clone();
        let restored_for_thread = restored.clone();

        std::thread::spawn(move || {
            let queue = DispatchQueue::new("com.personal.prns.ble", None);

            let central_delegate =
                CentralDelegate::new(central_events, peripherals_for_thread, restored_for_thread);
            let central_proto = ProtocolObject::from_ref(&*central_delegate);
            #[cfg(target_os = "ios")]
            let central_options = Some(central_manager_options());
            #[cfg(not(target_os = "ios"))]
            let central_options: Option<Retained<NSDictionary<NSString, AnyObject>>> = None;
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

            let peripheral_delegate = PeripheralDelegate::new(events_tx, queue.clone(), identity);
            let peripheral_proto = ProtocolObject::from_ref(&*peripheral_delegate);
            #[cfg(target_os = "ios")]
            let peripheral_options = Some(peripheral_manager_options());
            #[cfg(not(target_os = "ios"))]
            let peripheral_options: Option<
                Retained<NSDictionary<NSString, AnyObject>>,
            > = None;
            // SAFETY: the delegate and dispatch queue are retained for at least as long as the
            // manager, and every Objective-C argument has the framework-declared type.
            let _peripheral: Retained<CBPeripheralManager> = unsafe {
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
            let _hold = (central, central_delegate, peripheral_delegate, _peripheral);
        });

        let handles = handles_rx.await.map_err(|_| MacosBleError::Closed)?;
        let Handles {
            central,
            central_delegate,
            peripheral_delegate,
            queue,
        } = handles;

        loop {
            match tokio::time::timeout(POWER_ON_TIMEOUT, events_rx.recv()).await {
                Ok(Some(Event::Published { psm })) => {
                    let psm = Psm::new(psm).ok_or(MacosBleError::PublishFailed)?;
                    crate::diagnostic_log::debug!(
                        "bluetooth: powered on, advertising as Prns, L2CAP listener on PSM {:#06x}",
                        psm.get()
                    );
                    return Ok(Self {
                        _keepalive: keepalive,
                        events: events_rx,
                        psm,
                        seen: HashSet::new(),
                        central,
                        central_delegate,
                        peripheral_delegate,
                        peripherals,
                        restored,
                        dials: JoinSet::new(),
                        queue,
                    });
                }
                Ok(Some(Event::PublishFailed)) => {
                    crate::diagnostic_log::error!("bluetooth: L2CAP publish failed at startup");
                    return Err(MacosBleError::PublishFailed);
                }
                Ok(Some(_)) => continue,
                Ok(None) => return Err(MacosBleError::Closed),
                Err(_) => {
                    crate::diagnostic_log::error!(
                        "bluetooth: timed out waiting for power-on / L2CAP publish — is Bluetooth on and permission granted?"
                    );
                    return Err(MacosBleError::PowerOnTimeout);
                }
            }
        }
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

impl BleBackend<{ MacosBleBackend::MAX_PEERS }> for MacosBleBackend {
    type Error = MacosBleError;
    type Link = GattLink;

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), MacosBleError> {
        self.peripheral_delegate.0.set_advertising(mode);
        Ok(())
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), MacosBleError> {
        let enabled = mode.is_on();
        let central = SendCentralManager(self.central.0.clone());
        self.queue.exec_async(move || {
            let central = central;
            // SAFETY: the retained central manager is only messaged on its serial dispatch queue.
            unsafe { central.0.stopScan() };
            if enabled {
                start_scan(&central.0);
                crate::diagnostic_log::debug!("bluetooth: scanning for Prns peers");
            } else {
                crate::diagnostic_log::debug!(
                    "bluetooth: scanning stopped — at connection capacity"
                );
            }
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
        let (data_inbound_tx, data_inbound_rx) = tokio_mpsc::channel::<Box<[u8]>>(16);
        let command = DialCommand {
            central: self.central.0.clone(),
            delegate: self.central_delegate.0.clone(),
            peripheral: peripheral.clone(),
            peer_id,
            session: CentralPeerSession::new(address, control_tx, completion_tx, data_inbound_tx),
        };
        crate::diagnostic_log::debug!("bluetooth: dialing {token:02x?} over LE (central role)");
        self.queue.exec_async(move || {
            let command = command;
            // SAFETY: both retained Objective-C objects stay alive for the delegate assignment,
            // which runs on the CoreBluetooth serial dispatch queue.
            unsafe {
                command
                    .peripheral
                    .setDelegate(Some(ProtocolObject::from_ref(&*command.delegate)));
            }
            if !command
                .delegate
                .begin_session(command.peer_id, command.session)
            {
                return;
            }
            // SAFETY: the retained manager and peripheral are both owned by this queued command,
            // and CoreBluetooth connection calls are serialized on their dispatch queue.
            unsafe {
                command
                    .central
                    .connectPeripheral_options(&command.peripheral, None);
            }
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
                delegate.0.remove_session(peer_id);
                cancel_connection(&central, &peripheral);
            });
        }
        self.peripheral_delegate.0.clear_peer(address);
    }
}
