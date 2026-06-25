use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bluer::adv::{Advertisement, AdvertisementHandle};
use bluer::gatt::local::{
    characteristic_control, service_control, Application, ApplicationHandle, Characteristic,
    CharacteristicControl, CharacteristicControlEvent, CharacteristicControlHandle,
    CharacteristicNotify, CharacteristicNotifyMethod, CharacteristicWrite,
    CharacteristicWriteMethod, Service,
};
use bluer::gatt::remote::Characteristic as RemoteCharacteristic;
use bluer::gatt::{CharacteristicReader, CharacteristicWriter};
use bluer::l2cap::{
    Security, SecurityLevel, SeqPacket, SeqPacketListener, Socket, SocketAddr as L2capSocketAddr,
};
use bluer::{
    Adapter, AdapterEvent, Address, AddressType, Device, DiscoveryFilter, DiscoveryTransport,
    Session, Uuid,
};
use futures_util::stream::FuturesUnordered;
use futures_util::{Stream, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

use crate::interfaces::bluetooth_auto::core::{
    encode_stream_frame, fragments_of, BleAddress, BleUuid, Control, Dialect, Fragment, L2capPlan,
    Psm, Reassembler, StreamDeframer, BLE_HW_MTU, BLE_SERVICE_UUID, CONTROL_MAX_LEN,
    FRAGMENT_HEADER_LEN, NATIVE_CONTROL_UUID, NATIVE_DATA_UUID, STREAM_FRAME_PREFIX_LEN,
};
use crate::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin,
};

const ADVERTISED_NAME: &str = "Prns";

const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;

const GATT_FRAGMENT_PAYLOAD: usize = 180;
const GATT_REASSEMBLY_CAP: usize = 600;

const SCAN_STOP_POLL: Duration = Duration::from_millis(20);
const SCAN_STOP_ATTEMPTS: usize = 25;

const RESWEEP_INTERVAL: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const L2CAP_UPGRADE_TIMEOUT: Duration = Duration::from_secs(5);

const EATT_BLOCKED_REASON: &str = "BlueZ EATT enabled (would prompt nearby Android peers)";

fn gatt_channels_setting() -> Option<u32> {
    let text = std::fs::read_to_string("/etc/bluetooth/main.conf").ok()?;
    let mut in_gatt = false;
    let mut channels = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_gatt = line.eq_ignore_ascii_case("[gatt]");
            continue;
        }
        if in_gatt {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim().eq_ignore_ascii_case("Channels") {
                    if let Ok(n) = value.trim().parse::<u32>() {
                        channels = Some(n);
                    }
                }
            }
        }
    }
    channels
}

