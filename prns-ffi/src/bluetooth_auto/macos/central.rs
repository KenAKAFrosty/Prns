use core::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_core_bluetooth::{
    CBCentralManager, CBCentralManagerDelegate, CBCentralManagerRestoredStatePeripheralsKey,
    CBCharacteristic, CBManagerState, CBPeripheral, CBPeripheralDelegate, CBPeripheralState,
    CBService,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString,
};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use prns_core::interfaces::bluetooth_auto::{BleAddress, BleIdentity, Control, PeerProtocol};

use super::{
    cbuuid_eq, columba_identity_uuid, columba_rx_uuid, columba_tx_uuid, control_uuid,
    core_bluetooth_peer_id, data_uuid, service_uuid, start_scan, CoreBluetoothPeerId, Event,
    PeripheralTable, RestoredPeripherals, SendCharacteristicRef, SendPeripheral,
};

pub(super) struct DialChars {
    pub(super) peer_protocol: PeerProtocol,
    pub(super) peer_identity: Option<BleIdentity>,
    pub(super) control: SendCharacteristicRef,
    pub(super) data: Option<SendCharacteristicRef>,
}

pub(super) enum DialCompletion {
    Ready(DialChars),
    Failed,
    Rejected,
}

enum ColumbaReadiness {
    AwaitingIdentityAndSubscription,
    Subscribed,
    Identified(BleIdentity),
}

enum CentralProfile {
    Discovering,
    Native {
        data: Option<SendCharacteristicRef>,
    },
    Columba {
        write: SendCharacteristicRef,
        notify: SendCharacteristicRef,
        readiness: ColumbaReadiness,
    },
    Ready,
}

pub(super) struct CentralPeerSession {
    address: BleAddress,
    control_tx: tokio_mpsc::Sender<Control>,
    completion_tx: Option<oneshot::Sender<DialCompletion>>,
    data_tx: tokio_mpsc::Sender<Box<[u8]>>,
    profile: CentralProfile,
}

impl CentralPeerSession {
    pub(super) fn new(
        address: BleAddress,
        control_tx: tokio_mpsc::Sender<Control>,
        completion_tx: oneshot::Sender<DialCompletion>,
        data_tx: tokio_mpsc::Sender<Box<[u8]>>,
    ) -> Self {
        Self {
            address,
            control_tx,
            completion_tx: Some(completion_tx),
            data_tx,
            profile: CentralProfile::Discovering,
        }
    }

    fn select_native(&mut self, data: Option<SendCharacteristicRef>) {
        self.profile = CentralProfile::Native { data };
    }

    fn select_columba(&mut self, write: SendCharacteristicRef, notify: SendCharacteristicRef) {
        self.profile = CentralProfile::Columba {
            write,
            notify,
            readiness: ColumbaReadiness::AwaitingIdentityAndSubscription,
        };
    }

    fn native_ready(&mut self, control: SendCharacteristicRef) {
        let profile = core::mem::replace(&mut self.profile, CentralProfile::Ready);
        let CentralProfile::Native { data } = profile else {
            self.profile = profile;
            return;
        };
        self.complete(DialChars {
            peer_protocol: PeerProtocol::Native,
            peer_identity: None,
            control,
            data,
        });
    }

    fn columba_identity(&mut self, identity: BleIdentity) {
        let profile = core::mem::replace(&mut self.profile, CentralProfile::Ready);
        let CentralProfile::Columba {
            write,
            notify,
            readiness,
        } = profile
        else {
            self.profile = profile;
            return;
        };
        match readiness {
            ColumbaReadiness::AwaitingIdentityAndSubscription | ColumbaReadiness::Identified(_) => {
                self.profile = CentralProfile::Columba {
                    write,
                    notify,
                    readiness: ColumbaReadiness::Identified(identity),
                };
            }
            ColumbaReadiness::Subscribed => {
                self.complete_columba(write, notify, identity);
            }
        }
    }

