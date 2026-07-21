use core::time::Duration;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc as sync_mpsc;
use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass};
use objc2_core_bluetooth::{CBCentralManager, CBPeripheralManager};
use objc2_foundation::{NSDictionary, NSString};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tokio::task::JoinSet;

use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, Origin, ScanningMode,
};
use prns_core::interfaces::bluetooth_auto::{BleAddress, BleIdentity, Control, Psm};

use super::central::{CentralDelegate, DialChars, DialCommand, DialSession};
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

#[cfg(target_os = "ios")]
const CENTRAL_RESTORE_IDENTIFIER: &str = "com.personal.prns.ble.central";
#[cfg(target_os = "ios")]
const PERIPHERAL_RESTORE_IDENTIFIER: &str = "com.personal.prns.ble.peripheral";
struct Handles {
    central: SendCentralManager,
    central_delegate: SendCentralDelegate,
    peripheral_delegate: SendPeripheralDelegate,
    queue: DispatchRetained<DispatchQueue>,
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
    dials: JoinSet<Option<(GattLink, Option<i8>)>>,
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
            if let Some(token) = self
                .restored
                .lock()
                .ok()
                .and_then(|mut queue| queue.pop_front())
            {
                return BleEvent::Sighting {
                    address: BleAddress::new(token),
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
                    if let Ok(Some((link, peer_rssi))) = done {
                        return BleEvent::LinkReady {
                            link,
                            origin: Origin::Dialed,
                            peer_rssi,
                        };
                    }
                }
            }
        }
    }

    async fn dial(&mut self, address: BleAddress) {
        let token = *address.octets();
        let Some((peripheral, peer_rssi)) = self
            .peripherals
            .lock()
            .ok()
            .and_then(|map| map.get(&token).map(|(p, rssi)| (p.0.clone(), *rssi)))
        else {
            crate::diagnostic_log::warn!(
                "bluetooth: dial to {token:02x?} — peripheral not yet sighted"
            );
            return;
        };
        let (control_tx, control_rx) = tokio_mpsc::channel::<Control>(8);
        let (result_tx, result_rx) = oneshot::channel::<DialChars>();
        let (data_inbound_tx, data_inbound_rx) = tokio_mpsc::channel::<Box<[u8]>>(16);
        let command = DialCommand {
            central: self.central.0.clone(),
            delegate: self.central_delegate.0.clone(),
            peripheral: peripheral.clone(),
            session: DialSession {
                address,
                control_tx,
                result_tx: Some(result_tx),
                data_tx: data_inbound_tx,
                data_char: None,
                peer_protocol: None,
                columba_write: None,
                columba_notify: None,
                peer_identity: None,
                columba_notify_ready: false,
            },
        };
        crate::diagnostic_log::debug!("bluetooth: dialing {token:02x?} over LE (central role)");
        self.queue.exec_async(move || {
            let command = command;
            unsafe {
                command
                    .peripheral
                    .setDelegate(Some(ProtocolObject::from_ref(&*command.delegate)));
            }
            *command.delegate.ivars().session.borrow_mut() = Some(command.session);
            unsafe {
                command
                    .central
                    .connectPeripheral_options(&command.peripheral, None);
            }
        });
        let send_peripheral = SendPeripheral(peripheral);
        let send_peripheral_manager = SendPeripheralDelegate(self.peripheral_delegate.0.clone());
        self.dials.spawn(async move {
            let chars = match tokio::time::timeout(DIAL_TIMEOUT, result_rx).await {
                Ok(Ok(chars)) => chars,
                _ => {
                    crate::diagnostic_log::warn!(
                        "bluetooth: dial to {token:02x?} did not reach control-ready"
                    );
                    return None;
                }
            };
            Some((
                GattLink {
                    peer_protocol: chars.peer_protocol,
                    peer_identity: chars.peer_identity,
                    control: ControlPlane::Central {
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
            ))
        });
    }

    async fn on_link_closed(&mut self, address: BleAddress) {
        let token = *address.octets();
        if let Some(peripheral) = self
            .peripherals
            .lock()
            .ok()
            .and_then(|map| map.get(&token).map(|(p, _)| p.0.clone()))
        {
            let peripheral = SendPeripheral(peripheral);
            let central = SendCentralManager(self.central.0.clone());
            let delegate = SendCentralDelegate(self.central_delegate.0.clone());
            self.queue.exec_async(move || {
                let central = central;
                let delegate = delegate;
                let peripheral = peripheral;
                delegate.0.clear_session();
                unsafe { central.0.cancelPeripheralConnection(&peripheral.0) };
            });
        }
        self.peripheral_delegate.0.clear_active(token);
    }
}