fn bluez_eatt_default_on() -> bool {
    let Ok(output) = std::process::Command::new("bluetoothctl")
        .arg("--version")
        .output()
    else {
        return true;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(version) = text.split_whitespace().last() else {
        return true;
    };
    let mut parts = version.split('.');
    let (Some(major), Some(minor)) = (
        parts.next().and_then(|p| p.parse::<u32>().ok()),
        parts.next().and_then(|p| p.parse::<u32>().ok()),
    ) else {
        return true;
    };
    major == 5 && (54..=66).contains(&minor)
}

fn eatt_is_risky() -> bool {
    match gatt_channels_setting() {
        Some(1) => false,
        Some(_) => true,
        None => bluez_eatt_default_on(),
    }
}

#[derive(Debug)]
pub enum BluerError {
    Bluez(bluer::Error),
    Io(std::io::Error),
    NoControlCharacteristic,
    ControlPduTooLarge,
    MalformedControl,
    NotUpgraded,
    FrameTooLarge,
    DialTimeout,
    L2capTimeout,
    Closed,
}

impl From<bluer::Error> for BluerError {
    fn from(error: bluer::Error) -> Self {
        BluerError::Bluez(error)
    }
}

impl From<std::io::Error> for BluerError {
    fn from(error: std::io::Error) -> Self {
        BluerError::Io(error)
    }
}

fn uuid_of(uuid: BleUuid) -> Uuid {
    match uuid {
        BleUuid::Bit128(bytes) => Uuid::from_bytes(bytes),
        BleUuid::Bit16(short) => {
            let mut bytes = [
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b,
                0x34, 0xfb,
            ];
            bytes[2..4].copy_from_slice(&short.to_be_bytes());
            Uuid::from_bytes(bytes)
        }
    }
}

fn native_characteristic(uuid: Uuid, control_handle: CharacteristicControlHandle) -> Characteristic {
    Characteristic {
        uuid,
        write: Some(CharacteristicWrite {
            write: true,
            write_without_response: true,
            method: CharacteristicWriteMethod::Io,
            ..Default::default()
        }),
        notify: Some(CharacteristicNotify {
            notify: true,
            method: CharacteristicNotifyMethod::Io,
            ..Default::default()
        }),
        control_handle,
        ..Default::default()
    }
}

#[derive(Default)]
struct PendingHalves {
    reader: Option<CharacteristicReader>,
    writer: Option<CharacteristicWriter>,
}

#[derive(Default)]
struct PendingData {
    writer: Option<CharacteristicWriter>,
    reader: Option<CharacteristicReader>,
}

enum Half {
    Reader(CharacteristicReader),
    Writer(CharacteristicWriter),
}

enum DataRead {
    Ready(CharacteristicReader),
    Pending(oneshot::Receiver<CharacteristicReader>),
}

enum ServerData {
    TwoChar {
        writer: CharacteristicWriter,
        reader: DataRead,
    },
    SingleChar,
}

type ConnectFuture = Pin<Box<dyn Future<Output = (Address, Result<BluerLink, BluerError>)> + Send>>;

enum Observed {
    Candidate(Address),
    Greeting { address: Address, half: Half },
    DataHalf { address: Address, half: Half },
    Connected(Address, Result<BluerLink, BluerError>),
    Resweep,
    Idle,
}

pub struct BluerBackend {
    adapter: Adapter,
    address: Address,
    address_type: AddressType,
    psm: Psm,
    connecting: HashSet<Address>,
    connects: FuturesUnordered<ConnectFuture>,
    pending: HashMap<Address, PendingHalves>,
    scan_enabled: bool,
    resweep_next: usize,
    discovery: Option<Pin<Box<dyn Stream<Item = AdapterEvent> + Send>>>,
    control: Option<Pin<Box<CharacteristicControl>>>,
    data_control: Option<Pin<Box<CharacteristicControl>>>,
    pending_data: HashMap<Address, PendingData>,
    awaiting_data_reader: HashMap<Address, oneshot::Sender<CharacteristicReader>>,
    listener: Option<Arc<SeqPacketListener>>,
    _advertisement: Option<AdvertisementHandle>,
    _application: Option<ApplicationHandle>,
    blocked: Option<&'static str>,
}

impl BluerBackend {
    pub async fn open(psm: Psm) -> Result<Self, BluerError> {
        let session = Session::new().await?;
        let adapter = session.default_adapter().await?;
        adapter.set_powered(true).await?;
        let address = adapter.address().await?;
        let address_type = adapter.address_type().await?;
        let blocked = if eatt_is_risky() {
            log::error!(
                "bluetooth: NOT starting — BlueZ Enhanced ATT (EATT) is enabled, so every nearby \
                 Android would show a pairing prompt (EATT requires an encrypted link). This will not \
                 resolve on its own. Disable EATT and restart bluetoothd (one-time):\n  printf \
                 '\\n[GATT]\\nChannels = 1\\n' | sudo tee -a /etc/bluetooth/main.conf && sudo \
                 systemctl restart bluetooth\n  (Channels=1 is the upstream BlueZ default on 5.67+; \
                 on those versions BLE starts with no action.)"
            );
            Some(EATT_BLOCKED_REASON)
        } else {
            None
        };
        Ok(Self {
            adapter,
            address,
            address_type,
            psm,
            connecting: HashSet::new(),
            connects: FuturesUnordered::new(),
            pending: HashMap::new(),
            scan_enabled: false,
            resweep_next: 0,
            discovery: None,
            control: None,
            data_control: None,
            pending_data: HashMap::new(),
            awaiting_data_reader: HashMap::new(),
            listener: None,
            _advertisement: None,
            _application: None,
            blocked,
        })
    }

    async fn advertises_our_service(&self, address: Address) -> bool {
        let Ok(device) = self.adapter.device(address) else {
            return false;
        };
        match device.uuids().await {
            Ok(Some(uuids)) => uuids.contains(&uuid_of(BLE_SERVICE_UUID)),
            _ => false,
        }
    }

    async fn resweep_sighting(&mut self) -> Option<Address> {
        let mut addresses = self.adapter.device_addresses().await.ok()?;
        addresses.retain(|address| *address != self.address && !self.connecting.contains(address));
        if addresses.is_empty() {
            return None;
        }
        addresses.sort_by_key(|address| address.0);
        let count = addresses.len();
        for offset in 0..count {
            let index = (self.resweep_next + offset) % count;
            let address = addresses[index];
            if self.advertises_our_service(address).await {
                self.resweep_next = index + 1;
                return Some(address);
            }
        }
        None
    }

    async fn peer_rssi(&self, address: Address) -> Option<i8> {
        let device = self.adapter.device(address).ok()?;
        let rssi = device.rssi().await.ok().flatten()?;
        Some(rssi.clamp(i8::MIN as i16, i8::MAX as i16) as i8)
    }

    async fn start_discovery(&mut self) -> Result<(), BluerError> {
        self.adapter
            .set_discovery_filter(DiscoveryFilter {
                transport: DiscoveryTransport::Le,
                uuids: [uuid_of(BLE_SERVICE_UUID)].into_iter().collect(),
                ..Default::default()
            })
            .await?;
        let discovery = self.adapter.discover_devices().await?;
        self.discovery = Some(Box::pin(discovery));
        Ok(())
    }

    fn admit_greeting(&mut self, address: Address, half: Half) -> Option<AcceptedLink> {
        let ready = {
            let entry = self.pending.entry(address).or_default();
            match half {
                Half::Reader(reader) => entry.reader = Some(reader),
                Half::Writer(writer) => entry.writer = Some(writer),
            }
            entry.reader.is_some() && entry.writer.is_some()
        };
        if !ready {
            return None;
        }
        match (self.pending.remove(&address), self.listener.clone()) {
            (
                Some(PendingHalves {
                    reader: Some(reader),
                    writer: Some(writer),
                }),
                Some(listener),
            ) => {
                let data = self.take_server_data(address);
                Some(AcceptedLink {
                    reader,
                    writer,
                    address,
                    listener,
                    socket: None,
                    data,
                })
            }
            _ => None,
        }
    }

    fn take_server_data(&mut self, address: Address) -> ServerData {
        let data = self.pending_data.remove(&address).unwrap_or_default();
        match data.writer {
            Some(writer) => {
                let reader = match data.reader {
                    Some(reader) => DataRead::Ready(reader),
                    None => {
                        let (tx, rx) = oneshot::channel();
                        self.awaiting_data_reader.insert(address, tx);
                        DataRead::Pending(rx)
                    }
                };
                ServerData::TwoChar { writer, reader }
            }
            None => ServerData::SingleChar,
        }
    }

    fn admit_data_half(&mut self, address: Address, half: Half) {
        match half {
            Half::Writer(writer) => {
                self.pending_data.entry(address).or_default().writer = Some(writer);
            }
            Half::Reader(reader) => match self.awaiting_data_reader.remove(&address) {
                Some(tx) => {
                    let _ = tx.send(reader);
                }
                None => {
                    self.pending_data.entry(address).or_default().reader = Some(reader);
                }
            },
        }
    }
}

async fn next_or_pending<S>(stream: Option<&mut S>) -> Option<S::Item>
where
    S: Stream + Unpin,
{
    match stream {
        Some(stream) => stream.next().await,
        None => core::future::pending().await,
    }
}

async fn next_connect(
    connects: &mut FuturesUnordered<ConnectFuture>,
) -> (Address, Result<BluerLink, BluerError>) {
    match connects.next().await {
        Some(done) => done,
        None => core::future::pending().await,
    }
}

async fn await_scan_stopped(adapter: &Adapter) {
    for _ in 0..SCAN_STOP_ATTEMPTS {
        if matches!(adapter.is_discovering().await, Ok(false)) {
            return;
        }
        tokio::time::sleep(SCAN_STOP_POLL).await;
    }
}

async fn connect_link(adapter: Adapter, target: Address) -> Result<BluerLink, BluerError> {
    let discovered = adapter.device(target)?;
    let peer_address_type = discovered.address_type().await?;
    log::info!("bluetooth: dialing {target} ({peer_address_type:?})");
    let device = if discovered.is_connected().await? {
        discovered
    } else {
        let _ = adapter.remove_device(target).await;
        await_scan_stopped(&adapter).await;
        match adapter.connect_device(target, peer_address_type).await {
            Ok(device) => device,
            Err(error) => {
                log::warn!("bluetooth: LE connect to {target} failed: {error}");
                return Err(error.into());
            }
        }
    };
    let control = match find_characteristic(&device, uuid_of(NATIVE_CONTROL_UUID)).await {
        Ok(Some(control)) => control,
        Ok(None) => {
            log::warn!("bluetooth: no native control characteristic on {target}");
            return Err(BluerError::NoControlCharacteristic);
        }
        Err(error) => {
            log::warn!("bluetooth: failed to inspect {target} services: {error:?}");
            return Err(error);
        }
    };
    let data = find_characteristic(&device, uuid_of(NATIVE_DATA_UUID))
        .await
        .ok()
        .flatten();
    let notify = control.notify().await?;
    let data_notify = match &data {
        Some(data) => match data.notify().await {
            Ok(stream) => Some(Box::pin(stream) as Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>),
            Err(error) => {
                log::warn!("bluetooth: {target} data characteristic notify failed: {error}");
                None
            }
        },
        None => None,
    };
    log::info!("bluetooth: {target} connected over LE, control characteristic ready; handshaking");
    Ok(BluerLink::Dialed(Box::new(DialedLink {
        control,
        notify: Box::pin(notify),
        data,
        data_notify,
        peer_address: target,
        peer_address_type,
        socket: None,
        _device: device,
    })))
}

impl BleBackend for BluerBackend {
    const MAX_PEERS: usize = 8;
    type Error = BluerError;
    type Link = BluerLink;

    fn blocked(&self) -> Option<&str> {
        self.blocked
    }

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), BluerError> {
        if !enabled {
            self._advertisement = None;
            self._application = None;
            self.control = None;
            self.data_control = None;
            self.pending_data.clear();
            self.awaiting_data_reader.clear();
            self.listener = None;
            log::info!("bluetooth: advertising + GATT server down");
            return Ok(());
        }
        if self.control.is_some() {
            return Ok(());
        }
        let advertisement = Advertisement {
            advertisement_type: bluer::adv::Type::Peripheral,
            service_uuids: [uuid_of(BLE_SERVICE_UUID)].into_iter().collect(),
            discoverable: Some(true),
            local_name: Some(ADVERTISED_NAME.to_string()),
            ..Default::default()
        };
        let advertisement = self.adapter.advertise(advertisement).await?;

        let (control, control_handle) = characteristic_control();
        let (data, data_handle) = characteristic_control();
        let (_service_control, service_handle) = service_control();
        let application = Application {
            services: vec![Service {
                uuid: uuid_of(BLE_SERVICE_UUID),
                primary: true,
                characteristics: vec![
                    native_characteristic(uuid_of(NATIVE_CONTROL_UUID), control_handle),
                    native_characteristic(uuid_of(NATIVE_DATA_UUID), data_handle),
                ],
                control_handle: service_handle,
                ..Default::default()
            }],
            ..Default::default()
        };
        let application = self.adapter.serve_gatt_application(application).await?;

        let listener = SeqPacketListener::bind(L2capSocketAddr::new(
            self.address,
            self.address_type,
            self.psm.get(),
        ))
        .await
        .ok();

        self.control = Some(Box::pin(control));
        self.data_control = Some(Box::pin(data));
        self.listener = listener.map(Arc::new);
        self._advertisement = Some(advertisement);
        self._application = Some(application);
        log::info!(
            "bluetooth: advertising as {ADVERTISED_NAME}, control PSM {:#x}, listener {}",
            self.psm.get(),
            if self.listener.is_some() { "bound" } else { "unavailable" },
        );
        Ok(())
    }

    async fn set_scanning(&mut self, enabled: bool) -> Result<(), BluerError> {
        self.scan_enabled = enabled;
        if !enabled {
            self.discovery = None;
            log::info!("bluetooth: scanning off");
        } else {
            log::info!("bluetooth: scanning LE for Prns peers");
        }
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<BluerLink> {
        loop {
            let want_discovery = self.scan_enabled && self.connecting.is_empty();
            if want_discovery && self.discovery.is_none() {
                if let Err(error) = self.start_discovery().await {
                    log::warn!("bluetooth: failed to (re)start scanning: {error:?}");
                }
            } else if !want_discovery && self.discovery.is_some() {
                self.discovery = None;
            }
            let observed = {
                let discovery = self.discovery.as_mut();
                let control = self.control.as_mut();
                let data_control = self.data_control.as_mut();
                let connects = &mut self.connects;
                tokio::select! {
                    event = next_or_pending(discovery) => match event {
                        Some(AdapterEvent::DeviceAdded(address)) => Observed::Candidate(address),
                        _ => Observed::Idle,
                    },
                    event = next_or_pending(control) => match event {
                        Some(CharacteristicControlEvent::Write(request)) => {
                            let address = request.device_address();
                            match request.accept() {
                                Ok(reader) => Observed::Greeting {
                                    address,
                                    half: Half::Reader(reader),
                                },
                                Err(_) => Observed::Idle,
                            }
                        }
                        Some(CharacteristicControlEvent::Notify(writer)) => Observed::Greeting {
                            address: writer.device_address(),
                            half: Half::Writer(writer),
                        },
                        None => Observed::Idle,
                    },
                    event = next_or_pending(data_control) => match event {
                        Some(CharacteristicControlEvent::Write(request)) => {
                            let address = request.device_address();
                            match request.accept() {
                                Ok(reader) => Observed::DataHalf {
                                    address,
                                    half: Half::Reader(reader),
                                },
                                Err(_) => Observed::Idle,
                            }
                        }
                        Some(CharacteristicControlEvent::Notify(writer)) => Observed::DataHalf {
                            address: writer.device_address(),
                            half: Half::Writer(writer),
                        },
                        None => Observed::Idle,
                    },
                    (target, result) = next_connect(connects) => Observed::Connected(target, result),
                    () = tokio::time::sleep(RESWEEP_INTERVAL), if want_discovery => Observed::Resweep,
                }
            };
            match observed {
                Observed::Candidate(address) => {
                    let mine = address == self.address;
                    let dialing = self.connecting.contains(&address);
                    if !mine && !dialing && self.advertises_our_service(address).await {
                        let rssi = self.peer_rssi(address).await;
                        log::info!("bluetooth: sighted Prns peer {address}");
                        return BleEvent::Sighting {
                            address: BleAddress::new(address.0),
                            rssi,
                        };
                    }
                }
                Observed::Greeting { address, half } => {
                    if let Some(link) = self.admit_greeting(address, half) {
                        let peer_rssi = self.peer_rssi(address).await;
                        log::info!("bluetooth: inbound link from {address}");
                        return BleEvent::LinkReady {
                            link: BluerLink::Accepted(Box::new(link)),
                            origin: Origin::Accepted,
                            peer_rssi,
                        };
                    }
                }
                Observed::DataHalf { address, half } => {
                    self.admit_data_half(address, half);
                }
                Observed::Connected(target, result) => {
                    self.connecting.remove(&target);
                    match result {
                        Ok(link) => {
                            let peer_rssi = self.peer_rssi(target).await;
                            return BleEvent::LinkReady {
                                link,
                                origin: Origin::Dialed,
                                peer_rssi,
                            };
                        }
                        Err(error) => {
                            log::warn!("bluetooth: dial to {target} failed: {error:?}");
                            return BleEvent::DialFailed {
                                address: BleAddress::new(target.0),
                            };
                        }
                    }
                }
                Observed::Resweep => {
                    if let Some(address) = self.resweep_sighting().await {
                        let rssi = self.peer_rssi(address).await;
                        return BleEvent::Sighting {
                            address: BleAddress::new(address.0),
                            rssi,
                        };
                    }
                }
                Observed::Idle => {}
            }
        }
    }

    async fn dial(&mut self, address: BleAddress) {
        let target = Address::new(*address.octets());
        if self.connecting.contains(&target) {
            return;
        }
        self.connecting.insert(target);
        let adapter = self.adapter.clone();
        self.connects.push(Box::pin(async move {
            let result = match tokio::time::timeout(CONNECT_TIMEOUT, connect_link(adapter, target))
                .await
            {
                Ok(result) => result,
                Err(_) => Err(BluerError::DialTimeout),
            };
            (target, result)
        }));
    }

    async fn on_link_closed(&mut self, address: BleAddress) {
        let target = Address::new(*address.octets());
        self.connecting.remove(&target);
        self.pending_data.remove(&target);
        self.awaiting_data_reader.remove(&target);
        let _ = self.adapter.remove_device(target).await;
        log::info!("bluetooth: {target} link released; will re-sight if it returns");
    }
}

async fn find_characteristic(
    device: &Device,
    uuid: Uuid,
) -> Result<Option<RemoteCharacteristic>, BluerError> {
    let service_uuid = uuid_of(BLE_SERVICE_UUID);
    for service in device.services().await? {
        if service.uuid().await? == service_uuid {
            for characteristic in service.characteristics().await? {
                if characteristic.uuid().await? == uuid {
                    return Ok(Some(characteristic));
                }
            }
        }
    }
    Ok(None)
}

pub enum BluerLink {
    Dialed(Box<DialedLink>),
    Accepted(Box<AcceptedLink>),
}

impl BleLink for BluerLink {
    type Error = BluerError;
    type Source = BluerSource;
    type Sink = BluerSink;

    fn dialect(&self) -> Dialect {
        match self {
            BluerLink::Dialed(link) => link.dialect(),
            BluerLink::Accepted(link) => link.dialect(),
        }
    }

    fn address(&self) -> BleAddress {
        match self {
            BluerLink::Dialed(link) => link.address(),
            BluerLink::Accepted(link) => link.address(),
        }
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), BluerError> {
        match self {
            BluerLink::Dialed(link) => link.control_send(msg).await,
            BluerLink::Accepted(link) => link.control_send(msg).await,
        }
    }

    async fn control_recv(&mut self) -> Result<Control, BluerError> {
        match self {
            BluerLink::Dialed(link) => link.control_recv().await,
            BluerLink::Accepted(link) => link.control_recv().await,
        }
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), BluerError> {
        match self {
            BluerLink::Dialed(link) => link.upgrade(plan).await,
            BluerLink::Accepted(link) => link.upgrade(plan).await,
        }
    }

    fn into_data(self) -> (BluerSource, BluerSink) {
        match self {
            BluerLink::Dialed(link) => link.into_data(),
            BluerLink::Accepted(link) => link.into_data(),
        }
    }
}

pub struct DialedLink {
    control: RemoteCharacteristic,
    notify: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
    data: Option<RemoteCharacteristic>,
    data_notify: Option<Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>>,
    peer_address: Address,
    peer_address_type: AddressType,
    socket: Option<Arc<SeqPacket>>,
    _device: Device,
}

impl BleLink for DialedLink {
    type Error = BluerError;
    type Source = BluerSource;
    type Sink = BluerSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        BleAddress::new(self.peer_address.0)
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), BluerError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg.encode(&mut buf).ok_or(BluerError::ControlPduTooLarge)?;
        match self.control.write(&buf[..len]).await {
            Ok(()) => {
                log::debug!("bluetooth: {} <- {msg:?}", self.peer_address);
                Ok(())
            }
            Err(error) => {
                log::warn!(
                    "bluetooth: {} control write failed: {error}",
                    self.peer_address
                );
                Err(error.into())
            }
        }
    }

    async fn control_recv(&mut self) -> Result<Control, BluerError> {
        let value = self.notify.next().await.ok_or(BluerError::Closed)?;
        match Control::decode(&value) {
            Some(control) => {
                log::debug!("bluetooth: {} -> {control:?}", self.peer_address);
                Ok(control)
            }
            None => {
                log::warn!(
                    "bluetooth: {} sent an undecodable control notification ({} bytes)",
                    self.peer_address,
                    value.len()
                );
                Err(BluerError::MalformedControl)
            }
        }
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), BluerError> {
        match plan {
            L2capPlan::Open { psm } => {
                log::info!(
                    "bluetooth: {} handshake settled, opening L2CAP CoC to PSM {:#x}",
                    self.peer_address,
                    psm.get()
                );
                let socket = Socket::<SeqPacket>::new_seq_packet()?;
                socket.set_security(Security {
                    level: SecurityLevel::Low,
                    key_size: 0,
                })?;
                socket.set_recv_mtu(L2CAP_SDU_LEN as u16)?;
                socket.bind(L2capSocketAddr::any_le())?;
                let target =
                    L2capSocketAddr::new(self.peer_address, self.peer_address_type, psm.get());
                let connected =
                    match tokio::time::timeout(L2CAP_UPGRADE_TIMEOUT, socket.connect(target)).await {
                        Ok(Ok(connected)) => connected,
                        Ok(Err(error)) => {
                            log::warn!(
                                "bluetooth: {} L2CAP connect to PSM {:#x} failed: {error}; settling on GATT",
                                self.peer_address,
                                psm.get()
                            );
                            return Err(error.into());
                        }
                        Err(_) => {
                            log::warn!(
                                "bluetooth: {} L2CAP connect to PSM {:#x} timed out; settling on GATT",
                                self.peer_address,
                                psm.get()
                            );
                            return Err(BluerError::L2capTimeout);
                        }
                    };
                self.socket = Some(Arc::new(connected));
                log::info!("bluetooth: {} L2CAP data plane up", self.peer_address);
                Ok(())
            }
            L2capPlan::Accept => {
                log::debug!(
                    "bluetooth: {} stays on the GATT data plane (a dialed Linux link does not L2CAP-accept)",
                    self.peer_address
                );
                Ok(())
            }
            L2capPlan::None => Ok(()),
        }
    }

    fn into_data(self) -> (BluerSource, BluerSink) {
        match self.socket {
            Some(socket) => (
                BluerSource::L2cap(Box::new(L2capSource {
                    socket: Some(socket.clone()),
                    deframer: StreamDeframer::new(),
                })),
                BluerSink::L2cap(L2capSink(Some(socket))),
            ),
            None => {
                log::info!("bluetooth: {} GATT data plane up", self.peer_address);
                let (rx, tx) = match (self.data_notify, self.data) {
                    (Some(data_notify), Some(data)) => {
                        (GattRx::Notify(data_notify), GattTx::Remote(data))
                    }
                    _ => (GattRx::Notify(self.notify), GattTx::Remote(self.control)),
                };
                (
                    BluerSource::Gatt(Box::new(GattSource {
                        rx,
                        reassembler: Reassembler::new(),
                    })),
                    BluerSink::Gatt(GattSink { tx }),
                )
            }
        }
    }
}

