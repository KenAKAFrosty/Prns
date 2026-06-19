#![allow(clippy::undocumented_unsafe_blocks)]

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::mpsc as sync_mpsc;
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_core_bluetooth::{
    CBATTError, CBATTRequest, CBAdvertisementDataServiceUUIDsKey, CBAttributePermissions,
    CBCentralManager, CBCentralManagerDelegate, CBCharacteristic, CBCharacteristicProperties,
    CBManagerState, CBMutableCharacteristic, CBMutableService, CBPeripheral, CBPeripheralManager,
    CBPeripheralManagerDelegate, CBService, CBUUID,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString, NSUUID,
};
use tokio::sync::mpsc as tokio_mpsc;

use personal_rns::interfaces::bluetooth_auto::core::{
    BleAddress, BleUuid, Control, Dialect, Transport, BLE_SERVICE_UUID, CONTROL_MAX_LEN,
    NATIVE_CONTROL_UUID,
};
use personal_rns::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource,
};

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

fn advertisement_data(services: &NSArray<CBUUID>) -> Retained<NSDictionary<NSString, AnyObject>> {
    let key: &NSString = unsafe { CBAdvertisementDataServiceUUIDsKey };
    let value: &AnyObject = services;
    NSDictionary::from_slices(&[key], &[value])
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

enum Event {
    Powered { on: bool },
    Sighting(BleAddress),
    Inbound(GattLink),
}

struct CentralDelegateIvars {
    events: tokio_mpsc::UnboundedSender<Event>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = CentralDelegateIvars]
    struct CentralDelegate;

    unsafe impl NSObjectProtocol for CentralDelegate {}

    unsafe impl CBCentralManagerDelegate for CentralDelegate {
        #[unsafe(method(centralManagerDidUpdateState:))]
        fn did_update_state(&self, central: &CBCentralManager) {
            let on = unsafe { central.state() } == CBManagerState::PoweredOn;
            let _ = self.ivars().events.send(Event::Powered { on });
            if on {
                let uuid = service_uuid();
                let services = NSArray::from_slice(&[&*uuid]);
                unsafe { central.scanForPeripheralsWithServices_options(Some(&services), None) };
            }
        }

        #[unsafe(method(centralManager:didDiscoverPeripheral:advertisementData:RSSI:))]
        fn did_discover(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            _advertisement_data: &NSDictionary<NSString, AnyObject>,
            _rssi: &NSNumber,
        ) {
            let identifier = unsafe { peripheral.identifier() };
            let token = uuid_token(&identifier);
            let _ = self
                .ivars()
                .events
                .send(Event::Sighting(BleAddress::new(token)));
        }
    }
);

impl CentralDelegate {
    fn new(events: tokio_mpsc::UnboundedSender<Event>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(CentralDelegateIvars { events });
        unsafe { msg_send![super(this), init] }
    }
}

struct PeripheralDelegateIvars {
    events: tokio_mpsc::UnboundedSender<Event>,
    characteristic: Retained<CBMutableCharacteristic>,
    active: RefCell<Option<tokio_mpsc::UnboundedSender<Control>>>,
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
                let control: &CBCharacteristic = &self.ivars().characteristic;
                let characteristics = NSArray::from_slice(&[control]);
                let service = unsafe {
                    CBMutableService::initWithType_primary(
                        CBMutableService::alloc(),
                        &service_uuid(),
                        true,
                    )
                };
                unsafe { service.setCharacteristics(Some(&characteristics)) };
                unsafe { peripheral.addService(&service) };
            }
        }

        #[unsafe(method(peripheralManager:didAddService:error:))]
        fn did_add_service(
            &self,
            peripheral: &CBPeripheralManager,
            _service: &CBService,
            _error: Option<&NSError>,
        ) {
            let uuid = service_uuid();
            let services = NSArray::from_slice(&[&*uuid]);
            let data = advertisement_data(&services);
            unsafe { peripheral.startAdvertising(Some(&data)) };
        }

        #[unsafe(method(peripheralManager:didReceiveWriteRequests:))]
        fn did_receive_write_requests(
            &self,
            peripheral: &CBPeripheralManager,
            requests: &NSArray<CBATTRequest>,
        ) {
            for request in requests.iter() {
                let Some(data) = (unsafe { request.value() }) else {
                    unsafe {
                        peripheral.respondToRequest_withResult(&request, CBATTError::Success)
                    };
                    continue;
                };
                if let Some(control) = Control::decode(&data.to_vec()) {
                    let mut active = self.ivars().active.borrow_mut();
                    if active.is_none() {
                        let (tx, rx) = tokio_mpsc::unbounded_channel::<Control>();
                        let central = unsafe { request.central() };
                        let identifier = unsafe { central.identifier() };
                        let link = GattLink {
                            manager: SendPeripheralManager(peripheral.retain()),
                            characteristic: SendCharacteristic(self.ivars().characteristic.clone()),
                            control_rx: rx,
                            address: BleAddress::new(uuid_token(&identifier)),
                        };
                        let _ = self.ivars().events.send(Event::Inbound(link));
                        *active = Some(tx);
                    }
                    if let Some(tx) = active.as_ref() {
                        let _ = tx.send(control);
                    }
                }
                unsafe { peripheral.respondToRequest_withResult(&request, CBATTError::Success) };
            }
        }
    }
);