    fn columba_subscribed(&mut self) {
        let profile = core::mem::replace(&mut self.profile, CentralProfile::Ready);
        let CentralProfile::Columba {
            write,
            notify,
            readiness,
        } = profile
        else {
            self.profile = profile;
            return;
        };
        match readiness {
            ColumbaReadiness::AwaitingIdentityAndSubscription | ColumbaReadiness::Subscribed => {
                self.profile = CentralProfile::Columba {
                    write,
                    notify,
                    readiness: ColumbaReadiness::Subscribed,
                };
            }
            ColumbaReadiness::Identified(identity) => {
                self.complete_columba(write, notify, identity);
            }
        }
    }

    fn complete_columba(
        &mut self,
        write: SendCharacteristicRef,
        _notify: SendCharacteristicRef,
        identity: BleIdentity,
    ) {
        self.complete(DialChars {
            peer_protocol: PeerProtocol::Columba,
            peer_identity: Some(identity),
            data: Some(SendCharacteristicRef(write.0.clone())),
            control: write,
        });
    }

    fn complete(&mut self, chars: DialChars) {
        self.profile = CentralProfile::Ready;
        if let Some(completion_tx) = self.completion_tx.take() {
            let _ = completion_tx.send(DialCompletion::Ready(chars));
        }
    }

    fn fail(mut self) {
        if let Some(completion_tx) = self.completion_tx.take() {
            let _ = completion_tx.send(DialCompletion::Failed);
        }
    }

    fn reject(mut self) {
        if let Some(completion_tx) = self.completion_tx.take() {
            let _ = completion_tx.send(DialCompletion::Rejected);
        }
    }
}

pub(super) struct DialCommand {
    pub(super) central: Retained<CBCentralManager>,
    pub(super) delegate: Retained<CentralDelegate>,
    pub(super) peripheral: Retained<CBPeripheral>,
    pub(super) peer_id: CoreBluetoothPeerId,
    pub(super) session: CentralPeerSession,
}
// SAFETY: every retained CoreBluetooth object in the command is transferred to and consumed on
// the single serial CoreBluetooth dispatch queue; the embedded session is Send.
unsafe impl Send for DialCommand {}