pub struct AcceptedLink {
    reader: CharacteristicReader,
    writer: CharacteristicWriter,
    address: Address,
    listener: Arc<SeqPacketListener>,
    socket: Option<Arc<SeqPacket>>,
    data: ServerData,
}

impl BleLink for AcceptedLink {
    type Error = BluerError;
    type Source = BluerSource;
    type Sink = BluerSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        BleAddress::new(self.address.0)
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), BluerError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg.encode(&mut buf).ok_or(BluerError::ControlPduTooLarge)?;
        self.writer.write_all(&buf[..len]).await?;
        self.writer.flush().await?;
        log::debug!("bluetooth: {} <- {msg:?}", self.address);
        Ok(())
    }

    async fn control_recv(&mut self) -> Result<Control, BluerError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let read = self.reader.read(&mut buf).await?;
        if read == 0 {
            return Err(BluerError::Closed);
        }
        match Control::decode(&buf[..read]) {
            Some(control) => {
                log::debug!("bluetooth: {} -> {control:?}", self.address);
                Ok(control)
            }
            None => {
                log::warn!(
                    "bluetooth: {} sent an undecodable control write ({read} bytes)",
                    self.address
                );
                Err(BluerError::MalformedControl)
            }
        }
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), BluerError> {
        match plan {
            L2capPlan::Accept => {
                log::info!(
                    "bluetooth: {} handshake settled, accepting L2CAP CoC on our listener",
                    self.address
                );
                match tokio::time::timeout(L2CAP_UPGRADE_TIMEOUT, self.listener.accept()).await {
                    Ok(Ok((connected, _peer))) => {
                        self.socket = Some(Arc::new(connected));
                        log::info!("bluetooth: {} L2CAP data plane up", self.address);
                        Ok(())
                    }
                    Ok(Err(error)) => {
                        log::warn!(
                            "bluetooth: {} L2CAP accept failed: {error}; settling on GATT",
                            self.address
                        );
                        Err(error.into())
                    }
                    Err(_) => {
                        log::warn!(
                            "bluetooth: {} L2CAP accept timed out; settling on GATT",
                            self.address
                        );
                        Err(BluerError::L2capTimeout)
                    }
                }
            }
            L2capPlan::Open { .. } => {
                log::debug!(
                    "bluetooth: {} stays on the GATT data plane (accepted link; Linux-opens-CoC-to-peer is the capability-role follow-up)",
                    self.address
                );
                Ok(())
            }
            L2capPlan::None => Ok(()),
        }
    }

    fn into_data(self) -> (BluerSource, BluerSink) {
        match self.socket {
            Some(socket) => (
                BluerSource::L2cap(Box::new(L2capSource {
                    socket: Some(socket.clone()),
                    deframer: StreamDeframer::new(),
                })),
                BluerSink::L2cap(L2capSink(Some(socket))),
            ),
            None => {
                log::info!("bluetooth: {} GATT data plane up", self.address);
                let (rx, tx) = match self.data {
                    ServerData::TwoChar { writer, reader } => {
                        let rx = match reader {
                            DataRead::Ready(reader) => GattRx::Reader(reader),
                            DataRead::Pending(pending) => GattRx::Pending(Some(pending)),
                        };
                        (rx, GattTx::Writer(writer))
                    }
                    ServerData::SingleChar => {
                        (GattRx::Reader(self.reader), GattTx::Writer(self.writer))
                    }
                };
                (
                    BluerSource::Gatt(Box::new(GattSource {
                        rx,
                        reassembler: Reassembler::new(),
                    })),
                    BluerSink::Gatt(GattSink { tx }),
                )
            }
        }
    }
}

