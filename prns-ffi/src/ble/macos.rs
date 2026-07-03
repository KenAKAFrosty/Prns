#![allow(clippy::undocumented_unsafe_blocks)]

use core::cell::RefCell;
use core::ffi::c_void;
use core::ptr::NonNull;
use core::time::Duration;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc as sync_mpsc;
use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_core_bluetooth::{
    CBATTError, CBATTRequest, CBAdvertisementDataLocalNameKey, CBAdvertisementDataServiceUUIDsKey,
    CBAttributePermissions, CBCentral, CBCentralManager, CBCentralManagerDelegate,
    CBCentralManagerRestoredStatePeripheralsKey, CBCentralManagerScanOptionAllowDuplicatesKey,
    CBCharacteristic, CBCharacteristicProperties, CBCharacteristicWriteType, CBL2CAPChannel,
    CBManagerState, CBMutableCharacteristic, CBMutableService, CBPeripheral, CBPeripheralDelegate,
    CBPeripheralManager, CBPeripheralManagerDelegate, CBPeripheralManagerRestoredStateServicesKey,
    CBPeripheralState, CBService, CBUUID,
};
use objc2_core_foundation::{
    CFOptionFlags, CFReadStream, CFStreamClientContext, CFStreamEventType, CFWriteStream,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSError, NSInputStream, NSNumber, NSObject, NSObjectProtocol,
    NSOutputStream, NSString, NSUUID,
};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tokio::task::JoinSet;

use prns_core::interfaces::bluetooth_auto::core::{
    encode_stream_frame, fragments_of, BleAddress, BleUuid, Control, Dialect, Fragment, L2capPlan,
    Psm, Reassembler, StreamDeframer, BLE_HW_MTU, BLE_SERVICE_UUID, CONTROL_MAX_LEN,
    NATIVE_CONTROL_UUID, NATIVE_DATA_UUID, STREAM_FRAME_PREFIX_LEN,
};
use prns_core::interfaces::bluetooth_auto::limits;
use prns_core::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin,
};

const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;
const READ_CHUNK: usize = L2CAP_SDU_LEN;
const GATT_REASSEMBLY_CAP: usize = BLE_HW_MTU;
const GATT_FRAGMENT_PAYLOAD: usize = 180;
const GATT_FRAGMENT_BUF: usize = 256;
const POWER_ON_TIMEOUT: Duration = Duration::from_secs(10);
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(target_os = "ios")]
const CENTRAL_RESTORE_IDENTIFIER: &str = "com.personal.prns.ble.central";
#[cfg(target_os = "ios")]
const PERIPHERAL_RESTORE_IDENTIFIER: &str = "com.personal.prns.ble.peripheral";

const READ_EVENTS: CFOptionFlags = CFStreamEventType::HasBytesAvailable.0
    | CFStreamEventType::ErrorOccurred.0
    | CFStreamEventType::EndEncountered.0;
const WRITE_EVENTS: CFOptionFlags = CFStreamEventType::CanAcceptBytes.0
    | CFStreamEventType::ErrorOccurred.0
    | CFStreamEventType::EndEncountered.0;

type PeripheralTable = Arc<Mutex<HashMap<[u8; 6], (SendPeripheral, Option<i8>)>>>;
type RestoredPeripherals = Arc<Mutex<VecDeque<[u8; 6]>>>;

fn cbuuid(uuid: BleUuid) -> Retained<CBUUID> {
    match uuid {
        BleUuid::Bit128(bytes) => unsafe { CBUUID::UUIDWithData(&NSData::with_bytes(&bytes)) },
        BleUuid::Bit16(short) => unsafe {
            CBUUID::UUIDWithData(&NSData::with_bytes(&short.to_be_bytes()))
        },
    }
}

fn service_uuid() -> Retained<CBUUID> {
    cbuuid(BLE_SERVICE_UUID)
}

fn control_uuid() -> Retained<CBUUID> {
    cbuuid(NATIVE_CONTROL_UUID)
}

fn data_uuid() -> Retained<CBUUID> {
    cbuuid(NATIVE_DATA_UUID)
}

fn cbuuid_eq(a: &CBUUID, b: &CBUUID) -> bool {
    unsafe { a.data() }.to_vec() == unsafe { b.data() }.to_vec()
}

fn advertisement_data(services: &NSArray<CBUUID>) -> Retained<NSDictionary<NSString, AnyObject>> {
    let uuids_key: &NSString = unsafe { CBAdvertisementDataServiceUUIDsKey };
    let uuids_value: &AnyObject = services;
    let name_key: &NSString = unsafe { CBAdvertisementDataLocalNameKey };
    let name = NSString::from_str("Prns");
    let name_ref: &NSString = &name;
    let name_value: &AnyObject = name_ref;
    NSDictionary::from_slices(&[uuids_key, name_key], &[uuids_value, name_value])
}

fn scan_options() -> Retained<NSDictionary<NSString, AnyObject>> {
    let duplicates_key: &NSString = unsafe { CBCentralManagerScanOptionAllowDuplicatesKey };
    let duplicates = NSNumber::new_bool(true);
    let duplicates_value: &AnyObject = &duplicates;
    NSDictionary::from_slices(&[duplicates_key], &[duplicates_value])
}

#[cfg(target_os = "ios")]
fn central_manager_options() -> Retained<NSDictionary<NSString, AnyObject>> {
    use objc2_core_bluetooth::CBCentralManagerOptionRestoreIdentifierKey;
    let key: &NSString = unsafe { CBCentralManagerOptionRestoreIdentifierKey };
    let value = NSString::from_str(CENTRAL_RESTORE_IDENTIFIER);
    let value_ref: &NSString = &value;
    let value_obj: &AnyObject = value_ref;
    NSDictionary::from_slices(&[key], &[value_obj])
}

