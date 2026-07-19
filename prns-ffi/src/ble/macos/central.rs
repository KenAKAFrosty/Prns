use core::cell::RefCell;

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

use prns_core::interfaces::bluetooth_auto::core::{BleAddress, Control};

use super::{
    cbuuid_eq, control_uuid, data_uuid, service_uuid, start_scan, uuid_token, Event,
    PeripheralTable, RestoredPeripherals, SendCharacteristicRef, SendPeripheral,
};

pub(super) struct DialChars {
    pub(super) control: SendCharacteristicRef,
    pub(super) data: Option<SendCharacteristicRef>,
}

pub(super) struct DialSession {
    pub(super) address: BleAddress,
    pub(super) control_tx: tokio_mpsc::UnboundedSender<Control>,
    pub(super) result_tx: Option<oneshot::Sender<DialChars>>,
    pub(super) data_tx: tokio_mpsc::UnboundedSender<Box<[u8]>>,
    pub(super) data_char: Option<SendCharacteristicRef>,
}

pub(super) struct DialCommand {
    pub(super) central: Retained<CBCentralManager>,
    pub(super) delegate: Retained<CentralDelegate>,
    pub(super) peripheral: Retained<CBPeripheral>,
    pub(super) session: DialSession,
}
unsafe impl Send for DialCommand {}