pub struct L2capSource {
    socket: Option<Arc<SeqPacket>>,
    deframer: StreamDeframer<{ 2 * L2CAP_SDU_LEN }>,
}

impl BleSource for L2capSource {
    type Error = BluerError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, BluerError> {
        let Some(socket) = self.socket.clone() else {
            return Err(BluerError::NotUpgraded);
        };
        loop {
            if let Some(len) = self.deframer.next_frame(out) {
                return Ok(len);
            }
            let mut scratch = [0u8; L2CAP_SDU_LEN];
            let read = socket.recv(&mut scratch).await?;
            if read == 0 {
                return Err(BluerError::Closed);
            }
            if !self.deframer.absorb(&scratch[..read]) {
                return Err(BluerError::FrameTooLarge);
            }
        }
    }
}

pub struct L2capSink(Option<Arc<SeqPacket>>);

impl BleSink for L2capSink {
    type Error = BluerError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), BluerError> {
        match &self.0 {
            Some(socket) => {
                let mut framed = [0u8; L2CAP_SDU_LEN];
                let n = encode_stream_frame(frame, &mut framed).ok_or(BluerError::FrameTooLarge)?;
                socket.send(&framed[..n]).await?;
                Ok(())
            }
            None => Err(BluerError::NotUpgraded),
        }
    }
}