#[cfg(target_os = "ios")]
fn peripheral_manager_options() -> Retained<NSDictionary<NSString, AnyObject>> {
    use objc2_core_bluetooth::CBPeripheralManagerOptionRestoreIdentifierKey;
    let key: &NSString = unsafe { CBPeripheralManagerOptionRestoreIdentifierKey };
    let value = NSString::from_str(PERIPHERAL_RESTORE_IDENTIFIER);
    let value_ref: &NSString = &value;
    let value_obj: &AnyObject = value_ref;
    NSDictionary::from_slices(&[key], &[value_obj])
}

fn start_scan(central: &CBCentralManager) {
    let uuid = service_uuid();
    let services = NSArray::from_slice(&[&*uuid]);
    let options = scan_options();
    unsafe { central.scanForPeripheralsWithServices_options(Some(&services), Some(&options)) };
}

fn uuid_token(uuid: &NSUUID) -> [u8; 6] {
    let mut raw = [0u8; 16];
    unsafe {
        let _: () = msg_send![uuid, getUUIDBytes: raw.as_mut_ptr()];
    }
    let mut token = [0u8; 6];
    token.copy_from_slice(&raw[..6]);
    token
}

struct SendPeripheralManager(Retained<CBPeripheralManager>);
unsafe impl Send for SendPeripheralManager {}

struct SendCharacteristic(Retained<CBMutableCharacteristic>);
unsafe impl Send for SendCharacteristic {}

struct SendPeripheral(Retained<CBPeripheral>);
unsafe impl Send for SendPeripheral {}

struct SendCharacteristicRef(Retained<CBCharacteristic>);
unsafe impl Send for SendCharacteristicRef {}

struct SendCentralManager(Retained<CBCentralManager>);
unsafe impl Send for SendCentralManager {}

struct SendCentralDelegate(Retained<CentralDelegate>);
unsafe impl Send for SendCentralDelegate {}

struct SendPeripheralDelegate(Retained<PeripheralDelegate>);
unsafe impl Send for SendPeripheralDelegate {}

enum ControlPlane {
    Listener {
        manager: SendPeripheralManager,
        characteristic: SendCharacteristic,
        data_characteristic: SendCharacteristic,
        delegate: SendPeripheralDelegate,
    },
    Central {
        peripheral: SendPeripheral,
        characteristic: SendCharacteristicRef,
        data_characteristic: Option<SendCharacteristicRef>,
        peripheral_manager: SendPeripheralDelegate,
    },
}

enum GattWriter {
    Central {
        peripheral: SendPeripheral,
        characteristic: SendCharacteristicRef,
    },
    Listener {
        manager: SendPeripheralManager,
        characteristic: SendCharacteristic,
    },
}

