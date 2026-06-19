#![allow(clippy::undocumented_unsafe_blocks)]

use core::cell::RefCell;
use core::ffi::c_void;
use core::ptr::NonNull;
use core::time::Duration;
use std::collections::{HashSet, VecDeque};
use std::sync::mpsc as sync_mpsc;
use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_core_bluetooth::{
    CBATTError, CBATTRequest, CBAdvertisementDataLocalNameKey, CBAdvertisementDataServiceUUIDsKey,
    CBAttributePermissions, CBCentral, CBCentralManager, CBCentralManagerDelegate,
    CBCharacteristic, CBCharacteristicProperties, CBL2CAPChannel, CBManagerState,
    CBMutableCharacteristic, CBMutableService, CBPeripheral, CBPeripheralManager,
    CBPeripheralManagerDelegate, CBService, CBUUID,
};
use objc2_core_foundation::{
    CFOptionFlags, CFReadStream, CFStreamClientContext, CFStreamEventType, CFWriteStream,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSError, NSInputStream, NSNumber, NSObject, NSObjectProtocol,
    NSOutputStream, NSString, NSUUID,
};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use personal_rns::interfaces::bluetooth_auto::core::{
    encode_stream_frame, BleAddress, BleUuid, Control, Dialect, Psm, StreamDeframer, Transport,
    BLE_HW_MTU, BLE_SERVICE_UUID, CONTROL_MAX_LEN, NATIVE_CONTROL_UUID, STREAM_FRAME_PREFIX_LEN,
};
use personal_rns::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource,
};

const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;
const READ_CHUNK: usize = L2CAP_SDU_LEN;
const POWER_ON_TIMEOUT: Duration = Duration::from_secs(10);
const L2CAP_OPEN_TIMEOUT: Duration = Duration::from_secs(10);

const READ_EVENTS: CFOptionFlags = CFStreamEventType::HasBytesAvailable.0
    | CFStreamEventType::ErrorOccurred.0
    | CFStreamEventType::EndEncountered.0;
const WRITE_EVENTS: CFOptionFlags = CFStreamEventType::CanAcceptBytes.0
    | CFStreamEventType::ErrorOccurred.0
    | CFStreamEventType::EndEncountered.0;

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
    let uuids_key: &NSString = unsafe { CBAdvertisementDataServiceUUIDsKey };
    let uuids_value: &AnyObject = services;
    let name_key: &NSString = unsafe { CBAdvertisementDataLocalNameKey };
    let name = NSString::from_str("Prns");
    let name_ref: &NSString = &name;
    let name_value: &AnyObject = name_ref;
    NSDictionary::from_slices(&[uuids_key, name_key], &[uuids_value, name_value])
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

enum Event {
    Powered,
    Published { psm: u16 },
    PublishFailed,
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
            if unsafe { central.state() } == CBManagerState::PoweredOn {
                let _ = self.ivars().events.send(Event::Powered);
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
    queue: DispatchRetained<DispatchQueue>,
    active: RefCell<Option<tokio_mpsc::UnboundedSender<Control>>>,
    pending_channel: RefCell<Option<oneshot::Sender<DataPlane>>>,
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
                unsafe { peripheral.publishL2CAPChannelWithEncryption(false) };
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
            let Some(tx) = self.ivars().pending_channel.borrow_mut().take() else {
                log::warn!("bluetooth: L2CAP channel opened but no link is awaiting it — dropping");
                return;
            };
            let Some(data) = wire_l2cap(channel, &self.ivars().queue) else {
                log::warn!("bluetooth: L2CAP channel exposes no streams — dropping");
                return;
            };
            log::info!("bluetooth: L2CAP channel opened, data plane up");
            let _ = tx.send(data);
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
                        let (chan_tx, chan_rx) = oneshot::channel::<DataPlane>();
                        let central = unsafe { request.central() };
                        let identifier = unsafe { central.identifier() };
                        let address = BleAddress::new(uuid_token(&identifier));
                        log::info!(
                            "bluetooth: inbound central {:02x?} — control link opened, handshaking",
                            address.octets()
                        );
                        let link = GattLink {
                            manager: SendPeripheralManager(peripheral.retain()),
                            characteristic: SendCharacteristic(self.ivars().characteristic.clone()),
                            control_rx: rx,
                            address,
                            data_rx: Some(chan_rx),
                            data: None,
                        };
                        let _ = self.ivars().events.send(Event::Inbound(link));
                        *active = Some(tx);
                        *self.ivars().pending_channel.borrow_mut() = Some(chan_tx);
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
            log::warn!(
                "bluetooth: central {:02x?} unsubscribed — GATT link dropped before/after handshake",
                uuid_token(&identifier)
            );
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
            queue,
            active: RefCell::new(None),
            pending_channel: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }
}

pub struct GattLink {
    manager: SendPeripheralManager,
    characteristic: SendCharacteristic,
    control_rx: tokio_mpsc::UnboundedReceiver<Control>,
    address: BleAddress,
    data_rx: Option<oneshot::Receiver<DataPlane>>,
    data: Option<DataPlane>,
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

    async fn control_recv(&mut self) -> Result<Control, MacosBleError> {
        let control = self.control_rx.recv().await.ok_or(MacosBleError::Closed)?;
        log::debug!("bluetooth: {:02x?} <- {control:?}", self.address.octets());
        Ok(control)
    }