impl PeripheralDelegate {
    fn new(events: tokio_mpsc::UnboundedSender<Event>) -> Retained<Self> {
        let characteristic = unsafe {
            CBMutableCharacteristic::initWithType_properties_value_permissions(
                CBMutableCharacteristic::alloc(),
                &control_uuid(),
                CBCharacteristicProperties::Write
                    | CBCharacteristicProperties::WriteWithoutResponse
                    | CBCharacteristicProperties::Notify,
                None,
                CBAttributePermissions::Writeable,
            )
        };
        let this = Self::alloc().set_ivars(PeripheralDelegateIvars {
            events,
            characteristic,
            active: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }
}

pub struct GattLink {
    manager: SendPeripheralManager,
    characteristic: SendCharacteristic,
    control_rx: tokio_mpsc::UnboundedReceiver<Control>,
    address: BleAddress,
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
        let sent = unsafe {
            self.manager
                .0
                .updateValue_forCharacteristic_onSubscribedCentrals(
                    &data,
                    &self.characteristic.0,
                    None,
                )
        };
        if sent {
            Ok(())
        } else {
            Err(MacosBleError::NotifyFailed)
        }
    }

    async fn control_recv(&mut self) -> Result<Control, MacosBleError> {
        self.control_rx.recv().await.ok_or(MacosBleError::Closed)
    }

    async fn upgrade(&mut self, _transport: &Transport) -> Result<(), MacosBleError> {
        Ok(())
    }

    fn into_data(self) -> (GattSource, GattSink) {
        (GattSource, GattSink)
    }
}

pub struct GattSource;

impl BleSource for GattSource {
    type Error = MacosBleError;

    async fn recv_frame(&mut self, _out: &mut [u8]) -> Result<usize, MacosBleError> {
        core::future::pending().await
    }
}

pub struct GattSink;

impl BleSink for GattSink {
    type Error = MacosBleError;

    async fn send_frame(&mut self, _frame: &[u8]) -> Result<(), MacosBleError> {
        Ok(())
    }
}

pub struct MacosBleBackend {
    _keepalive: sync_mpsc::Sender<()>,
    events: tokio_mpsc::UnboundedReceiver<Event>,
    seen: HashSet<[u8; 6]>,
}

#[derive(Debug)]
pub enum MacosBleError {
    PowerOnTimeout,
    Closed,
    ControlTooLarge,
    NotifyFailed,
    DialNotImplemented,
}

const POWER_ON_TIMEOUT: Duration = Duration::from_secs(10);

impl MacosBleBackend {
    pub async fn new() -> Result<Self, MacosBleError> {
        let (events_tx, mut events_rx) = tokio_mpsc::unbounded_channel::<Event>();
        let (keepalive, shutdown_rx) = sync_mpsc::channel::<()>();
        let central_events = events_tx.clone();

        std::thread::spawn(move || {
            let queue = DispatchQueue::new("com.personal.prns.ble", None);

            let central_delegate = CentralDelegate::new(central_events);
            let central_proto = ProtocolObject::from_ref(&*central_delegate);
            let _central: Retained<CBCentralManager> = unsafe {
                CBCentralManager::initWithDelegate_queue(
                    CBCentralManager::alloc(),
                    Some(central_proto),
                    Some(&queue),
                )
            };

            let peripheral_delegate = PeripheralDelegate::new(events_tx);
            let peripheral_proto = ProtocolObject::from_ref(&*peripheral_delegate);
            let _peripheral: Retained<CBPeripheralManager> = unsafe {
                CBPeripheralManager::initWithDelegate_queue(
                    CBPeripheralManager::alloc(),
                    Some(peripheral_proto),
                    Some(&queue),
                )
            };

            let _ = shutdown_rx.recv();
        });

        loop {
            match tokio::time::timeout(POWER_ON_TIMEOUT, events_rx.recv()).await {
                Ok(Some(Event::Powered { on: true })) => {
                    return Ok(Self {
                        _keepalive: keepalive,
                        events: events_rx,
                        seen: HashSet::new(),
                    });
                }
                Ok(Some(_)) => continue,
                Ok(None) => return Err(MacosBleError::Closed),
                Err(_) => return Err(MacosBleError::PowerOnTimeout),
            }
        }
    }

    pub async fn next_sighting(&mut self) -> Option<BleAddress> {
        loop {
            match self.events.recv().await? {
                Event::Sighting(address) => {
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
    const MAX_PEERS: usize = 8;
    type Error = MacosBleError;
    type Link = GattLink;

    async fn advertise(&mut self) -> Result<(), MacosBleError> {
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<GattLink> {
        loop {
            match self.events.recv().await {
                Some(Event::Sighting(address)) => {
                    if self.seen.insert(*address.octets()) {
                        return BleEvent::Sighting(address);
                    }
                }
                Some(Event::Inbound(link)) => return BleEvent::Inbound(link),
                Some(Event::Powered { .. }) => continue,
                None => core::future::pending().await,
            }
        }
    }

    async fn dial(&mut self, _address: BleAddress) -> Result<GattLink, MacosBleError> {
        Err(MacosBleError::DialNotImplemented)
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