impl GattWriter {
    fn send(&self, frame: &[u8]) -> Result<(), MacosBleError> {
        let mut buf = [0u8; GATT_FRAGMENT_BUF];
        for fragment in fragments_of(frame, GATT_FRAGMENT_PAYLOAD) {
            let len = fragment
                .encode(&mut buf)
                .ok_or(MacosBleError::FrameTooLarge)?;
            let data = NSData::with_bytes(&buf[..len]);
            match self {
                GattWriter::Central {
                    peripheral,
                    characteristic,
                } => unsafe {
                    peripheral.0.writeValue_forCharacteristic_type(
                        &data,
                        &characteristic.0,
                        CBCharacteristicWriteType::WithoutResponse,
                    );
                },
                GattWriter::Listener {
                    manager,
                    characteristic,
                } => {
                    let sent = unsafe {
                        manager
                            .0
                            .updateValue_forCharacteristic_onSubscribedCentrals(
                                &data,
                                &characteristic.0,
                                None,
                            )
                    };
                    if !sent {
                        log::warn!(
                            "bluetooth: GATT-data notify queue full — fragment dropped, peer will retransmit"
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

struct Outbound {
    pending: VecDeque<u8>,
    closed: bool,
}

struct StreamPump {
    input: Retained<NSInputStream>,
    output: Retained<NSOutputStream>,
    inbound_tx: RefCell<Option<tokio_mpsc::UnboundedSender<Box<[u8]>>>>,
    outbound: Arc<Mutex<Outbound>>,
    _channel: Retained<CBL2CAPChannel>,
}

#[derive(Clone, Copy)]
struct PumpPtr(*const StreamPump);
unsafe impl Send for PumpPtr {}

struct SendStreamPtr(*mut StreamPump);
unsafe impl Send for SendStreamPtr {}

struct PumpHandle {
    ptr: *mut StreamPump,
    queue: DispatchRetained<DispatchQueue>,
}
unsafe impl Send for PumpHandle {}
unsafe impl Sync for PumpHandle {}

impl Drop for PumpHandle {
    fn drop(&mut self) {
        let raw = SendStreamPtr(self.ptr);
        self.queue.exec_async(move || {
            let raw = raw;
            unsafe {
                let pump = &*raw.0;
                let cf_in = &*(Retained::as_ptr(&pump.input) as *const CFReadStream);
                let cf_out = &*(Retained::as_ptr(&pump.output) as *const CFWriteStream);
                cf_in.set_client(0, None, core::ptr::null_mut());
                cf_out.set_client(0, None, core::ptr::null_mut());
                pump.input.close();
                pump.output.close();
                drop(Box::from_raw(raw.0));
            }
        });
    }
}

fn flush(pump: &StreamPump) {
    let Ok(mut out) = pump.outbound.lock() else {
        return;
    };
    if out.closed {
        return;
    }
    while !out.pending.is_empty() && pump.output.hasSpaceAvailable() {
        let (ptr, len) = {
            let (front, _) = out.pending.as_slices();
            (front.as_ptr() as *mut u8, front.len())
        };
        let written = unsafe {
            pump.output
                .write_maxLength(NonNull::new_unchecked(ptr), len)
        };
        if written > 0 {
            out.pending.drain(..written as usize);
        } else {
            if written < 0 {
                log::warn!("bluetooth: L2CAP write returned {written} — data plane down");
                out.closed = true;
                pump.inbound_tx.borrow_mut().take();
            }
            break;
        }
    }
}

unsafe extern "C-unwind" fn read_cb(
    _stream: *mut CFReadStream,
    event: CFStreamEventType,
    info: *mut c_void,
) {
    let pump = unsafe { &*(info as *const StreamPump) };
    if (event.0 & CFStreamEventType::HasBytesAvailable.0) != 0 {
        let mut buf = [0u8; READ_CHUNK];
        while pump.input.hasBytesAvailable() {
            let read = unsafe {
                pump.input
                    .read_maxLength(NonNull::new_unchecked(buf.as_mut_ptr()), READ_CHUNK)
            };
            if read > 0 {
                if let Some(tx) = pump.inbound_tx.borrow().as_ref() {
                    let _ = tx.send(Box::from(&buf[..read as usize]));
                }
            } else {
                break;
            }
        }
    }
    if (event.0 & (CFStreamEventType::ErrorOccurred.0 | CFStreamEventType::EndEncountered.0)) != 0 {
        log::warn!("bluetooth: L2CAP read stream closed/errored — inbound data plane down");
        pump.inbound_tx.borrow_mut().take();
    }
}

unsafe extern "C-unwind" fn write_cb(
    _stream: *mut CFWriteStream,
    event: CFStreamEventType,
    info: *mut c_void,
) {
    let pump = unsafe { &*(info as *const StreamPump) };
    if (event.0 & CFStreamEventType::CanAcceptBytes.0) != 0 {
        flush(pump);
    }
    if (event.0 & (CFStreamEventType::ErrorOccurred.0 | CFStreamEventType::EndEncountered.0)) != 0 {
        log::warn!("bluetooth: L2CAP write stream closed/errored — outbound data plane down");
        if let Ok(mut out) = pump.outbound.lock() {
            out.closed = true;
        }
        pump.inbound_tx.borrow_mut().take();
    }
}

fn wire_l2cap(
    channel: &CBL2CAPChannel,
    queue: &DispatchRetained<DispatchQueue>,
) -> Option<DataPlane> {
    let input = unsafe { channel.inputStream() }?;
    let output = unsafe { channel.outputStream() }?;
    let (inbound_tx, inbound_rx) = tokio_mpsc::unbounded_channel::<Box<[u8]>>();
    let outbound = Arc::new(Mutex::new(Outbound {
        pending: VecDeque::new(),
        closed: false,
    }));
    let pump = Box::into_raw(Box::new(StreamPump {
        input,
        output,
        inbound_tx: RefCell::new(Some(inbound_tx)),
        outbound: outbound.clone(),
        _channel: channel.retain(),
    }));
    unsafe {
        let pump_ref = &*pump;
        let cf_in = &*(Retained::as_ptr(&pump_ref.input) as *const CFReadStream);
        let cf_out = &*(Retained::as_ptr(&pump_ref.output) as *const CFWriteStream);
        let mut ctx = CFStreamClientContext {
            version: 0,
            info: pump as *mut c_void,
            retain: None,
            release: None,
            copyDescription: None,
        };
        cf_in.set_client(READ_EVENTS, Some(read_cb), &mut ctx);
        cf_in.set_dispatch_queue(Some(&**queue));
        cf_out.set_client(WRITE_EVENTS, Some(write_cb), &mut ctx);
        cf_out.set_dispatch_queue(Some(&**queue));
        pump_ref.input.open();
        pump_ref.output.open();
    }
    Some(DataPlane {
        inbound_rx,
        outbound,
        queue: queue.clone(),
        pump_ptr: PumpPtr(pump),
        pump: Arc::new(PumpHandle {
            ptr: pump,
            queue: queue.clone(),
        }),
    })
}

struct DataPlane {
    inbound_rx: tokio_mpsc::UnboundedReceiver<Box<[u8]>>,
    outbound: Arc<Mutex<Outbound>>,
    queue: DispatchRetained<DispatchQueue>,
    pump_ptr: PumpPtr,
    pump: Arc<PumpHandle>,
}

const MAX_BUFFERED_L2CAP: usize = 4;

#[derive(Default)]
struct PendingL2cap {
    waiters: VecDeque<oneshot::Sender<DataPlane>>,
    ready: VecDeque<DataPlane>,
}

impl PendingL2cap {
    fn deliver(&mut self, mut data: DataPlane) {
        while let Some(tx) = self.waiters.pop_front() {
            match tx.send(data) {
                Ok(()) => return,
                Err(returned) => data = returned,
            }
        }
        self.ready.push_back(data);
        while self.ready.len() > MAX_BUFFERED_L2CAP {
            self.ready.pop_front();
        }
    }

    fn arm(&mut self, tx: oneshot::Sender<DataPlane>) {
        match self.ready.pop_front() {
            Some(data) => {
                let _ = tx.send(data);
            }
            None => self.waiters.push_back(tx),
        }
    }

    fn clear(&mut self) {
        self.waiters.clear();
        self.ready.clear();
    }
}

enum Event {
    Powered,
    Published {
        psm: u16,
    },
    PublishFailed,
    Sighting {
        address: BleAddress,
        rssi: Option<i8>,
    },
    Inbound(GattLink),
}

struct DialChars {
    control: SendCharacteristicRef,
    data: Option<SendCharacteristicRef>,
}

struct DialSession {
    address: BleAddress,
    control_tx: tokio_mpsc::UnboundedSender<Control>,
    result_tx: Option<oneshot::Sender<DialChars>>,
    data_tx: tokio_mpsc::UnboundedSender<Box<[u8]>>,
    data_char: Option<SendCharacteristicRef>,
}

struct DialCommand {
    central: Retained<CBCentralManager>,
    delegate: Retained<CentralDelegate>,
    peripheral: Retained<CBPeripheral>,
    session: DialSession,
}
unsafe impl Send for DialCommand {}

struct CentralDelegateIvars {
    events: tokio_mpsc::UnboundedSender<Event>,
    peripherals: PeripheralTable,
    restored: RestoredPeripherals,
    session: RefCell<Option<DialSession>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = CentralDelegateIvars]
    struct CentralDelegate;

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
                log::info!(
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
            log::debug!("bluetooth: dial connected over LE, discovering Prns service");
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
            log::warn!("bluetooth: dial connect FAILED: {error:?}");
            self.ivars().session.borrow_mut().take();
        }

        #[unsafe(method(centralManager:didDisconnectPeripheral:error:))]
        fn did_disconnect(
            &self,
            _central: &CBCentralManager,
            _peripheral: &CBPeripheral,
            error: Option<&NSError>,
        ) {
            log::warn!("bluetooth: dialed peripheral disconnected: {error:?}");
            self.ivars().session.borrow_mut().take();
        }
    }

    unsafe impl CBPeripheralDelegate for CentralDelegate {
        #[unsafe(method(peripheral:didDiscoverServices:))]
        fn did_discover_services(&self, peripheral: &CBPeripheral, error: Option<&NSError>) {
            if let Some(error) = error {
                log::warn!("bluetooth: service discovery FAILED: {error:?}");
                self.ivars().session.borrow_mut().take();
                return;
            }
            let service = unsafe { peripheral.services() }.and_then(|s| s.iter().next());
            let Some(service) = service else {
                log::warn!("bluetooth: no Prns service on peripheral — dropping dial");
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
                log::warn!("bluetooth: characteristic discovery FAILED: {error:?}");
                self.ivars().session.borrow_mut().take();
                return;
            }
            let Some(characteristics) = (unsafe { service.characteristics() }) else {
                log::warn!("bluetooth: no characteristics on Prns service — dropping dial");
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
                log::warn!("bluetooth: no control characteristic — dropping dial");
                self.ivars().session.borrow_mut().take();
                return;
            };
            if let Some(data) = &data {
                if let Some(session) = self.ivars().session.borrow_mut().as_mut() {
                    session.data_char = Some(SendCharacteristicRef(data.retain()));
                }
                unsafe { peripheral.setNotifyValue_forCharacteristic(true, data) };
            }
            log::debug!("bluetooth: control characteristic found, subscribing");
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
                log::warn!("bluetooth: subscribe FAILED: {error:?}");
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
                log::info!(
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
    fn new(
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

    fn clear_session(&self) {
        self.ivars().session.borrow_mut().take();
    }
}

struct PeripheralDelegateIvars {
    events: tokio_mpsc::UnboundedSender<Event>,
    characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    data_characteristic: RefCell<Retained<CBMutableCharacteristic>>,
    queue: DispatchRetained<DispatchQueue>,
    manager: RefCell<Option<SendPeripheralManager>>,
    service_published: RefCell<bool>,
    active: RefCell<Option<tokio_mpsc::UnboundedSender<Control>>>,
    active_address: RefCell<Option<[u8; 6]>>,
    data_inbound: RefCell<Option<tokio_mpsc::UnboundedSender<Box<[u8]>>>>,
    pending: RefCell<PendingL2cap>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = PeripheralDelegateIvars]
    struct PeripheralDelegate;

    unsafe impl NSObjectProtocol for PeripheralDelegate {}

    unsafe impl CBPeripheralManagerDelegate for PeripheralDelegate {
        #[unsafe(method(peripheralManagerDidUpdateState:))]
        fn did_update_state(&self, peripheral: &CBPeripheralManager) {
            if unsafe { peripheral.state() } == CBManagerState::PoweredOn {
                *self.ivars().manager.borrow_mut() =
                    Some(SendPeripheralManager(peripheral.retain()));
                if !*self.ivars().service_published.borrow() {
                    let control_ref = self.ivars().characteristic.borrow();
                    let data_ref = self.ivars().data_characteristic.borrow();
                    let control: &CBCharacteristic = &control_ref;
                    let data: &CBCharacteristic = &data_ref;
                    let characteristics = NSArray::from_slice(&[control, data]);
                    let service = unsafe {
                        CBMutableService::initWithType_primary(
                            CBMutableService::alloc(),
                            &service_uuid(),
                            true,
                        )
                    };
                    unsafe { service.setCharacteristics(Some(&characteristics)) };
                    unsafe { peripheral.addService(&service) };
                    *self.ivars().service_published.borrow_mut() = true;
                }
                unsafe { peripheral.publishL2CAPChannelWithEncryption(false) };
            }
        }

        #[unsafe(method(peripheralManager:willRestoreState:))]
        fn will_restore_state(
            &self,
            peripheral: &CBPeripheralManager,
            dict: &NSDictionary<NSString, AnyObject>,
        ) {
            *self.ivars().manager.borrow_mut() = Some(SendPeripheralManager(peripheral.retain()));
            let key: &NSString = unsafe { CBPeripheralManagerRestoredStateServicesKey };
            let Some(restored) = dict.objectForKey(key) else {
                return;
            };
            let services: &NSArray<CBService> =
                unsafe { &*(Retained::as_ptr(&restored) as *const NSArray<CBService>) };
            let control_id = control_uuid();
            let data_id = data_uuid();
            for service in services.iter() {
                let service_id = unsafe { service.UUID() };
                if !cbuuid_eq(&service_id, &service_uuid()) {
                    continue;
                }
                let Some(characteristics) = (unsafe { service.characteristics() }) else {
                    continue;
                };
                for characteristic in characteristics.iter() {
                    let uuid = unsafe { characteristic.UUID() };
                    let mutable: &CBMutableCharacteristic = unsafe {
                        &*(Retained::as_ptr(&characteristic) as *const CBMutableCharacteristic)
                    };
                    if cbuuid_eq(&uuid, &control_id) {
                        *self.ivars().characteristic.borrow_mut() = mutable.retain();
                    } else if cbuuid_eq(&uuid, &data_id) {
                        *self.ivars().data_characteristic.borrow_mut() = mutable.retain();
                    }
                }
                *self.ivars().service_published.borrow_mut() = true;
                log::info!(
                    "bluetooth: restored the published Prns GATT service from a background relaunch"
                );
            }
        }

        #[unsafe(method(peripheralManager:didAddService:error:))]
        fn did_add_service(
            &self,
            peripheral: &CBPeripheralManager,
            _service: &CBService,
            error: Option<&NSError>,
        ) {
            if let Some(error) = error {
                log::error!("bluetooth: GATT service add FAILED: {error:?}");
                return;
            }
            log::debug!(
                "bluetooth: GATT service added (control characteristic live), starting advertising"
            );
            let uuid = service_uuid();
            let services = NSArray::from_slice(&[&*uuid]);
            let data = advertisement_data(&services);
            unsafe { peripheral.startAdvertising(Some(&data)) };
        }

        #[unsafe(method(peripheralManagerDidStartAdvertising:error:))]
        fn did_start_advertising(
            &self,
            _peripheral: &CBPeripheralManager,
            error: Option<&NSError>,
        ) {
            if let Some(error) = error {
                log::error!("bluetooth: advertising FAILED to start: {error:?}");
            } else {
                log::info!(
                    "bluetooth: advertising started — discoverable as Prns, service UUID in the BlueZ-visible packet"
                );
            }
        }

        #[unsafe(method(peripheralManager:didPublishL2CAPChannel:error:))]
        fn did_publish_l2cap(
            &self,
            _peripheral: &CBPeripheralManager,
            psm: u16,
            error: Option<&NSError>,
        ) {
            if let Some(error) = error {
                log::error!("bluetooth: L2CAP publish FAILED: {error:?}");
                let _ = self.ivars().events.send(Event::PublishFailed);
            } else {
                log::info!("bluetooth: published L2CAP channel, PSM {psm:#06x}");
                let _ = self.ivars().events.send(Event::Published { psm });
            }
        }

        #[unsafe(method(peripheralManager:didOpenL2CAPChannel:error:))]
        fn did_open_l2cap(
            &self,
            _peripheral: &CBPeripheralManager,
            channel: Option<&CBL2CAPChannel>,
            error: Option<&NSError>,
        ) {
            if let Some(error) = error {
                log::warn!("bluetooth: L2CAP channel open FAILED: {error:?}");
            }
            let Some(channel) = channel else {
                log::warn!(
                    "bluetooth: L2CAP open callback with no channel — data plane not established"
                );
                return;
            };
            let Some(data) = wire_l2cap(channel, &self.ivars().queue) else {
                log::warn!("bluetooth: L2CAP channel exposes no streams — dropping");
                return;
            };
            log::info!("bluetooth: L2CAP channel opened, data plane up");
            self.ivars().pending.borrow_mut().deliver(data);
        }

        #[unsafe(method(peripheralManager:didReceiveWriteRequests:))]
        fn did_receive_write_requests(
            &self,
            peripheral: &CBPeripheralManager,
            requests: &NSArray<CBATTRequest>,
        ) {
            for request in requests.iter() {
                let Some(value) = (unsafe { request.value() }) else {
                    unsafe {
                        peripheral.respondToRequest_withResult(&request, CBATTError::Success)
                    };
                    continue;
                };
                let characteristic = unsafe { request.characteristic() };
                let written_uuid = unsafe { characteristic.UUID() };
                if cbuuid_eq(&written_uuid, &data_uuid()) {
                    if let Some(tx) = self.ivars().data_inbound.borrow().as_ref() {
                        let _ = tx.send(Box::from(&value.to_vec()[..]));
                    }
                    unsafe {
                        peripheral.respondToRequest_withResult(&request, CBATTError::Success)
                    };
                    continue;
                }
                if let Some(control) = Control::decode(&value.to_vec()) {
                    let mut active = self.ivars().active.borrow_mut();
                    if active.is_none() {
                        let (tx, rx) = tokio_mpsc::unbounded_channel::<Control>();
                        let (data_tx, data_rx) = tokio_mpsc::unbounded_channel::<Box<[u8]>>();
                        let central = unsafe { request.central() };
                        let identifier = unsafe { central.identifier() };
                        let address = BleAddress::new(uuid_token(&identifier));
                        *self.ivars().active_address.borrow_mut() = Some(*address.octets());
                        log::info!(
                            "bluetooth: inbound central {:02x?} — control link opened, handshaking",
                            address.octets()
                        );
                        let link = GattLink {
                            control: ControlPlane::Listener {
                                manager: SendPeripheralManager(peripheral.retain()),
                                characteristic: SendCharacteristic(
                                    self.ivars().characteristic.borrow().clone(),
                                ),
                                data_characteristic: SendCharacteristic(
                                    self.ivars().data_characteristic.borrow().clone(),
                                ),
                                delegate: SendPeripheralDelegate(self.retain()),
                            },
                            control_rx: rx,
                            address,
                            data_inbound_rx: Some(data_rx),
                            l2cap_pending: None,
                        };
                        let _ = self.ivars().events.send(Event::Inbound(link));
                        *active = Some(tx);
                        *self.ivars().data_inbound.borrow_mut() = Some(data_tx);
                    }
                    if let Some(tx) = active.as_ref() {
                        let _ = tx.send(control);
                    }
                }
                unsafe { peripheral.respondToRequest_withResult(&request, CBATTError::Success) };
            }
        }

        #[unsafe(method(peripheralManager:central:didSubscribeToCharacteristic:))]
        fn did_subscribe(
            &self,
            _peripheral: &CBPeripheralManager,
            central: &CBCentral,
            _characteristic: &CBCharacteristic,
        ) {
            let identifier = unsafe { central.identifier() };
            log::info!(
                "bluetooth: central {:02x?} subscribed to control characteristic — GATT connected, awaiting Hello",
                uuid_token(&identifier)
            );
        }

        #[unsafe(method(peripheralManager:central:didUnsubscribeFromCharacteristic:))]
        fn did_unsubscribe(
            &self,
            _peripheral: &CBPeripheralManager,
            central: &CBCentral,
            _characteristic: &CBCharacteristic,
        ) {
            let identifier = unsafe { central.identifier() };
            log::debug!(
                "bluetooth: central {:02x?} unsubscribed — clearing listener slot so the next central can re-accept",
                uuid_token(&identifier)
            );
            let token = uuid_token(&identifier);
            if self
                .ivars()
                .active_address
                .borrow()
                .is_none_or(|active| active == token)
            {
                self.ivars().active.borrow_mut().take();
                self.ivars().active_address.borrow_mut().take();
                self.ivars().data_inbound.borrow_mut().take();
                self.ivars().pending.borrow_mut().clear();
            }
        }

        #[unsafe(method(peripheralManagerIsReadyToUpdateSubscribers:))]
        fn is_ready_to_update(&self, _peripheral: &CBPeripheralManager) {
            log::debug!("bluetooth: notify queue drained — ready to update subscribers");
        }
    }
);

impl PeripheralDelegate {
    fn new(
        events: tokio_mpsc::UnboundedSender<Event>,
        queue: DispatchRetained<DispatchQueue>,
    ) -> Retained<Self> {
        let data_plane_properties = CBCharacteristicProperties::Write
            | CBCharacteristicProperties::WriteWithoutResponse
            | CBCharacteristicProperties::Notify;
        let characteristic = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &control_uuid(),
                data_plane_properties,
                None,
                CBAttributePermissions::Writeable,
            )
        };
        let data_characteristic = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &data_uuid(),
                data_plane_properties,
                None,
                CBAttributePermissions::Writeable,
            )
        };
        let this = Self::alloc().set_ivars(PeripheralDelegateIvars {
            events,
            characteristic: RefCell::new(characteristic),
            data_characteristic: RefCell::new(data_characteristic),
            queue,
            manager: RefCell::new(None),
            service_published: RefCell::new(false),
            active: RefCell::new(None),
            active_address: RefCell::new(None),
            data_inbound: RefCell::new(None),
            pending: RefCell::new(PendingL2cap::default()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn arm_pending_channel(&self, tx: oneshot::Sender<DataPlane>) {
        let queue = self.ivars().queue.clone();
        let this = SendPeripheralDelegate(self.retain());
        queue.exec_async(move || {
            let this = this;
            this.0.ivars().pending.borrow_mut().arm(tx);
        });
    }

    fn set_advertising(&self, enabled: bool) {
        let queue = self.ivars().queue.clone();
        let this = SendPeripheralDelegate(self.retain());
        queue.exec_async(move || {
            let this = this;
            let Some(manager) = this
                .0
                .ivars()
                .manager
                .borrow()
                .as_ref()
                .map(|m| m.0.clone())
            else {
                return;
            };
            if enabled {
                let uuid = service_uuid();
                let services = NSArray::from_slice(&[&*uuid]);
                let data = advertisement_data(&services);
                unsafe { manager.startAdvertising(Some(&data)) };
            } else {
                unsafe { manager.stopAdvertising() };
                log::info!("bluetooth: advertising stopped — at connection capacity");
            }
        });
    }

    fn clear_active(&self, address: [u8; 6]) {
        let queue = self.ivars().queue.clone();
        let this = SendPeripheralDelegate(self.retain());
        queue.exec_async(move || {
            let this = this;
            if this
                .0
                .ivars()
                .active_address
                .borrow()
                .is_some_and(|active| active == address)
            {
                this.0.ivars().active.borrow_mut().take();
                this.0.ivars().active_address.borrow_mut().take();
                this.0.ivars().data_inbound.borrow_mut().take();
                this.0.ivars().pending.borrow_mut().clear();
            }
        });
    }
}

pub struct GattLink {
    control: ControlPlane,
    control_rx: tokio_mpsc::UnboundedReceiver<Control>,
    address: BleAddress,
    data_inbound_rx: Option<tokio_mpsc::UnboundedReceiver<Box<[u8]>>>,
    l2cap_pending: Option<oneshot::Receiver<DataPlane>>,
}

impl BleLink for GattLink {
    type Error = MacosBleError;
    type Source = GattSource;
    type Sink = GattSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        self.address
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), MacosBleError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg.encode(&mut buf).ok_or(MacosBleError::ControlTooLarge)?;
        let data = NSData::with_bytes(&buf[..len]);
        match &self.control {
            ControlPlane::Listener {
                manager,
                characteristic,
                ..
            } => {
                let sent = unsafe {
                    manager
                        .0
                        .updateValue_forCharacteristic_onSubscribedCentrals(
                            &data,
                            &characteristic.0,
                            None,
                        )
                };
                if sent {
                    log::debug!("bluetooth: {:02x?} -> {msg:?}", self.address.octets());
                    Ok(())
                } else {
                    log::warn!(
                        "bluetooth: {:02x?} notify failed — control PDU did not reach the central, handshake will stall",
                        self.address.octets()
                    );
                    Err(MacosBleError::NotifyFailed)
                }
            }
            ControlPlane::Central {
                peripheral,
                characteristic,
                ..
            } => {
                let max = unsafe {
                    peripheral
                        .0
                        .maximumWriteValueLengthForType(CBCharacteristicWriteType::WithResponse)
                };
                if max < len {
                    log::warn!(
                        "bluetooth: {:02x?} control write {len}B exceeds max single write {max}B (negotiated ATT MTU is small) — CoreBluetooth will use a long/prepared write; the peer GATT server must reassemble it",
                        self.address.octets()
                    );
                } else {
                    log::debug!(
                        "bluetooth: {:02x?} control write {len}B fits one ATT packet (max {max}B)",
                        self.address.octets()
                    );
                }
                unsafe {
                    peripheral.0.writeValue_forCharacteristic_type(
                        &data,
                        &characteristic.0,
                        CBCharacteristicWriteType::WithResponse,
                    )
                };
                log::debug!("bluetooth: {:02x?} -> {msg:?}", self.address.octets());
                Ok(())
            }
        }
    }

    async fn control_recv(&mut self) -> Result<Control, MacosBleError> {
        let control = self.control_rx.recv().await.ok_or(MacosBleError::Closed)?;
        log::debug!("bluetooth: {:02x?} <- {control:?}", self.address.octets());
        Ok(control)
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), MacosBleError> {
        match plan {
            L2capPlan::Accept => {
                let (tx, rx) = oneshot::channel::<DataPlane>();
                match &self.control {
                    ControlPlane::Central {
                        peripheral_manager, ..
                    } => peripheral_manager.0.arm_pending_channel(tx),
                    ControlPlane::Listener { delegate, .. } => delegate.0.arm_pending_channel(tx),
                };
                self.l2cap_pending = Some(rx);
                log::debug!(
                    "bluetooth: {:02x?} armed the L2CAP acceptor — the peer's CoC will upgrade the live GATT-floor link in the background",
                    self.address.octets()
                );
                Ok(())
            }
            L2capPlan::Open { .. } => {
                log::warn!(
                    "bluetooth: {:02x?} asked to open a CoC, but the macOS backend is acceptor-only (a central-side open bonds) — staying on the GATT floor",
                    self.address.octets()
                );
                Ok(())
            }
            L2capPlan::None => Ok(()),
        }
    }

    fn into_data(self) -> (GattSource, GattSink) {
        let (merged_tx, merged_rx) = tokio_mpsc::unbounded_channel::<Box<[u8]>>();

        if let Some(mut inbound_rx) = self.data_inbound_rx {
            let frames = merged_tx.clone();
            tokio::spawn(async move {
                let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
                while let Some(message) = inbound_rx.recv().await {
                    let Some(fragment) = Fragment::decode(&message) else {
                        continue;
                    };
                    if let Some(frame) = reassembler.absorb(&fragment) {
                        if frames.send(Box::from(frame)).is_err() {
                            break;
                        }
                    }
                }
            });
        }

        let l2cap_pending = self.l2cap_pending.map(|pending| {
            let (write_tx, write_rx) = oneshot::channel::<L2capWriteHalf>();
            let frames = merged_tx.clone();
            tokio::spawn(async move {
                let Ok(data) = pending.await else {
                    return;
                };
                log::info!("bluetooth: L2CAP fast lane up — data now rides the channel, GATT stays the floor");
                let DataPlane {
                    mut inbound_rx,
                    outbound,
                    queue,
                    pump_ptr,
                    pump,
                } = data;
                let _ = write_tx.send(L2capWriteHalf {
                    outbound,
                    queue,
                    pump_ptr,
                    _pump: pump.clone(),
                });
                let _read_pump = pump;
                let mut deframer = StreamDeframer::<{ 2 * L2CAP_SDU_LEN }>::new();
                let mut frame = std::vec![0u8; 2 * L2CAP_SDU_LEN];
                while let Some(chunk) = inbound_rx.recv().await {
                    if !deframer.absorb(&chunk) {
                        break;
                    }
                    while let Some(len) = deframer.next_frame(&mut frame) {
                        if frames.send(Box::from(&frame[..len])).is_err() {
                            return;
                        }
                    }
                }
            });
            write_rx
        });

        drop(merged_tx);
        (
            GattSource { inbound: merged_rx },
            GattSink {
                gatt: gatt_writer(&self.control),
                l2cap: None,
                l2cap_pending,
            },
        )
    }
}

fn gatt_writer(control: &ControlPlane) -> Option<GattWriter> {
    match control {
        ControlPlane::Central {
            peripheral,
            data_characteristic: Some(data_characteristic),
            ..
        } => Some(GattWriter::Central {
            peripheral: SendPeripheral(peripheral.0.clone()),
            characteristic: SendCharacteristicRef(data_characteristic.0.clone()),
        }),
        ControlPlane::Central {
            data_characteristic: None,
            ..
        } => None,
        ControlPlane::Listener {
            manager,
            data_characteristic,
            ..
        } => Some(GattWriter::Listener {
            manager: SendPeripheralManager(manager.0.clone()),
            characteristic: SendCharacteristic(data_characteristic.0.clone()),
        }),
    }
}

pub struct GattSource {
    inbound: tokio_mpsc::UnboundedReceiver<Box<[u8]>>,
}

impl BleSource for GattSource {
    type Error = MacosBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, MacosBleError> {
        let frame = self.inbound.recv().await.ok_or(MacosBleError::Closed)?;
        let len = frame.len().min(out.len());
        out[..len].copy_from_slice(&frame[..len]);
        Ok(len)
    }
}

struct L2capWriteHalf {
    outbound: Arc<Mutex<Outbound>>,
    queue: DispatchRetained<DispatchQueue>,
    pump_ptr: PumpPtr,
    _pump: Arc<PumpHandle>,
}

impl L2capWriteHalf {
    fn send(&self, frame: &[u8]) -> Result<(), MacosBleError> {
        let mut framed = [0u8; L2CAP_SDU_LEN];
        let len = encode_stream_frame(frame, &mut framed).ok_or(MacosBleError::FrameTooLarge)?;
        {
            let Ok(mut out) = self.outbound.lock() else {
                return Err(MacosBleError::Closed);
            };
            if out.closed {
                return Err(MacosBleError::Closed);
            }
            out.pending.extend(framed[..len].iter().copied());
        }
        let ptr = self.pump_ptr;
        self.queue.exec_async(move || {
            let ptr = ptr;
            flush(unsafe { &*ptr.0 });
        });
        Ok(())
    }
}

pub struct GattSink {
    gatt: Option<GattWriter>,
    l2cap: Option<L2capWriteHalf>,
    l2cap_pending: Option<oneshot::Receiver<L2capWriteHalf>>,
}

impl BleSink for GattSink {
    type Error = MacosBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), MacosBleError> {
        if self.l2cap.is_none() {
            if let Some(pending) = self.l2cap_pending.as_mut() {
                match pending.try_recv() {
                    Ok(half) => {
                        self.l2cap = Some(half);
                        self.l2cap_pending = None;
                    }
                    Err(oneshot::error::TryRecvError::Closed) => self.l2cap_pending = None,
                    Err(oneshot::error::TryRecvError::Empty) => {}
                }
            }
        }
        if let Some(l2cap) = &self.l2cap {
            match l2cap.send(frame) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    self.l2cap = None;
                    if self.gatt.is_none() {
                        return Err(err);
                    }
                    log::warn!(
                        "bluetooth: L2CAP send failed — the fast lane is down, frames fall back to the GATT floor"
                    );
                }
            }
        }
        if let Some(gatt) = &self.gatt {
            return gatt.send(frame);
        }
        Ok(())
    }
}

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

#[derive(Debug)]
pub enum MacosBleError {
    PowerOnTimeout,
    Closed,
    ControlTooLarge,
    NotifyFailed,
    PublishFailed,
    FrameTooLarge,
    DialFailed,
}

impl MacosBleBackend {
    pub async fn new() -> Result<Self, MacosBleError> {
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

            let peripheral_delegate = PeripheralDelegate::new(events_tx, queue.clone());
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
                    log::info!(
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
                    log::error!("bluetooth: L2CAP publish failed at startup");
                    return Err(MacosBleError::PublishFailed);
                }
                Ok(Some(_)) => continue,
                Ok(None) => return Err(MacosBleError::Closed),
                Err(_) => {
                    log::error!(
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

impl BleBackend for MacosBleBackend {
    const MAX_PEERS: usize = limits::MACOS_MAX_PEERS;
    type Error = MacosBleError;
    type Link = GattLink;

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), MacosBleError> {
        self.peripheral_delegate.0.set_advertising(enabled);
        Ok(())
    }

    async fn set_scanning(&mut self, enabled: bool) -> Result<(), MacosBleError> {
        let central = SendCentralManager(self.central.0.clone());
        self.queue.exec_async(move || {
            let central = central;
            unsafe { central.0.stopScan() };
            if enabled {
                start_scan(&central.0);
                log::info!("bluetooth: scanning for Prns peers");
            } else {
                log::info!("bluetooth: scanning stopped — at connection capacity");
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
                        log::debug!(
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
            log::warn!("bluetooth: dial to {token:02x?} — peripheral not yet sighted");
            return;
        };
        let (control_tx, control_rx) = tokio_mpsc::unbounded_channel::<Control>();
        let (result_tx, result_rx) = oneshot::channel::<DialChars>();
        let (data_inbound_tx, data_inbound_rx) = tokio_mpsc::unbounded_channel::<Box<[u8]>>();
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
            },
        };
        log::debug!("bluetooth: dialing {token:02x?} over LE (central role)");
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
                    log::warn!("bluetooth: dial to {token:02x?} did not reach control-ready");
                    return None;
                }
            };
            Some((
                GattLink {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "needs a real Bluetooth radio + Bluetooth permission; run with `--ignored` on a Mac"]
    async fn the_node_powers_on_advertises_and_scans() {
        let _backend = MacosBleBackend::new()
            .await
            .expect("bluetooth should power on, advertise, and scan");
    }
}