    async fn upgrade(&mut self, transport: &Transport) -> Result<(), MacosBleError> {
        match transport {
            Transport::L2cap { .. } => {
                let rx = self.data_rx.take().ok_or(MacosBleError::Closed)?;
                match tokio::time::timeout(L2CAP_OPEN_TIMEOUT, rx).await {
                    Ok(Ok(data)) => {
                        log::info!(
                            "bluetooth: {:02x?} L2CAP data plane established",
                            self.address.octets()
                        );
                        self.data = Some(data);
                        Ok(())
                    }
                    Ok(Err(_)) => {
                        log::warn!(
                            "bluetooth: {:02x?} L2CAP upgrade aborted — channel setup failed",
                            self.address.octets()
                        );
                        Err(MacosBleError::Closed)
                    }
                    Err(_) => {
                        log::warn!(
                            "bluetooth: {:02x?} L2CAP upgrade timed out after {}s — peer never opened the CoC; member will carry no data",
                            self.address.octets(),
                            L2CAP_OPEN_TIMEOUT.as_secs()
                        );
                        Err(MacosBleError::Closed)
                    }
                }
            }
            Transport::Gatt => {
                log::info!(
                    "bluetooth: {:02x?} settled GATT-only (peer offered no L2CAP) — no data plane",
                    self.address.octets()
                );
                Ok(())
            }
        }
    }

    fn into_data(self) -> (GattSource, GattSink) {
        match self.data {
            Some(data) => {
                let pump = data.pump;
                (
                    GattSource {
                        inner: SourceInner::L2cap(Box::new(L2capReader {
                            inbound_rx: data.inbound_rx,
                            deframer: StreamDeframer::new(),
                            _pump: pump.clone(),
                        })),
                    },
                    GattSink {
                        inner: SinkInner::L2cap(L2capWriter {
                            outbound: data.outbound,
                            queue: data.queue,
                            pump_ptr: data.pump_ptr,
                            _pump: pump,
                        }),
                    },
                )
            }
            None => (
                GattSource {
                    inner: SourceInner::Pending,
                },
                GattSink {
                    inner: SinkInner::Noop,
                },
            ),
        }
    }
}

struct L2capReader {
    inbound_rx: tokio_mpsc::UnboundedReceiver<Box<[u8]>>,
    deframer: StreamDeframer<{ 2 * L2CAP_SDU_LEN }>,
    _pump: Arc<PumpHandle>,
}

enum SourceInner {
    Pending,
    L2cap(Box<L2capReader>),
}

pub struct GattSource {
    inner: SourceInner,
}

impl BleSource for GattSource {
    type Error = MacosBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, MacosBleError> {
        match &mut self.inner {
            SourceInner::Pending => core::future::pending().await,
            SourceInner::L2cap(reader) => loop {
                if let Some(len) = reader.deframer.next_frame(out) {
                    return Ok(len);
                }
                let chunk = reader
                    .inbound_rx
                    .recv()
                    .await
                    .ok_or(MacosBleError::Closed)?;
                if !reader.deframer.absorb(&chunk) {
                    return Err(MacosBleError::FrameTooLarge);
                }
            },
        }
    }
}

struct L2capWriter {
    outbound: Arc<Mutex<Outbound>>,
    queue: DispatchRetained<DispatchQueue>,
    pump_ptr: PumpPtr,
    _pump: Arc<PumpHandle>,
}

enum SinkInner {
    Noop,
    L2cap(L2capWriter),
}

pub struct GattSink {
    inner: SinkInner,
}

impl BleSink for GattSink {
    type Error = MacosBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), MacosBleError> {
        match &mut self.inner {
            SinkInner::Noop => Ok(()),
            SinkInner::L2cap(writer) => {
                let mut framed = [0u8; L2CAP_SDU_LEN];
                let len =
                    encode_stream_frame(frame, &mut framed).ok_or(MacosBleError::FrameTooLarge)?;
                {
                    let Ok(mut out) = writer.outbound.lock() else {
                        return Err(MacosBleError::Closed);
                    };
                    if out.closed {
                        return Err(MacosBleError::Closed);
                    }
                    out.pending.extend(framed[..len].iter().copied());
                }
                let ptr = writer.pump_ptr;
                writer.queue.exec_async(move || {
                    let ptr = ptr;
                    flush(unsafe { &*ptr.0 });
                });
                Ok(())
            }
        }
    }
}

pub struct MacosBleBackend {
    _keepalive: sync_mpsc::Sender<()>,
    events: tokio_mpsc::UnboundedReceiver<Event>,
    psm: Psm,
    seen: HashSet<[u8; 6]>,
}

#[derive(Debug)]
pub enum MacosBleError {
    PowerOnTimeout,
    Closed,
    ControlTooLarge,
    NotifyFailed,
    PublishFailed,
    FrameTooLarge,
    DialNotImplemented,
}

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

            let peripheral_delegate = PeripheralDelegate::new(events_tx, queue.clone());
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
                        log::debug!("bluetooth: sighted Prns peer {:02x?}", address.octets());
                        return BleEvent::Sighting(address);
                    }
                }
                Some(Event::Inbound(link)) => return BleEvent::Inbound(link),
                Some(_) => continue,
                None => core::future::pending().await,
            }
        }
    }

    async fn dial(&mut self, address: BleAddress) -> Result<GattLink, MacosBleError> {
        log::debug!(
            "bluetooth: dial to {:02x?} skipped — central role not yet implemented (M2a); the Mac only accepts inbound for now",
            address.octets()
        );
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