pub enum BluerSource {
    L2cap(Box<L2capSource>),
    Gatt(Box<GattSource>),
}

impl BleSource for BluerSource {
    type Error = BluerError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, BluerError> {
        match self {
            BluerSource::L2cap(source) => source.recv_frame(out).await,
            BluerSource::Gatt(source) => source.recv_frame(out).await,
        }
    }
}

pub enum BluerSink {
    L2cap(L2capSink),
    Gatt(GattSink),
}

impl BleSink for BluerSink {
    type Error = BluerError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), BluerError> {
        match self {
            BluerSink::L2cap(sink) => sink.send_frame(frame).await,
            BluerSink::Gatt(sink) => sink.send_frame(frame).await,
        }
    }
}

enum GattRx {
    Notify(Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>),
    Reader(CharacteristicReader),
    Pending(Option<oneshot::Receiver<CharacteristicReader>>),
}

enum GattTx {
    Remote(RemoteCharacteristic),
    Writer(CharacteristicWriter),
}

pub struct GattSource {
    rx: GattRx,
    reassembler: Reassembler<GATT_REASSEMBLY_CAP>,
}

impl BleSource for GattSource {
    type Error = BluerError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, BluerError> {
        loop {
            let chunk = match &mut self.rx {
                GattRx::Notify(notify) => notify.next().await.ok_or(BluerError::Closed)?,
                GattRx::Reader(reader) => {
                    let mut scratch = [0u8; BLE_HW_MTU];
                    let read = reader.read(&mut scratch).await?;
                    if read == 0 {
                        return Err(BluerError::Closed);
                    }
                    scratch[..read].to_vec()
                }
                GattRx::Pending(slot) => {
                    let pending = slot.take().ok_or(BluerError::Closed)?;
                    let reader = pending.await.map_err(|_| BluerError::Closed)?;
                    self.rx = GattRx::Reader(reader);
                    continue;
                }
            };
            let Some(fragment) = Fragment::decode(&chunk) else {
                continue;
            };
            if let Some(frame) = self.reassembler.absorb(&fragment) {
                let len = frame.len().min(out.len());
                out[..len].copy_from_slice(&frame[..len]);
                return Ok(len);
            }
        }
    }
}

pub struct GattSink {
    tx: GattTx,
}

impl BleSink for GattSink {
    type Error = BluerError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), BluerError> {
        let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
        for fragment in fragments_of(frame, GATT_FRAGMENT_PAYLOAD) {
            let n = fragment.encode(&mut buf).ok_or(BluerError::FrameTooLarge)?;
            match &mut self.tx {
                GattTx::Remote(remote) => {
                    remote.write(&buf[..n]).await?;
                }
                GattTx::Writer(writer) => {
                    writer.write_all(&buf[..n]).await?;
                    writer.flush().await?;
                }
            }
        }
        Ok(())
    }
}