pub(super) struct CentralDelegateIvars {
    events: tokio_mpsc::UnboundedSender<Event>,
    peripherals: PeripheralTable,
    restored: RestoredPeripherals,
    pub(super) session: RefCell<Option<DialSession>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = CentralDelegateIvars]
    pub(super) struct CentralDelegate;

    unsafe impl NSObjectProtocol for CentralDelegate {}

    unsafe impl CBCentralManagerDelegate for CentralDelegate {
        #[unsafe(method(centralManagerDidUpdateState:))]
        fn did_update_state(&self, central: &CBCentralManager) {
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
            let key: &NSString = unsafe { CBCentralManagerRestoredStatePeripheralsKey };
            let Some(restored) = dict.objectForKey(key) else {
                return;
            };
            let peripherals: &NSArray<CBPeripheral> =
                unsafe { &*(Retained::as_ptr(&restored) as *const NSArray<CBPeripheral>) };
            for peripheral in peripherals.iter() {
                let identifier = unsafe { peripheral.identifier() };
                let token = uuid_token(&identifier);
                crate::diagnostic_log::debug!(
                    "bluetooth: restored peripheral {token:02x?} from a background relaunch — re-adopting"
                );
                if let Ok(mut map) = self.ivars().peripherals.lock() {
                    map.insert(token, (SendPeripheral(peripheral.retain()), None));
                }
                if let Ok(mut queue) = self.ivars().restored.lock() {
                    queue.push_back(token);
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
            if unsafe { peripheral.state() } != CBPeripheralState::Disconnected {
                return;
            }
            let dbm = rssi.integerValue();
            let rssi = if dbm == 127 {
                None
            } else {
                i8::try_from(dbm).ok()
            };
            let identifier = unsafe { peripheral.identifier() };
            let token = uuid_token(&identifier);
            if let Ok(mut map) = self.ivars().peripherals.lock() {
                map.insert(token, (SendPeripheral(peripheral.retain()), rssi));
            }
            let _ = self.ivars().events.send(Event::Sighting {
                address: BleAddress::new(token),
                rssi,
            });
        }

        #[unsafe(method(centralManager:didConnectPeripheral:))]
        fn did_connect(&self, _central: &CBCentralManager, peripheral: &CBPeripheral) {
            crate::diagnostic_log::debug!(
                "bluetooth: dial connected over LE, discovering Prns service"
            );
            let uuid = service_uuid();
            let services = NSArray::from_slice(&[&*uuid]);
            unsafe { peripheral.discoverServices(Some(&services)) };
        }

        #[unsafe(method(centralManager:didFailToConnectPeripheral:error:))]
        fn did_fail_to_connect(
            &self,
            _central: &CBCentralManager,
            _peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            crate::diagnostic_log::warn!("bluetooth: dial connect FAILED: {error:?}");
            self.ivars().session.borrow_mut().take();
        }

        #[unsafe(method(centralManager:didDisconnectPeripheral:error:))]
        fn did_disconnect(
            &self,
            _central: &CBCentralManager,
            _peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            crate::diagnostic_log::warn!("bluetooth: dialed peripheral disconnected: {error:?}");
            self.ivars().session.borrow_mut().take();
        }
    }

    unsafe impl CBPeripheralDelegate for CentralDelegate {
        #[unsafe(method(peripheral:didDiscoverServices:))]
        fn did_discover_services(&self, peripheral: &CBPeripheral, error: Option<&NSError>) {
            if let Some(error) = error {
                crate::diagnostic_log::warn!("bluetooth: service discovery FAILED: {error:?}");
                self.ivars().session.borrow_mut().take();
                return;
            }
            let service = unsafe { peripheral.services() }.and_then(|s| s.iter().next());
            let Some(service) = service else {
                crate::diagnostic_log::warn!(
                    "bluetooth: no Prns service on peripheral — dropping dial"
                );
                self.ivars().session.borrow_mut().take();
                return;
            };
            unsafe { peripheral.discoverCharacteristics_forService(None, &service) };
        }

        #[unsafe(method(peripheral:didDiscoverCharacteristicsForService:error:))]
        fn did_discover_characteristics(
            &self,
            peripheral: &CBPeripheral,
            service: &CBService,
            error: Option<&NSError>,
        ) {
            if let Some(error) = error {
                crate::diagnostic_log::warn!(
                    "bluetooth: characteristic discovery FAILED: {error:?}"
                );
                self.ivars().session.borrow_mut().take();
                return;
            }
            let Some(characteristics) = (unsafe { service.characteristics() }) else {
                crate::diagnostic_log::warn!(
                    "bluetooth: no characteristics on Prns service — dropping dial"
                );
                self.ivars().session.borrow_mut().take();
                return;
            };
            let control_id = control_uuid();
            let data_id = data_uuid();
            let mut control = None;
            let mut data = None;
            for characteristic in characteristics.iter() {
                let uuid = unsafe { characteristic.UUID() };
                if cbuuid_eq(&uuid, &control_id) {
                    control = Some(characteristic);
                } else if cbuuid_eq(&uuid, &data_id) {
                    data = Some(characteristic);
                }
            }
            let Some(control) = control else {
                crate::diagnostic_log::warn!(
                    "bluetooth: no control characteristic — dropping dial"
                );
                self.ivars().session.borrow_mut().take();
                return;
            };
            if let Some(data) = &data {
                if let Some(session) = self.ivars().session.borrow_mut().as_mut() {
                    session.data_char = Some(SendCharacteristicRef(data.retain()));
                }
                unsafe { peripheral.setNotifyValue_forCharacteristic(true, data) };
            }
            crate::diagnostic_log::debug!("bluetooth: control characteristic found, subscribing");
            unsafe { peripheral.setNotifyValue_forCharacteristic(true, &control) };
        }

        #[unsafe(method(peripheral:didUpdateNotificationStateForCharacteristic:error:))]
        fn did_update_notification_state(
            &self,
            _peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&NSError>,
        ) {
            if let Some(error) = error {
                crate::diagnostic_log::warn!("bluetooth: subscribe FAILED: {error:?}");
                self.ivars().session.borrow_mut().take();
                return;
            }
            let subscribed_uuid = unsafe { characteristic.UUID() };
            if !cbuuid_eq(&subscribed_uuid, &control_uuid()) {
                return;
            }
            let mut session = self.ivars().session.borrow_mut();
            let Some(session) = session.as_mut() else {
                return;
            };
            if let Some(result_tx) = session.result_tx.take() {
                crate::diagnostic_log::debug!(
                    "bluetooth: {:02x?} subscribed — control ready, handshaking as dialer",
                    session.address.octets()
                );
                let _ = result_tx.send(DialChars {
                    control: SendCharacteristicRef(characteristic.retain()),
                    data: session.data_char.take(),
                });
            }
        }

        #[unsafe(method(peripheral:didUpdateValueForCharacteristic:error:))]
        fn did_update_value(
            &self,
            _peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            _error: Option<&NSError>,
        ) {
            let Some(value) = (unsafe { characteristic.value() }) else {
                return;
            };
            let updated_uuid = unsafe { characteristic.UUID() };
            if cbuuid_eq(&updated_uuid, &data_uuid()) {
                if let Some(session) = self.ivars().session.borrow().as_ref() {
                    let _ = session.data_tx.send(Box::from(&value.to_vec()[..]));
                }
                return;
            }
            let Some(control) = Control::decode(&value.to_vec()) else {
                return;
            };
            if let Some(session) = self.ivars().session.borrow().as_ref() {
                let _ = session.control_tx.send(control);
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
            session: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub(super) fn clear_session(&self) {
        self.ivars().session.borrow_mut().take();
    }
}