pub(super) struct CentralDelegateIvars {
    events: tokio_mpsc::UnboundedSender<Event>,
    peripherals: PeripheralTable,
    restored: RestoredPeripherals,
    sessions: RefCell<HashMap<CoreBluetoothPeerId, CentralPeerSession>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = CentralDelegateIvars]
    pub(super) struct CentralDelegate;

    unsafe impl NSObjectProtocol for CentralDelegate {}

    unsafe impl CBCentralManagerDelegate for CentralDelegate {
        #[unsafe(method(centralManagerDidUpdateState:))]
        fn did_update_state(&self, central: &CBCentralManager) {
            // SAFETY: CoreBluetooth supplied this live manager to its delegate on the configured
            // serial dispatch queue.
            if unsafe { central.state() } == CBManagerState::PoweredOn {
                let _ = self.ivars().events.send(Event::Powered);
                start_scan(central);
            }
        }

        #[unsafe(method(centralManager:willRestoreState:))]
        fn will_restore_state(
            &self,
            _central: &CBCentralManager,
            dict: &NSDictionary<NSString, AnyObject>,
        ) {
            // SAFETY: CoreBluetooth exports this NSString constant with process lifetime.
            let key: &NSString = unsafe { CBCentralManagerRestoredStatePeripheralsKey };
            let Some(restored) = dict.objectForKey(key) else {
                return;
            };
            // SAFETY: CoreBluetooth documents this restoration dictionary value as an NSArray of
            // CBPeripheral objects; `restored` retains the array for the duration of the borrow.
            let peripherals: &NSArray<CBPeripheral> =
                unsafe { &*(Retained::as_ptr(&restored) as *const NSArray<CBPeripheral>) };
            for peripheral in peripherals.iter() {
                let peer_id = core_bluetooth_peer_id(&peripheral);
                let address = peer_id.address();
                crate::diagnostic_log::debug!(
                    "bluetooth: restored peripheral {:02x?} from a background relaunch — re-adopting",
                    address.octets()
                );
                if let Ok(mut map) = self.ivars().peripherals.lock() {
                    map.insert(peer_id, (SendPeripheral(peripheral.retain()), None));
                }
                if let Ok(mut queue) = self.ivars().restored.lock() {
                    queue.push_back(peer_id);
                }
            }
        }

        #[unsafe(method(centralManager:didDiscoverPeripheral:advertisementData:RSSI:))]
        fn did_discover(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            _advertisement_data: &NSDictionary<NSString, AnyObject>,
            rssi: &NSNumber,
        ) {
            // SAFETY: CoreBluetooth supplied this live peripheral to its delegate on the manager
            // queue, so querying immutable framework state is valid.
            if unsafe { peripheral.state() } != CBPeripheralState::Disconnected {
                return;
            }
            let dbm = rssi.integerValue();
            let rssi = if dbm == 127 {
                None
            } else {
                i8::try_from(dbm).ok()
            };
            let peer_id = core_bluetooth_peer_id(peripheral);
            let address = peer_id.address();
            if let Ok(mut map) = self.ivars().peripherals.lock() {
                map.insert(peer_id, (SendPeripheral(peripheral.retain()), rssi));
            }
            let _ = self.ivars().events.send(Event::Sighting { address, rssi });
        }

        #[unsafe(method(centralManager:didConnectPeripheral:))]
        fn did_connect(&self, _central: &CBCentralManager, peripheral: &CBPeripheral) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            if !self.ivars().sessions.borrow().contains_key(&peer_id) {
                return;
            }
            crate::diagnostic_log::debug!(
                "bluetooth: dial connected over LE, discovering Prns service"
            );
            let uuid = service_uuid();
            let services = NSArray::from_slice(&[&*uuid]);
            // SAFETY: `peripheral` is live for this callback and `services` is a correctly typed,
            // retained NSArray for the duration of the Objective-C message.
            unsafe { peripheral.discoverServices(Some(&services)) };
        }

        #[unsafe(method(centralManager:didFailToConnectPeripheral:error:))]
        fn did_fail_to_connect(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            crate::diagnostic_log::warn!("bluetooth: dial connect FAILED: {error:?}");
            self.fail_peer(core_bluetooth_peer_id(peripheral));
        }

        #[unsafe(method(centralManager:didDisconnectPeripheral:error:))]
        fn did_disconnect(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            crate::diagnostic_log::warn!("bluetooth: dialed peripheral disconnected: {error:?}");
            self.fail_peer(core_bluetooth_peer_id(peripheral));
        }
    }

    unsafe impl CBPeripheralDelegate for CentralDelegate {
        #[unsafe(method(peripheral:didDiscoverServices:))]
        fn did_discover_services(&self, peripheral: &CBPeripheral, error: Option<&NSError>) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            if let Some(error) = error {
                crate::diagnostic_log::warn!("bluetooth: service discovery FAILED: {error:?}");
                self.fail_peer(peer_id);
                return;
            }
            // SAFETY: the callback occurs only after CoreBluetooth completed service discovery;
            // the peripheral retains the returned service array.
            let service = unsafe { peripheral.services() }.and_then(|s| s.iter().next());
            let Some(service) = service else {
                crate::diagnostic_log::warn!(
                    "bluetooth: no Prns service on peripheral — dropping dial"
                );
                self.fail_peer(peer_id);
                return;
            };
            // SAFETY: `service` belongs to this live peripheral and both are retained throughout
            // the discovery message.
            unsafe { peripheral.discoverCharacteristics_forService(None, &service) };
        }

        #[unsafe(method(peripheral:didDiscoverCharacteristicsForService:error:))]
        fn did_discover_characteristics(
            &self,
            peripheral: &CBPeripheral,
            service: &CBService,
            error: Option<&NSError>,
        ) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            if let Some(error) = error {
                crate::diagnostic_log::warn!(
                    "bluetooth: characteristic discovery FAILED: {error:?}"
                );
                self.fail_peer(peer_id);
                return;
            }
            // SAFETY: this delegate callback follows characteristic discovery and CoreBluetooth
            // retains the characteristic collection on the live service.
            let Some(characteristics) = (unsafe { service.characteristics() }) else {
                crate::diagnostic_log::warn!(
                    "bluetooth: no characteristics on Prns service — dropping dial"
                );
                self.fail_peer(peer_id);
                return;
            };
            let control_id = control_uuid();
            let data_id = data_uuid();
            let columba_rx_id = columba_rx_uuid();
            let columba_tx_id = columba_tx_uuid();
            let columba_identity_id = columba_identity_uuid();
            let mut control = None;
            let mut data = None;
            let mut columba_rx = None;
            let mut columba_tx = None;
            let mut columba_identity = None;
            for characteristic in characteristics.iter() {
                // SAFETY: the characteristic is retained by the framework collection during this
                // iteration and UUID is a CoreBluetooth-owned immutable property.
                let uuid = unsafe { characteristic.UUID() };
                if cbuuid_eq(&uuid, &control_id) {
                    control = Some(characteristic);
                } else if cbuuid_eq(&uuid, &data_id) {
                    data = Some(characteristic);
                } else if cbuuid_eq(&uuid, &columba_rx_id) {
                    columba_rx = Some(characteristic);
                } else if cbuuid_eq(&uuid, &columba_tx_id) {
                    columba_tx = Some(characteristic);
                } else if cbuuid_eq(&uuid, &columba_identity_id) {
                    columba_identity = Some(characteristic);
                }
            }
            if let Some(control) = control {
                let data_ref = data
                    .as_ref()
                    .map(|data| SendCharacteristicRef(data.retain()));
                let Some(()) = self
                    .ivars()
                    .sessions
                    .borrow_mut()
                    .get_mut(&peer_id)
                    .map(|session| session.select_native(data_ref))
                else {
                    return;
                };
                if let Some(data) = data {
                    // SAFETY: this characteristic was discovered on `peripheral`; both remain live
                    // through the subscription message on the serial manager queue.
                    unsafe { peripheral.setNotifyValue_forCharacteristic(true, &data) };
                }
                crate::diagnostic_log::debug!(
                    "bluetooth: native control characteristic found, subscribing"
                );
                // SAFETY: `control` was discovered on `peripheral` and both objects are retained
                // throughout this queue-confined subscription call.
                unsafe { peripheral.setNotifyValue_forCharacteristic(true, &control) };
                return;
            }
            let (Some(rx), Some(tx), Some(identity)) = (columba_rx, columba_tx, columba_identity)
            else {
                crate::diagnostic_log::warn!(
                    "bluetooth: peer exposes neither a complete native nor Columba profile"
                );
                self.fail_peer(peer_id);
                return;
            };
            let Some(()) = self
                .ivars()
                .sessions
                .borrow_mut()
                .get_mut(&peer_id)
                .map(|session| {
                    session.select_columba(
                        SendCharacteristicRef(rx.retain()),
                        SendCharacteristicRef(tx.retain()),
                    );
                })
            else {
                return;
            };
            // SAFETY: the identity and transmit characteristics were discovered on this retained
            // peripheral, and both messages execute on its CoreBluetooth dispatch queue.
            unsafe {
                peripheral.readValueForCharacteristic(&identity);
                peripheral.setNotifyValue_forCharacteristic(true, &tx);
            }
        }

        #[unsafe(method(peripheral:didUpdateNotificationStateForCharacteristic:error:))]
        fn did_update_notification_state(
            &self,
            peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&NSError>,
        ) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            if let Some(error) = error {
                crate::diagnostic_log::warn!("bluetooth: subscribe FAILED: {error:?}");
                self.fail_peer(peer_id);
                return;
            }
            // SAFETY: CoreBluetooth supplied a live characteristic to this delegate callback.
            let subscribed_uuid = unsafe { characteristic.UUID() };
            if cbuuid_eq(&subscribed_uuid, &control_uuid()) {
                let mut sessions = self.ivars().sessions.borrow_mut();
                let Some(session) = sessions.get_mut(&peer_id) else {
                    return;
                };
                crate::diagnostic_log::debug!(
                    "bluetooth: {:02x?} subscribed — native control ready",
                    session.address.octets()
                );
                session.native_ready(SendCharacteristicRef(characteristic.retain()));
                return;
            }
            if !cbuuid_eq(&subscribed_uuid, &columba_tx_uuid()) {
                return;
            }
            let mut sessions = self.ivars().sessions.borrow_mut();
            let Some(session) = sessions.get_mut(&peer_id) else {
                return;
            };
            crate::diagnostic_log::debug!(
                "bluetooth: {:02x?} subscribed — Columba data path ready",
                session.address.octets()
            );
            session.columba_subscribed();
        }

        #[unsafe(method(peripheral:didUpdateValueForCharacteristic:error:))]
        fn did_update_value(
            &self,
            peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&NSError>,
        ) {
            let peer_id = core_bluetooth_peer_id(peripheral);
            if let Some(error) = error {
                crate::diagnostic_log::warn!("bluetooth: characteristic update FAILED: {error:?}");
                self.fail_peer(peer_id);
                return;
            }
            // SAFETY: CoreBluetooth supplied a live characteristic whose value is retained for the
            // duration of this value-update callback.
            let Some(value) = (unsafe { characteristic.value() }) else {
                return;
            };
            // SAFETY: CoreBluetooth supplied a live characteristic to this delegate callback.
            let updated_uuid = unsafe { characteristic.UUID() };
            if cbuuid_eq(&updated_uuid, &data_uuid())
                || cbuuid_eq(&updated_uuid, &columba_tx_uuid())
            {
                if let Some(session) = self.ivars().sessions.borrow().get(&peer_id) {
                    let _ = session.data_tx.try_send(Box::from(&value.to_vec()[..]));
                }
                return;
            }
            if cbuuid_eq(&updated_uuid, &columba_identity_uuid()) {
                let bytes = value.to_vec();
                let Ok(identity) = <[u8; 16]>::try_from(bytes.as_slice()) else {
                    self.fail_peer(peer_id);
                    return;
                };
                let mut sessions = self.ivars().sessions.borrow_mut();
                let Some(session) = sessions.get_mut(&peer_id) else {
                    return;
                };
                session.columba_identity(BleIdentity::new(identity));
                return;
            }
            let Some(control) = Control::decode(&value.to_vec()) else {
                return;
            };
            if let Some(session) = self.ivars().sessions.borrow().get(&peer_id) {
                let _ = session.control_tx.try_send(control);
            }
        }
    }
);

impl CentralDelegate {
    pub(super) fn new(
        events: tokio_mpsc::UnboundedSender<Event>,
        peripherals: PeripheralTable,
        restored: RestoredPeripherals,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(CentralDelegateIvars {
            events,
            peripherals,
            restored,
            sessions: RefCell::new(HashMap::new()),
        });
        // SAFETY: `this` is a freshly allocated CentralDelegate with fully initialized ivars;
        // forwarding to NSObject's designated initializer preserves its allocation identity.
        unsafe { msg_send![super(this), init] }
    }

    pub(super) fn begin_session(
        &self,
        peer_id: CoreBluetoothPeerId,
        session: CentralPeerSession,
    ) -> bool {
        if self.ivars().sessions.borrow().contains_key(&peer_id) {
            session.reject();
            return false;
        }
        self.ivars().sessions.borrow_mut().insert(peer_id, session);
        true
    }

    pub(super) fn remove_session(&self, peer_id: CoreBluetoothPeerId) {
        if let Some(session) = self.ivars().sessions.borrow_mut().remove(&peer_id) {
            session.fail();
        }
    }

    fn fail_peer(&self, peer_id: CoreBluetoothPeerId) {
        self.remove_session(peer_id);
    }
}
