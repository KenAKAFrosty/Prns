use core::time::Duration;
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use bluer::adv::{Advertisement, AdvertisementHandle};
use bluer::gatt::local::{
    characteristic_control, service_control, Application, ApplicationHandle, Characteristic,
    CharacteristicControl, CharacteristicControlEvent, CharacteristicControlHandle,
    CharacteristicNotify, CharacteristicNotifyMethod, CharacteristicRead, CharacteristicWrite,
    CharacteristicWriteMethod, Service,
};
use bluer::gatt::remote::{Characteristic as RemoteCharacteristic, CharacteristicWriteRequest};
use bluer::gatt::{CharacteristicReader, CharacteristicWriter, WriteOp};
use bluer::l2cap::{
    Security, SecurityLevel, SeqPacket, SeqPacketListener, Socket, SocketAddr as L2capSocketAddr,
};
use bluer::{
    Adapter, AdapterEvent, Address, AddressType, Device, DeviceEvent, DeviceProperty,
    DiscoveryFilter, DiscoveryTransport, Session, Uuid,
};
use futures_util::stream::{FuturesUnordered, SelectAll};
use futures_util::{Stream, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

use prns_core::interfaces::bluetooth_auto::{
    columba_connection_role, columba_role_capabilities_from_manufacturer, default_group_tag,
    encode_stream_frame, fragments_of, manufacturer_discovery_groups_match,
    manufacturer_role_payload, BleAddress, BleIdentity, BleRoleCapabilities, BleUuid,
    ColumbaConnectionRole, Control, Fragment, L2capPlan, PeerProtocol, Psm, Reassembler,
    StreamDeframer, BLE_HW_MTU, BLE_SERVICE_UUID, COLUMBA_IDENTITY_UUID, COLUMBA_RX_UUID,
    COLUMBA_TX_UUID, CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN, NATIVE_CONTROL_UUID, NATIVE_DATA_UUID,
    STREAM_FRAME_PREFIX_LEN,
};
use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, DialOutcome, Origin,
    RadioMode, ScanningMode,
};

const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;

const GATT_FRAGMENT_PAYLOAD: usize = 180;
const GATT_REASSEMBLY_CAP: usize = 600;
const GATT_HALF_OPEN_TIMEOUT: Duration = super::runtime::HANDSHAKE_TIMEOUT;
const INBOUND_GATT_SETUP_TIMEOUT: Duration = Duration::from_secs(4);
const ADVERTISEMENT_CONSUMER_TTL: Duration = Duration::from_secs(60);

const SCAN_STOP_POLL: Duration = Duration::from_millis(20);
const SCAN_STOP_ATTEMPTS: usize = 25;

const RESWEEP_INTERVAL: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const L2CAP_UPGRADE_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_PLANE_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const DISCOVERY_DEGRADED_AFTER: Duration = Duration::from_secs(15);
const ADVERTISING_INTERVAL_MIN: Duration = Duration::from_millis(100);
const ADVERTISING_INTERVAL_MAX: Duration = Duration::from_millis(150);

const EATT_BLOCKED_REASON: &str = "BlueZ GATT Channels >1; set Channels=1";
const PRNS_DEVICE_NAME: &str = "Prns";

struct ResweepSchedule {
    due_at: Instant,
}

impl ResweepSchedule {
    fn new(now: Instant) -> Self {
        Self {
            due_at: now + RESWEEP_INTERVAL,
        }
    }

    fn restart(&mut self, now: Instant) {
        self.due_at = now + RESWEEP_INTERVAL;
    }
}

fn identifies_prns_fallback(
    name: Option<&str>,
    manufacturer_data: Option<&HashMap<u16, Vec<u8>>>,
) -> bool {
    name == Some(PRNS_DEVICE_NAME)
        && manufacturer_data.is_some_and(|data| {
            data.iter().any(|(company_id, value)| {
                columba_role_capabilities_from_manufacturer(*company_id, value).is_some()
            })
        })
}

fn matches_local_discovery_group(manufacturer_data: Option<&HashMap<u16, Vec<u8>>>) -> bool {
    let local = default_group_tag();
    match manufacturer_data.and_then(|data| data.get(&0xffff)) {
        Some(body) => manufacturer_discovery_groups_match(local, 0xffff, body),
        // No Prns manufacturer field → treat as the default open mesh.
        None => local == default_group_tag(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EattRisk {
    Safe,
    Risky,
}

fn gatt_channels_setting() -> Option<EattRisk> {
    let text = std::fs::read_to_string("/etc/bluetooth/main.conf").ok()?;
    gatt_channels_setting_from_str(&text)
}

fn gatt_channels_setting_from_str(text: &str) -> Option<EattRisk> {
    let mut in_gatt = false;
    let mut saw_channels = false;
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
                        saw_channels = true;
                        if n != 1 {
                            return Some(EattRisk::Risky);
                        }
                    }
                }
            }
        }
    }
    saw_channels.then_some(EattRisk::Safe)
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
        Some(EattRisk::Safe) => false,
        Some(EattRisk::Risky) => true,
        None => bluez_eatt_default_on(),
    }
}

#[derive(Debug)]
pub enum BluerError {
    Bluez(bluer::Error),
    Io(std::io::Error),
    NoControlCharacteristic,
    NoColumbaIdentity,
    MalformedColumbaIdentity,
    ControlPduTooLarge,
    MalformedControl,
    NotUpgraded,
    FrameTooLarge,
    DialTimeout,
    L2capTimeout,
    GattNotificationsEnded,
    GattWriteChannelEnded,
    GattDataReaderUnavailable,
    GattDataWriterUnavailable,
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

fn acknowledged_write() -> CharacteristicWriteRequest {
    CharacteristicWriteRequest {
        op_type: WriteOp::Request,
        ..CharacteristicWriteRequest::default()
    }
}

fn native_characteristic(
    uuid: Uuid,
    control_handle: CharacteristicControlHandle,
) -> Characteristic {
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

fn columba_rx_characteristic(
    uuid: Uuid,
    control_handle: CharacteristicControlHandle,
) -> Characteristic {
    Characteristic {
        uuid,
        write: Some(CharacteristicWrite {
            write: true,
            write_without_response: true,
            method: CharacteristicWriteMethod::Io,
            ..Default::default()
        }),
        control_handle,
        ..Default::default()
    }
}

fn columba_tx_characteristic(
    uuid: Uuid,
    control_handle: CharacteristicControlHandle,
) -> Characteristic {
    Characteristic {
        uuid,
        read: Some(CharacteristicRead {
            read: true,
            fun: Box::new(|_| Box::pin(async { Ok(Vec::new()) })),
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

fn columba_identity_characteristic(identity: BleIdentity) -> Characteristic {
    Characteristic {
        uuid: uuid_of(COLUMBA_IDENTITY_UUID),
        read: Some(CharacteristicRead {
            read: true,
            fun: Box::new(move |request| {
                let identity = identity;
                Box::pin(async move {
                    let start = usize::from(request.offset).min(identity.as_bytes().len());
                    Ok(identity.as_bytes()[start..].to_vec())
                })
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

struct PendingHalves {
    opened_at: Instant,
    reader: Option<CharacteristicReader>,
    writer: Option<CharacteristicWriter>,
}

struct PendingData {
    opened_at: Instant,
    writer: Option<CharacteristicWriter>,
    reader: Option<CharacteristicReader>,
}

struct AwaitingDataReader {
    sender: oneshot::Sender<CharacteristicReader>,
}

struct AwaitingDataWriter {
    sender: oneshot::Sender<CharacteristicWriter>,
}

impl PendingHalves {
    fn new() -> Self {
        Self {
            opened_at: Instant::now(),
            reader: None,
            writer: None,
        }
    }

    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.opened_at) >= GATT_HALF_OPEN_TIMEOUT
    }
}

impl PendingData {
    fn new() -> Self {
        Self {
            opened_at: Instant::now(),
            writer: None,
            reader: None,
        }
    }

    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.opened_at) >= GATT_HALF_OPEN_TIMEOUT
    }
}

fn can_admit_address<T>(entries: &HashMap<Address, T>, address: Address) -> bool {
    entries.contains_key(&address) || entries.len() < BluerBackend::MAX_PEERS
}

#[derive(Default)]
struct DiscoveryHealth {
    retry_at: Option<Instant>,
    failing_since: Option<Instant>,
    warned: bool,
}

struct AcceptRouter<S> {
    waiting: HashMap<Address, oneshot::Sender<S>>,
}

impl<S> AcceptRouter<S> {
    fn new() -> Self {
        Self {
            waiting: HashMap::new(),
        }
    }

    fn register(&mut self, address: Address) -> oneshot::Receiver<S> {
        let (tx, rx) = oneshot::channel();
        self.waiting.insert(address, tx);
        rx
    }

    fn deliver(&mut self, address: Address, socket: S) -> Result<(), S> {
        match self.waiting.remove(&address) {
            Some(tx) => tx.send(socket),
            None => Err(socket),
        }
    }

    fn cancel(&mut self, address: &Address) {
        self.waiting.remove(address);
    }
}

enum Half {
    Reader(CharacteristicReader),
    Writer(CharacteristicWriter),
}

enum GattAdmission {
    Pending,
    Ready(Box<AcceptedLink>),
    ActiveLinkUpdated,
    ActiveLinkEventIgnored,
    AtCapacity,
}

enum ServerDataAdmission {
    Ready(ServerData),
    AtCapacity,
}

enum DataRead {
    Ready(CharacteristicReader),
    Pending(oneshot::Receiver<CharacteristicReader>),
}

enum DataWrite {
    Ready(CharacteristicWriter),
    Pending(oneshot::Receiver<CharacteristicWriter>),
}

enum ServerData {
    TwoChar { writer: DataWrite, reader: DataRead },
    Columba,
}

type ConnectFuture = Pin<Box<dyn Future<Output = (Address, Result<BluerLink, BluerError>)> + Send>>;
type DeviceEventStream = Pin<Box<dyn Stream<Item = (Address, DeviceEvent)> + Send>>;

enum Observed {
    Candidate(Address),
    Greeting {
        protocol: PeerProtocol,
        address: Address,
        half: Half,
    },
    DataHalf {
        address: Address,
        half: Half,
    },
    DiscoveryEnded,
    GattServerEnded,
    L2capFailed(BluerError),
    DeviceAdded(Address),
    DeviceRemoved(Address),
    DeviceConnection {
        address: Address,
        state: DeviceConnectionState,
    },
    InboundSetupExpired(Address),
    AdapterEventsEnded,
    GattRetry,
    Connected(Address, Result<BluerLink, BluerError>),
    Resweep,
    Idle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeviceConnectionState {
    Disconnected,
    Connected,
}

impl DeviceConnectionState {
    fn from_connected(connected: bool) -> Self {
        if connected {
            Self::Connected
        } else {
            Self::Disconnected
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RadioPower {
    Off,
    On,
}

impl RadioPower {
    fn from_enabled(enabled: bool) -> Self {
        if enabled {
            Self::On
        } else {
            Self::Off
        }
    }

    fn is_on(self) -> bool {
        matches!(self, Self::On)
    }
}

#[derive(Debug, Eq, PartialEq)]
enum AdvertisementRegistration<H = AdvertisementHandle> {
    Unwanted,
    Wanted,
    Registered(H),
}

impl<H> AdvertisementRegistration<H> {
    const fn new() -> Self {
        Self::Unwanted
    }

    fn wants_airtime(&self) -> bool {
        matches!(self, Self::Wanted | Self::Registered(_))
    }

    fn is_registered(&self) -> bool {
        matches!(self, Self::Registered(_))
    }

    fn want(&mut self) {
        if matches!(self, Self::Unwanted) {
            *self = Self::Wanted;
        }
    }

    fn register(&mut self, handle: H) {
        if matches!(self, Self::Wanted | Self::Registered(_)) {
            *self = Self::Registered(handle);
        }
    }

    fn republish(&mut self) {
        if matches!(self, Self::Registered(_)) {
            *self = Self::Wanted;
        }
    }

    fn stop(&mut self) {
        *self = Self::Unwanted;
    }
}

struct AdvertisementConsumers {
    opened_at: HashMap<Address, Instant>,
}

impl AdvertisementConsumers {
    fn new() -> Self {
        Self {
            opened_at: HashMap::new(),
        }
    }

    fn begin(&mut self, address: Address, now: Instant) -> bool {
        if self.opened_at.contains_key(&address) || !can_admit_address(&self.opened_at, address) {
            return false;
        }
        self.opened_at.insert(address, now);
        true
    }

    fn prune(&mut self, now: Instant) {
        self.opened_at
            .retain(|_, opened_at| now.duration_since(*opened_at) < ADVERTISEMENT_CONSUMER_TTL);
    }

    fn close(&mut self, address: Address) -> bool {
        self.opened_at.remove(&address).is_some()
    }

    fn clear(&mut self) {
        self.opened_at.clear();
    }
}

struct ActiveLinkGenerations {
    counts: HashMap<Address, usize>,
}

enum LinkClosure {
    LastActiveGeneration,
    SupersededGeneration,
    Untracked,
}

impl ActiveLinkGenerations {
    fn new() -> Self {
        Self {
            counts: HashMap::new(),
        }
    }

    fn contains(&self, address: Address) -> bool {
        self.counts.contains_key(&address)
    }

    fn activate(&mut self, address: Address) {
        *self.counts.entry(address).or_insert(0) += 1;
    }

    fn close(&mut self, address: Address) -> LinkClosure {
        match self.counts.entry(address) {
            Entry::Occupied(mut entry) if *entry.get() > 1 => {
                *entry.get_mut() -= 1;
                LinkClosure::SupersededGeneration
            }
            Entry::Occupied(entry) => {
                entry.remove();
                LinkClosure::LastActiveGeneration
            }
            Entry::Vacant(_) => LinkClosure::Untracked,
        }
    }

    fn clear(&mut self) {
        self.counts.clear();
    }
}

pub struct BluerBackend {
    adapter: Adapter,
    address: Address,
    address_type: AddressType,
    psm: Psm,
    identity: BleIdentity,
    group_tag: [u8; 4],
    radio_power: RadioPower,
    connecting: HashSet<Address>,
    active_links: ActiveLinkGenerations,
    connected_devices: HashSet<Address>,
    connects: FuturesUnordered<ConnectFuture>,
    pending: HashMap<Address, PendingHalves>,
    scan_enabled: bool,
    resweep_next: usize,
    resweep_schedule: ResweepSchedule,
    adapter_events: Option<Pin<Box<dyn Stream<Item = AdapterEvent> + Send>>>,
    device_events: SelectAll<DeviceEventStream>,
    observed_devices: HashSet<Address>,
    discovery: Option<Pin<Box<dyn Stream<Item = AdapterEvent> + Send>>>,
    discovery_health: DiscoveryHealth,
    control: Option<Pin<Box<CharacteristicControl>>>,
    data_control: Option<Pin<Box<CharacteristicControl>>>,
    columba_rx_control: Option<Pin<Box<CharacteristicControl>>>,
    columba_tx_control: Option<Pin<Box<CharacteristicControl>>>,
    pending_columba: HashMap<Address, PendingHalves>,
    pending_data: HashMap<Address, PendingData>,
    awaiting_data_reader: HashMap<Address, AwaitingDataReader>,
    awaiting_data_writer: HashMap<Address, AwaitingDataWriter>,
    listener: Option<Arc<SeqPacketListener>>,
    l2cap_acceptor: Option<tokio::task::JoinHandle<()>>,
    l2cap_failure: Option<oneshot::Receiver<BluerError>>,
    l2cap_router: Arc<std::sync::Mutex<AcceptRouter<SeqPacket>>>,
    advertisement: AdvertisementRegistration,
    advertisement_consumers: AdvertisementConsumers,
    inbound_setups: HashMap<Address, Instant>,
    gatt_retry_at: Option<Instant>,
    _application: Option<ApplicationHandle>,
    blocked: Option<&'static str>,
}

impl Drop for BluerBackend {
    fn drop(&mut self) {
        if let Some(acceptor) = self.l2cap_acceptor.take() {
            acceptor.abort();
        }
    }
}

impl BluerBackend {
    pub const MAX_PEERS: usize = 8;

    pub async fn open(psm: Psm, identity: BleIdentity, group_tag: [u8; 4]) -> Result<Self, BluerError> {
        let session = Session::new().await?;
        let adapter = session.default_adapter().await?;
        adapter.set_powered(true).await?;
        let address = adapter.address().await?;
        let address_type = adapter.address_type().await?;
        let adapter_events = Box::pin(adapter.events().await?);
        let blocked = if eatt_is_risky() {
            crate::diagnostic_log::error!(
                "bluetooth: NOT starting — BlueZ Enhanced ATT (EATT) is enabled, so nearby \
                 peers can show pairing prompts (EATT requires an encrypted link). This will not \
                 resolve on its own. Edit /etc/bluetooth/main.conf so it has one active [GATT] \
                 section with Channels = 1, then restart bluetoothd with `sudo systemctl restart \
                 bluetooth`. Channels=1 is the upstream BlueZ default on 5.67+; on those versions \
                 BLE starts with no action."
            );
            Some(EATT_BLOCKED_REASON)
        } else {
            None
        };
        let mut backend = Self {
            adapter,
            address,
            address_type,
            psm,
            identity,
            group_tag,
            radio_power: RadioPower::Off,
            connecting: HashSet::new(),
            active_links: ActiveLinkGenerations::new(),
            connected_devices: HashSet::new(),
            connects: FuturesUnordered::new(),
            pending: HashMap::new(),
            scan_enabled: false,
            resweep_next: 0,
            resweep_schedule: ResweepSchedule::new(Instant::now()),
            adapter_events: Some(adapter_events),
            device_events: SelectAll::new(),
            observed_devices: HashSet::new(),
            discovery: None,
            discovery_health: DiscoveryHealth::default(),
            control: None,
            data_control: None,
            columba_rx_control: None,
            columba_tx_control: None,
            pending_columba: HashMap::new(),
            pending_data: HashMap::new(),
            awaiting_data_reader: HashMap::new(),
            awaiting_data_writer: HashMap::new(),
            listener: None,
            l2cap_acceptor: None,
            l2cap_failure: None,
            l2cap_router: Arc::new(std::sync::Mutex::new(AcceptRouter::new())),
            advertisement: AdvertisementRegistration::new(),
            advertisement_consumers: AdvertisementConsumers::new(),
            inbound_setups: HashMap::new(),
            gatt_retry_at: None,
            _application: None,
            blocked,
        };
        for address in backend.adapter.device_addresses().await? {
            let _ = backend.observe_device(address).await;
        }
        Ok(backend)
    }

    async fn observe_device(
        &mut self,
        address: Address,
    ) -> Result<DeviceConnectionState, BluerError> {
        if self.observed_devices.contains(&address) {
            let device = self.adapter.device(address)?;
            return Ok(DeviceConnectionState::from_connected(
                device.is_connected().await?,
            ));
        }
        let device = self.adapter.device(address)?;
        let events = device.events().await?;
        self.device_events
            .push(Box::pin(events.map(move |event| (address, event))));
        self.observed_devices.insert(address);
        Ok(DeviceConnectionState::from_connected(
            device.is_connected().await?,
        ))
    }

    async fn advertises_our_service(&self, address: Address) -> bool {
        let Ok(device) = self.adapter.device(address) else {
            return false;
        };
        if device
            .uuids()
            .await
            .ok()
            .flatten()
            .is_some_and(|uuids| uuids.contains(&uuid_of(BLE_SERVICE_UUID)))
        {
            return true;
        }
        let name = device.name().await.ok().flatten();
        let manufacturer_data = device.manufacturer_data().await.ok().flatten();
        identifies_prns_fallback(name.as_deref(), manufacturer_data.as_ref())
    }

    async fn release_stale_prns_links(&self) {
        let Ok(addresses) = self.adapter.device_addresses().await else {
            return;
        };
        for address in addresses {
            if address == self.address {
                continue;
            }
            let Ok(device) = self.adapter.device(address) else {
                continue;
            };
            if !device.is_connected().await.unwrap_or(false)
                || !self.advertises_our_service(address).await
            {
                continue;
            }
            match self.adapter.remove_device(address).await {
                Ok(()) => crate::diagnostic_log::debug!(
                    "bluetooth: released stale Prns link {address} from the previous process generation"
                ),
                Err(error) => crate::diagnostic_log::warn!(
                    "bluetooth: stale Prns link {address} could not be released: {error}"
                ),
            }
        }
    }

    async fn should_dial(&self, address: Address) -> bool {
        let Ok(device) = self.adapter.device(address) else {
            return true;
        };
        let manufacturer_data = device.manufacturer_data().await.ok().flatten();
        if !matches_local_discovery_group(self.group_tag, manufacturer_data.as_ref()) {
            return false;
        }
        let peer_capabilities = manufacturer_data.as_ref().and_then(|data| {
            data.iter().find_map(|(company_id, value)| {
                columba_role_capabilities_from_manufacturer(*company_id, value)
            })
        });
        columba_connection_role(
            BleAddress::new(self.address.0),
            BleRoleCapabilities::DualRole,
            BleAddress::new(address.0),
            peer_capabilities.unwrap_or(BleRoleCapabilities::DualRole),
        ) == ColumbaConnectionRole::Dial
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
            if self.advertises_our_service(address).await && self.should_dial(address).await {
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

    fn advertisement(group_tag: [u8; 4]) -> Advertisement {
        let payload = manufacturer_role_payload(BleRoleCapabilities::DualRole, group_tag);
        Advertisement {
            advertisement_type: bluer::adv::Type::Peripheral,
            service_uuids: [uuid_of(BLE_SERVICE_UUID)].into_iter().collect(),
            manufacturer_data: [(0xffff, payload.to_vec())].into_iter().collect(),
            discoverable: Some(true),
            min_interval: Some(ADVERTISING_INTERVAL_MIN),
            max_interval: Some(ADVERTISING_INTERVAL_MAX),
            ..Default::default()
        }
    }

    fn l2cap_security() -> Security {
        Security {
            level: SecurityLevel::Low,
            key_size: 0,
        }
    }

    fn bind_l2cap_listener(&self) -> Result<SeqPacketListener, BluerError> {
        let socket = Socket::<SeqPacket>::new_seq_packet()?;
        socket.set_security(Self::l2cap_security())?;
        socket.bind(L2capSocketAddr::new(
            self.address,
            self.address_type,
            self.psm.get(),
        ))?;
        Ok(socket.listen(1)?)
    }

    async fn reconcile_advertisement(&mut self) -> Result<(), BluerError> {
        if !self.radio_power.is_on() || !self.advertisement.wants_airtime() {
            return Ok(());
        }
        if self.advertisement.is_registered() {
            return Ok(());
        }
        if let Some(retry_at) = self.gatt_retry_at {
            if Instant::now() < retry_at {
                return Ok(());
            }
            self.gatt_retry_at = None;
        }
        self.ensure_gatt_server().await?;
        let advertisement = self.adapter.advertise(Self::advertisement(self.group_tag)).await?;
        self.advertisement.register(advertisement);
        self.gatt_retry_at = None;
        crate::diagnostic_log::debug!(
            "bluetooth: advertising Reticulum BLE, control PSM {:#x}, listener {}",
            self.psm.get(),
            if self.listener.is_some() {
                "bound"
            } else {
                "unavailable"
            },
        );
        Ok(())
    }

    async fn ensure_gatt_server(&mut self) -> Result<(), BluerError> {
        if self.control.is_some() {
            return Ok(());
        }
        let (control, control_handle) = characteristic_control();
        let (data, data_handle) = characteristic_control();
        let (columba_rx, columba_rx_handle) = characteristic_control();
        let (columba_tx, columba_tx_handle) = characteristic_control();
        let (_service_control, service_handle) = service_control();
        let application = Application {
            services: vec![Service {
                uuid: uuid_of(BLE_SERVICE_UUID),
                primary: true,
                characteristics: vec![
                    native_characteristic(uuid_of(NATIVE_CONTROL_UUID), control_handle),
                    native_characteristic(uuid_of(NATIVE_DATA_UUID), data_handle),
                    columba_rx_characteristic(uuid_of(COLUMBA_RX_UUID), columba_rx_handle),
                    columba_tx_characteristic(uuid_of(COLUMBA_TX_UUID), columba_tx_handle),
                    columba_identity_characteristic(self.identity),
                ],
                control_handle: service_handle,
                ..Default::default()
            }],
            ..Default::default()
        };
        let application = self.adapter.serve_gatt_application(application).await?;

        let (listener, l2cap_acceptor, l2cap_failure) = match self.bind_l2cap_listener() {
            Ok(listener) => {
                let listener = Arc::new(listener);
                let (acceptor, failure) =
                    spawn_l2cap_acceptor(Arc::clone(&listener), Arc::clone(&self.l2cap_router));
                (Some(listener), Some(acceptor), Some(failure))
            }
            Err(error) => {
                crate::diagnostic_log::warn!(
                    "bluetooth: L2CAP listener unavailable: {error:?}; GATT floor only"
                );
                (None, None, None)
            }
        };

        self.control = Some(Box::pin(control));
        self.data_control = Some(Box::pin(data));
        self.columba_rx_control = Some(Box::pin(columba_rx));
        self.columba_tx_control = Some(Box::pin(columba_tx));
        self.listener = listener;
        self.l2cap_acceptor = l2cap_acceptor;
        self.l2cap_failure = l2cap_failure;
        self._application = Some(application);
        Ok(())
    }

    fn stop_gatt_server(&mut self) {
        self._application = None;
        self.control = None;
        self.data_control = None;
        self.columba_rx_control = None;
        self.columba_tx_control = None;
        self.pending.clear();
        self.pending_columba.clear();
        self.pending_data.clear();
        self.awaiting_data_reader.clear();
        self.awaiting_data_writer.clear();
        self.advertisement_consumers.clear();
        self.inbound_setups.clear();
        if let Some(acceptor) = self.l2cap_acceptor.take() {
            acceptor.abort();
        }
        self.l2cap_failure = None;
        if let Ok(mut router) = self.l2cap_router.lock() {
            *router = AcceptRouter::new();
        }
        self.listener = None;
    }

    fn stop_radio_resources(&mut self) {
        self.scan_enabled = false;
        self.discovery = None;
        self.discovery_health = DiscoveryHealth::default();
        self.advertisement.stop();
        self.stop_gatt_server();
        self.connecting.clear();
        self.active_links.clear();
        self.connected_devices.clear();
        self.connects = FuturesUnordered::new();
        self.resweep_next = 0;
        self.gatt_retry_at = None;
    }

    fn prune_half_open_gatt(&mut self, now: Instant) {
        self.pending.retain(|_, pending| !pending.expired(now));
        self.pending_columba
            .retain(|_, pending| !pending.expired(now));
        self.pending_data.retain(|_, pending| !pending.expired(now));
        self.advertisement_consumers.prune(now);
    }

    fn begin_inbound_setup(&mut self, address: Address) {
        if !self.connecting.contains(&address) && can_admit_address(&self.inbound_setups, address) {
            self.inbound_setups
                .entry(address)
                .or_insert_with(|| Instant::now() + INBOUND_GATT_SETUP_TIMEOUT);
        }
    }

    fn begin_physical_connection(&mut self, address: Address) {
        if self.connected_devices.insert(address) {
            self.begin_inbound_setup(address);
        }
    }

    fn end_physical_connection(&mut self, address: Address) -> bool {
        self.connected_devices.remove(&address);
        self.clear_inbound_state(address)
    }

    fn clear_inbound_state(&mut self, address: Address) -> bool {
        self.pending.remove(&address);
        self.pending_columba.remove(&address);
        self.pending_data.remove(&address);
        self.awaiting_data_reader.remove(&address);
        self.awaiting_data_writer.remove(&address);
        self.inbound_setups.remove(&address);
        if let Ok(mut router) = self.l2cap_router.lock() {
            router.cancel(&address);
        }
        self.advertisement_consumers.close(address)
    }

    fn recover_gatt_server(&mut self) {
        let resume_advertising = self.advertisement.wants_airtime();
        self.advertisement.stop();
        self.stop_gatt_server();
        if resume_advertising {
            self.advertisement.want();
        }
    }

    async fn republish_advertisement(&mut self) {
        self.advertisement.republish();
        if let Err(error) = self.reconcile_advertisement().await {
            self.gatt_retry_at
                .get_or_insert(Instant::now() + CONTROL_PLANE_RETRY_INTERVAL);
            crate::diagnostic_log::warn!(
                "bluetooth: advertising restart after inbound connection failed: {error:?}"
            );
        }
    }

    async fn consume_advertisement(&mut self, address: Address) {
        if self.advertisement_consumers.begin(address, Instant::now()) {
            self.republish_advertisement().await;
        }
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

    async fn poll_discovery(&mut self) {
        let now = Instant::now();
        if self.discovery_health.retry_at.is_some_and(|at| now < at) {
            return;
        }
        match self.start_discovery().await {
            Ok(()) => {
                if self.discovery_health.warned {
                    crate::diagnostic_log::debug!("bluetooth: scanning resumed");
                }
                self.discovery_health = DiscoveryHealth::default();
            }
            Err(error) => {
                self.discovery_health.retry_at = Some(now + CONTROL_PLANE_RETRY_INTERVAL);
                let failing_since = *self.discovery_health.failing_since.get_or_insert(now);
                if !self.discovery_health.warned
                    && now.duration_since(failing_since) >= DISCOVERY_DEGRADED_AFTER
                {
                    crate::diagnostic_log::warn!(
                        "bluetooth: discovery unavailable for {}s ({error:?}); sightings now rely on \
                         the periodic resweep. The adapter likely has stuck discovery state from an \
                         unclean exit or a co-resident scanner — reset it with `bluetoothctl power \
                         off` then `power on` if peers stop being found.",
                        now.duration_since(failing_since).as_secs()
                    );
                    self.discovery_health.warned = true;
                }
            }
        }
    }

    fn admit_greeting(
        &mut self,
        protocol: PeerProtocol,
        address: Address,
        half: Half,
    ) -> GattAdmission {
        if self.active_links.contains(address) && !self.inbound_setups.contains_key(&address) {
            return GattAdmission::ActiveLinkEventIgnored;
        }
        let pending = match protocol {
            PeerProtocol::Native => &mut self.pending,
            PeerProtocol::Columba => &mut self.pending_columba,
        };
        if !can_admit_address(pending, address) {
            return GattAdmission::AtCapacity;
        }
        let entry = pending.entry(address).or_insert_with(PendingHalves::new);
        match half {
            Half::Reader(reader) => entry.reader = Some(reader),
            Half::Writer(writer) => entry.writer = Some(writer),
        }
        self.take_ready_accepted_link(protocol, address)
    }

    fn take_ready_accepted_link(
        &mut self,
        protocol: PeerProtocol,
        address: Address,
    ) -> GattAdmission {
        let pending = match protocol {
            PeerProtocol::Native => &mut self.pending,
            PeerProtocol::Columba => &mut self.pending_columba,
        };
        if !pending
            .get(&address)
            .is_some_and(|entry| entry.reader.is_some() && entry.writer.is_some())
        {
            return GattAdmission::Pending;
        }
        let data = match protocol {
            PeerProtocol::Native => match self.take_server_data(address) {
                ServerDataAdmission::Ready(data) => data,
                ServerDataAdmission::AtCapacity => return GattAdmission::AtCapacity,
            },
            PeerProtocol::Columba => ServerData::Columba,
        };
        let pending = match protocol {
            PeerProtocol::Native => &mut self.pending,
            PeerProtocol::Columba => &mut self.pending_columba,
        };
        let Some(PendingHalves {
            reader: Some(reader),
            writer: Some(writer),
            ..
        }) = pending.remove(&address)
        else {
            return GattAdmission::Pending;
        };
        let l2cap = match protocol {
            PeerProtocol::Native => self.register_l2cap(address),
            PeerProtocol::Columba => None,
        };
        self.active_links.activate(address);
        GattAdmission::Ready(Box::new(AcceptedLink {
            peer_protocol: protocol,
            reader,
            writer,
            address,
            l2cap,
            socket: None,
            data,
        }))
    }

    fn register_l2cap(&mut self, address: Address) -> Option<oneshot::Receiver<SeqPacket>> {
        self.listener.as_ref()?;
        let mut router = self.l2cap_router.lock().ok()?;
        Some(router.register(address))
    }

    fn take_server_data(&mut self, address: Address) -> ServerDataAdmission {
        let (needs_writer, needs_reader) = match self.pending_data.get(&address) {
            Some(pending) => (pending.writer.is_none(), pending.reader.is_none()),
            None => (true, true),
        };
        if needs_writer && !can_admit_address(&self.awaiting_data_writer, address) {
            return ServerDataAdmission::AtCapacity;
        }
        if needs_reader && !can_admit_address(&self.awaiting_data_reader, address) {
            return ServerDataAdmission::AtCapacity;
        }
        let mut data = self
            .pending_data
            .remove(&address)
            .unwrap_or_else(PendingData::new);
        let writer = data.writer.take();
        let reader = data.reader.take();
        let writer = match writer {
            Some(writer) => DataWrite::Ready(writer),
            None => {
                let (tx, rx) = oneshot::channel();
                self.awaiting_data_writer
                    .insert(address, AwaitingDataWriter { sender: tx });
                DataWrite::Pending(rx)
            }
        };
        let reader = match reader {
            Some(reader) => DataRead::Ready(reader),
            None => {
                let (tx, rx) = oneshot::channel();
                self.awaiting_data_reader
                    .insert(address, AwaitingDataReader { sender: tx });
                DataRead::Pending(rx)
            }
        };
        ServerDataAdmission::Ready(ServerData::TwoChar { writer, reader })
    }

    fn admit_data_half(&mut self, address: Address, half: Half) -> GattAdmission {
        if self.active_links.contains(address) && !self.inbound_setups.contains_key(&address) {
            return match half {
                Half::Writer(writer) => match self.awaiting_data_writer.remove(&address) {
                    Some(pending) => match pending.sender.send(writer) {
                        Ok(()) => GattAdmission::ActiveLinkUpdated,
                        Err(_) => GattAdmission::ActiveLinkEventIgnored,
                    },
                    None => GattAdmission::ActiveLinkEventIgnored,
                },
                Half::Reader(reader) => match self.awaiting_data_reader.remove(&address) {
                    Some(pending) => match pending.sender.send(reader) {
                        Ok(()) => GattAdmission::ActiveLinkUpdated,
                        Err(_) => GattAdmission::ActiveLinkEventIgnored,
                    },
                    None => GattAdmission::ActiveLinkEventIgnored,
                },
            };
        }
        match half {
            Half::Writer(writer) => {
                if !can_admit_address(&self.pending_data, address) {
                    return GattAdmission::AtCapacity;
                }
                self.pending_data
                    .entry(address)
                    .or_insert_with(PendingData::new)
                    .writer = Some(writer);
            }
            Half::Reader(reader) => {
                if !can_admit_address(&self.pending_data, address) {
                    return GattAdmission::AtCapacity;
                }
                self.pending_data
                    .entry(address)
                    .or_insert_with(PendingData::new)
                    .reader = Some(reader);
            }
        }
        self.take_ready_accepted_link(PeerProtocol::Native, address)
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

async fn retry_or_pending(retry_at: Option<Instant>) {
    match retry_at {
        Some(retry_at) => tokio::time::sleep_until(retry_at.into()).await,
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

async fn next_device_event(events: &mut SelectAll<DeviceEventStream>) -> (Address, DeviceEvent) {
    match events.next().await {
        Some(event) => event,
        None => core::future::pending().await,
    }
}

async fn next_inbound_setup_expiration(setups: &HashMap<Address, Instant>) -> Address {
    match setups.iter().min_by_key(|(_, deadline)| *deadline) {
        Some((&address, &deadline)) => {
            tokio::time::sleep_until(deadline.into()).await;
            address
        }
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

fn spawn_l2cap_acceptor(
    listener: Arc<SeqPacketListener>,
    router: Arc<std::sync::Mutex<AcceptRouter<SeqPacket>>>,
) -> (tokio::task::JoinHandle<()>, oneshot::Receiver<BluerError>) {
    let (failure_tx, failure_rx) = oneshot::channel();
    let acceptor = tokio::spawn(async move {
        loop {
            let (socket, peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    let _ = failure_tx.send(error.into());
                    return;
                }
            };
            let delivered = match router.lock() {
                Ok(mut router) => router.deliver(peer.addr, socket).is_ok(),
                Err(_) => false,
            };
            if !delivered {
                crate::diagnostic_log::debug!(
                    "bluetooth: inbound L2CAP CoC from {} had no waiting link; dropped",
                    peer.addr
                );
            }
        }
    });
    (acceptor, failure_rx)
}

async fn next_l2cap_failure(failure: Option<&mut oneshot::Receiver<BluerError>>) -> BluerError {
    match failure {
        Some(failure) => failure.await.unwrap_or(BluerError::Closed),
        None => core::future::pending().await,
    }
}

async fn connect_link(adapter: Adapter, target: Address) -> Result<BluerLink, BluerError> {
    let discovered = adapter.device(target)?;
    let peer_address_type = discovered.address_type().await?;
    crate::diagnostic_log::debug!("bluetooth: dialing {target} over LE ({peer_address_type:?})");
    let _ = adapter.remove_device(target).await;
    await_scan_stopped(&adapter).await;
    let device = match adapter.connect_device(target, peer_address_type).await {
        Ok(device) => device,
        Err(error) => {
            crate::diagnostic_log::warn!("bluetooth: LE connect to {target} failed: {error}");
            return Err(error.into());
        }
    };
    let native_control = find_characteristic(&device, uuid_of(NATIVE_CONTROL_UUID)).await?;
    let (peer_protocol, control, notify, data, data_notify, peer_identity) = if let Some(control) =
        native_control
    {
        let data = find_characteristic(&device, uuid_of(NATIVE_DATA_UUID))
            .await
            .ok()
            .flatten();
        let notify = control.notify().await?;
        let data_notify = match &data {
            Some(data) => match data.notify().await {
                Ok(stream) => Some(Box::pin(stream) as Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>),
                Err(error) => {
                    crate::diagnostic_log::warn!(
                        "bluetooth: {target} data characteristic notify failed: {error}"
                    );
                    None
                }
            },
            None => None,
        };
        (
            PeerProtocol::Native,
            control,
            Box::pin(notify) as Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
            data,
            data_notify,
            None,
        )
    } else {
        let rx = find_characteristic(&device, uuid_of(COLUMBA_RX_UUID))
            .await?
            .ok_or(BluerError::NoControlCharacteristic)?;
        let tx = find_characteristic(&device, uuid_of(COLUMBA_TX_UUID))
            .await?
            .ok_or(BluerError::NoControlCharacteristic)?;
        let identity = find_characteristic(&device, uuid_of(COLUMBA_IDENTITY_UUID))
            .await?
            .ok_or(BluerError::NoColumbaIdentity)?;
        let identity = identity.read().await?;
        let identity: [u8; 16] = identity
            .try_into()
            .map_err(|_| BluerError::MalformedColumbaIdentity)?;
        let notify = tx.notify().await?;
        (
            PeerProtocol::Columba,
            rx,
            Box::pin(notify) as Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
            None,
            None,
            Some(BleIdentity::new(identity)),
        )
    };
    crate::diagnostic_log::debug!(
        "bluetooth: {target} connected over LE with {peer_protocol:?}; handshaking"
    );
    Ok(BluerLink::Dialed(Box::new(DialedLink {
        peer_protocol,
        peer_identity,
        control,
        notify,
        data,
        data_notify,
        peer_address: target,
        peer_address_type,
        socket: None,
        _device: device,
    })))
}

impl BleBackend<{ BluerBackend::MAX_PEERS }> for BluerBackend {
    type Error = BluerError;
    type Link = BluerLink;

    fn blocked(&self) -> Option<&'static str> {
        self.blocked
    }

    async fn set_radio_mode(&mut self, mode: RadioMode) -> Result<(), BluerError> {
        let enabled = mode.is_on();
        let power = RadioPower::from_enabled(enabled);
        if self.radio_power == power {
            return if enabled {
                self.reconcile_advertisement().await
            } else {
                Ok(())
            };
        }
        if !enabled {
            self.radio_power = RadioPower::Off;
            self.stop_radio_resources();
            crate::diagnostic_log::debug!("bluetooth: Linux BLE radio resources down");
            return Ok(());
        }
        self.adapter.set_powered(true).await?;
        self.release_stale_prns_links().await;
        self.radio_power = RadioPower::On;
        crate::diagnostic_log::debug!("bluetooth: Linux BLE radio resources up");
        self.reconcile_advertisement().await
    }

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), BluerError> {
        let enabled = mode.is_on();
        if !enabled {
            self.advertisement.stop();
            crate::diagnostic_log::debug!("bluetooth: advertising off");
            return Ok(());
        }
        self.advertisement.want();
        self.reconcile_advertisement().await
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), BluerError> {
        let enabled = mode.is_on();
        if enabled && !self.scan_enabled {
            self.resweep_schedule.restart(Instant::now());
        }
        self.scan_enabled = enabled;
        if !enabled || !self.radio_power.is_on() {
            self.discovery = None;
            if !enabled {
                crate::diagnostic_log::debug!("bluetooth: scanning off");
            }
        } else {
            crate::diagnostic_log::debug!("bluetooth: scanning LE for Prns peers");
        }
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<BluerLink> {
        loop {
            self.prune_half_open_gatt(Instant::now());
            if let Err(error) = self.reconcile_advertisement().await {
                self.gatt_retry_at
                    .get_or_insert(Instant::now() + CONTROL_PLANE_RETRY_INTERVAL);
                crate::diagnostic_log::warn!("bluetooth: advertising unavailable: {error:?}");
            }
            let want_discovery =
                self.radio_power.is_on() && self.scan_enabled && self.connecting.is_empty();
            if want_discovery && self.discovery.is_none() {
                self.poll_discovery().await;
            } else if !want_discovery && self.discovery.is_some() {
                self.discovery = None;
                self.discovery_health.retry_at = None;
            }
            let observed = {
                let discovery = self.discovery.as_mut();
                let adapter_events = self.adapter_events.as_mut();
                let device_events = &mut self.device_events;
                let inbound_setups = &self.inbound_setups;
                let control = self.control.as_mut();
                let data_control = self.data_control.as_mut();
                let columba_rx_control = self.columba_rx_control.as_mut();
                let columba_tx_control = self.columba_tx_control.as_mut();
                let l2cap_failure = self.l2cap_failure.as_mut();
                let connects = &mut self.connects;
                let gatt_retry_at = self.gatt_retry_at;
                let resweep_at = self.resweep_schedule.due_at;
                tokio::select! {
                    event = next_or_pending(adapter_events) => match event {
                        Some(AdapterEvent::DeviceAdded(address)) => Observed::DeviceAdded(address),
                        Some(AdapterEvent::DeviceRemoved(address)) => Observed::DeviceRemoved(address),
                        Some(AdapterEvent::PropertyChanged(_)) => Observed::Idle,
                        None => Observed::AdapterEventsEnded,
                    },
                    (address, event) = next_device_event(device_events) => match event {
                        DeviceEvent::PropertyChanged(DeviceProperty::Connected(connected)) => {
                            Observed::DeviceConnection {
                                address,
                                state: DeviceConnectionState::from_connected(connected),
                            }
                        }
                        _ => Observed::Idle,
                    },
                    address = next_inbound_setup_expiration(inbound_setups) => {
                        Observed::InboundSetupExpired(address)
                    },
                    event = next_or_pending(discovery) => match event {
                        Some(AdapterEvent::DeviceAdded(address)) => Observed::Candidate(address),
                        Some(_) => Observed::Idle,
                        None => Observed::DiscoveryEnded,
                    },
                    event = next_or_pending(control) => match event {
                        Some(CharacteristicControlEvent::Write(request)) => {
                            let address = request.device_address();
                            match request.accept() {
                                Ok(reader) => Observed::Greeting {
                                    protocol: PeerProtocol::Native,
                                    address,
                                    half: Half::Reader(reader),
                                },
                                Err(_) => Observed::Idle,
                            }
                        }
                        Some(CharacteristicControlEvent::Notify(writer)) => Observed::Greeting {
                            protocol: PeerProtocol::Native,
                            address: writer.device_address(),
                            half: Half::Writer(writer),
                        },
                        None => Observed::GattServerEnded,
                    },
                    event = next_or_pending(columba_rx_control) => match event {
                        Some(CharacteristicControlEvent::Write(request)) => {
                            let address = request.device_address();
                            match request.accept() {
                                Ok(reader) => Observed::Greeting {
                                    protocol: PeerProtocol::Columba,
                                    address,
                                    half: Half::Reader(reader),
                                },
                                Err(_) => Observed::Idle,
                            }
                        }
                        Some(_) => Observed::Idle,
                        None => Observed::GattServerEnded,
                    },
                    event = next_or_pending(columba_tx_control) => match event {
                        Some(CharacteristicControlEvent::Notify(writer)) => Observed::Greeting {
                            protocol: PeerProtocol::Columba,
                            address: writer.device_address(),
                            half: Half::Writer(writer),
                        },
                        Some(_) => Observed::Idle,
                        None => Observed::GattServerEnded,
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
                        None => Observed::GattServerEnded,
                    },
                    (target, result) = next_connect(connects) => Observed::Connected(target, result),
                    failure = next_l2cap_failure(l2cap_failure) => Observed::L2capFailed(failure),
                    () = retry_or_pending(gatt_retry_at) => Observed::GattRetry,
                    () = tokio::time::sleep_until(resweep_at.into()), if want_discovery => Observed::Resweep,
                }
            };
            match observed {
                Observed::Candidate(address) => {
                    let mine = address == self.address;
                    let dialing = self.connecting.contains(&address);
                    if !mine
                        && !dialing
                        && self.advertises_our_service(address).await
                        && self.should_dial(address).await
                    {
                        let rssi = self.peer_rssi(address).await;
                        crate::diagnostic_log::debug!("bluetooth: sighted Prns peer {address}");
                        return BleEvent::Sighting {
                            address: BleAddress::new(address.0),
                            rssi,
                        };
                    }
                }
                Observed::Greeting {
                    protocol,
                    address,
                    half,
                } => {
                    self.consume_advertisement(address).await;
                    self.prune_half_open_gatt(Instant::now());
                    match self.admit_greeting(protocol, address, half) {
                        GattAdmission::Ready(link) => {
                            self.inbound_setups.remove(&address);
                            let peer_rssi = self.peer_rssi(address).await;
                            crate::diagnostic_log::debug!("bluetooth: inbound link from {address}");
                            return BleEvent::LinkReady {
                                link: BluerLink::Accepted(link),
                                origin: Origin::Accepted,
                                peer_rssi,
                            };
                        }
                        GattAdmission::ActiveLinkUpdated => {
                            self.inbound_setups.remove(&address);
                        }
                        GattAdmission::Pending
                        | GattAdmission::ActiveLinkEventIgnored
                        | GattAdmission::AtCapacity => {}
                    }
                }
                Observed::DataHalf { address, half } => {
                    self.consume_advertisement(address).await;
                    self.prune_half_open_gatt(Instant::now());
                    match self.admit_data_half(address, half) {
                        GattAdmission::Ready(link) => {
                            self.inbound_setups.remove(&address);
                            let peer_rssi = self.peer_rssi(address).await;
                            crate::diagnostic_log::debug!("bluetooth: inbound link from {address}");
                            return BleEvent::LinkReady {
                                link: BluerLink::Accepted(link),
                                origin: Origin::Accepted,
                                peer_rssi,
                            };
                        }
                        GattAdmission::ActiveLinkUpdated => {
                            self.inbound_setups.remove(&address);
                        }
                        GattAdmission::Pending
                        | GattAdmission::ActiveLinkEventIgnored
                        | GattAdmission::AtCapacity => {}
                    }
                }
                Observed::DiscoveryEnded => {
                    let now = Instant::now();
                    self.discovery = None;
                    self.discovery_health.retry_at = Some(now + CONTROL_PLANE_RETRY_INTERVAL);
                    self.discovery_health.failing_since.get_or_insert(now);
                }
                Observed::GattServerEnded => {
                    self.recover_gatt_server();
                    self.gatt_retry_at = Some(Instant::now() + CONTROL_PLANE_RETRY_INTERVAL);
                }
                Observed::L2capFailed(error) => {
                    crate::diagnostic_log::debug!("bluetooth: L2CAP accept loop ended: {error:?}");
                    self.recover_gatt_server();
                    self.gatt_retry_at = Some(Instant::now() + CONTROL_PLANE_RETRY_INTERVAL);
                }
                Observed::DeviceAdded(address) => match self.observe_device(address).await {
                    Ok(DeviceConnectionState::Connected) => {
                        self.begin_physical_connection(address);
                        self.consume_advertisement(address).await;
                    }
                    Ok(DeviceConnectionState::Disconnected) => {}
                    Err(error) => crate::diagnostic_log::warn!(
                        "bluetooth: could not observe {address} connection state: {error:?}"
                    ),
                },
                Observed::DeviceRemoved(address) => {
                    self.observed_devices.remove(&address);
                    self.end_physical_connection(address);
                }
                Observed::DeviceConnection {
                    address,
                    state: DeviceConnectionState::Connected,
                } => {
                    self.begin_physical_connection(address);
                    self.consume_advertisement(address).await;
                }
                Observed::DeviceConnection {
                    address,
                    state: DeviceConnectionState::Disconnected,
                } => {
                    self.end_physical_connection(address);
                }
                Observed::InboundSetupExpired(address) => {
                    if self.inbound_setups.contains_key(&address) {
                        self.clear_inbound_state(address);
                        match self.adapter.remove_device(address).await {
                            Ok(()) => crate::diagnostic_log::warn!(
                                "bluetooth: inbound GATT setup from {address} expired; stale physical link removed"
                            ),
                            Err(error) => crate::diagnostic_log::warn!(
                                "bluetooth: inbound GATT setup from {address} expired and could not be removed: {error:?}"
                            ),
                        }
                        self.republish_advertisement().await;
                    }
                }
                Observed::AdapterEventsEnded => self.adapter_events = None,
                Observed::GattRetry => self.gatt_retry_at = None,
                Observed::Connected(target, result) => {
                    self.connecting.remove(&target);
                    match result {
                        Ok(link) => {
                            self.active_links.activate(target);
                            let peer_rssi = self.peer_rssi(target).await;
                            return BleEvent::LinkReady {
                                link,
                                origin: Origin::Dialed,
                                peer_rssi,
                            };
                        }
                        Err(error) => {
                            crate::diagnostic_log::warn!(
                                "bluetooth: dial to {target} failed: {error:?}"
                            );
                            return BleEvent::DialFailed {
                                address: BleAddress::new(target.0),
                            };
                        }
                    }
                }
                Observed::Resweep => {
                    self.resweep_schedule.restart(Instant::now());
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

    async fn dial(&mut self, address: BleAddress) -> DialOutcome {
        if !self.radio_power.is_on() {
            return DialOutcome::RadioOff;
        }
        let target = Address::new(*address.octets());
        if self.connecting.contains(&target) {
            return DialOutcome::Started;
        }
        self.discovery = None;
        self.discovery_health.retry_at = None;
        self.connecting.insert(target);
        let adapter = self.adapter.clone();
        let cleanup_adapter = adapter.clone();
        self.connects.push(Box::pin(async move {
            let result =
                match tokio::time::timeout(CONNECT_TIMEOUT, connect_link(adapter, target)).await {
                    Ok(result) => result,
                    Err(_) => Err(BluerError::DialTimeout),
                };
            if result.is_err() {
                let _ = cleanup_adapter.remove_device(target).await;
            }
            (target, result)
        }));
        DialOutcome::Started
    }

    async fn on_link_closed(&mut self, address: BleAddress) {
        let target = Address::new(*address.octets());
        self.connecting.remove(&target);
        if matches!(
            self.active_links.close(target),
            LinkClosure::SupersededGeneration
        ) {
            crate::diagnostic_log::debug!(
                "bluetooth: {target} older link generation released; replacement remains active"
            );
            return;
        }
        let consumed_advertisement = self.clear_inbound_state(target);
        let _ = self.adapter.remove_device(target).await;
        if consumed_advertisement {
            self.republish_advertisement().await;
        }
        crate::diagnostic_log::debug!(
            "bluetooth: {target} link released; will re-sight if it returns"
        );
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

    fn peer_protocol(&self) -> PeerProtocol {
        match self {
            BluerLink::Dialed(link) => link.peer_protocol(),
            BluerLink::Accepted(link) => link.peer_protocol(),
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

    async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, BluerError> {
        match self {
            BluerLink::Dialed(link) => link.receive_columba_peer_identity().await,
            BluerLink::Accepted(link) => link.receive_columba_peer_identity().await,
        }
    }

    async fn send_columba_identity(&mut self, identity: BleIdentity) -> Result<(), BluerError> {
        match self {
            BluerLink::Dialed(link) => link.send_columba_identity(identity).await,
            BluerLink::Accepted(link) => link.send_columba_identity(identity).await,
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
    peer_protocol: PeerProtocol,
    peer_identity: Option<BleIdentity>,
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

    fn peer_protocol(&self) -> PeerProtocol {
        self.peer_protocol
    }

    fn address(&self) -> BleAddress {
        BleAddress::new(self.peer_address.0)
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), BluerError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg.encode(&mut buf).ok_or(BluerError::ControlPduTooLarge)?;
        let written = match self.peer_protocol {
            PeerProtocol::Native => {
                self.control
                    .write_ext(&buf[..len], &acknowledged_write())
                    .await
            }
            PeerProtocol::Columba => self.control.write(&buf[..len]).await,
        };
        match written {
            Ok(()) => {
                crate::diagnostic_log::debug!("bluetooth: {} <- {msg:?}", self.peer_address);
                Ok(())
            }
            Err(error) => {
                crate::diagnostic_log::warn!(
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
                crate::diagnostic_log::debug!("bluetooth: {} -> {control:?}", self.peer_address);
                Ok(control)
            }
            None => {
                crate::diagnostic_log::warn!(
                    "bluetooth: {} sent an undecodable control notification ({} bytes)",
                    self.peer_address,
                    value.len()
                );
                Err(BluerError::MalformedControl)
            }
        }
    }

    async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, BluerError> {
        self.peer_identity.ok_or(BluerError::NoColumbaIdentity)
    }

    async fn send_columba_identity(&mut self, identity: BleIdentity) -> Result<(), BluerError> {
        self.control.write(identity.as_bytes()).await?;
        Ok(())
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), BluerError> {
        if self.peer_protocol == PeerProtocol::Columba {
            return Ok(());
        }
        match plan {
            L2capPlan::Open { psm } => {
                crate::diagnostic_log::debug!(
                    "bluetooth: {} handshake settled, opening L2CAP CoC to PSM {:#x}",
                    self.peer_address,
                    psm.get()
                );
                let socket = Socket::<SeqPacket>::new_seq_packet()?;
                socket.set_security(BluerBackend::l2cap_security())?;
                socket.set_recv_mtu(L2CAP_SDU_LEN as u16)?;
                socket.bind(L2capSocketAddr::any_le())?;
                let target =
                    L2capSocketAddr::new(self.peer_address, self.peer_address_type, psm.get());
                let connected = match tokio::time::timeout(
                    L2CAP_UPGRADE_TIMEOUT,
                    socket.connect(target),
                )
                .await
                {
                    Ok(Ok(connected)) => connected,
                    Ok(Err(error)) => {
                        crate::diagnostic_log::warn!(
                                "bluetooth: {} L2CAP connect to PSM {:#x} failed: {error}; settling on GATT",
                                self.peer_address,
                                psm.get()
                            );
                        return Err(error.into());
                    }
                    Err(_) => {
                        crate::diagnostic_log::warn!(
                            "bluetooth: {} L2CAP connect to PSM {:#x} timed out; settling on GATT",
                            self.peer_address,
                            psm.get()
                        );
                        return Err(BluerError::L2capTimeout);
                    }
                };
                self.socket = Some(Arc::new(connected));
                crate::diagnostic_log::debug!(
                    "bluetooth: {} L2CAP data plane up",
                    self.peer_address
                );
                Ok(())
            }
            L2capPlan::Accept => {
                crate::diagnostic_log::debug!(
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
                    _gatt_lifetime: L2capGattLifetime::Dialed,
                })),
                BluerSink::L2cap(L2capSink(Some(socket))),
            ),
            None => {
                crate::diagnostic_log::debug!(
                    "bluetooth: {} GATT data plane up",
                    self.peer_address
                );
                let (rx, tx) = match (self.data_notify, self.data) {
                    (Some(data_notify), Some(data)) => (
                        GattRx::Notify(data_notify),
                        GattTx::remote(self.peer_protocol, data),
                    ),
                    _ => (
                        GattRx::Notify(self.notify),
                        GattTx::remote(self.peer_protocol, self.control),
                    ),
                };
                (
                    BluerSource::Gatt(Box::new(GattSource {
                        rx,
                        reassembler: Reassembler::new(),
                    })),
                    BluerSink::Gatt(GattSink::new(tx)),
                )
            }
        }
    }
}

pub struct AcceptedLink {
    peer_protocol: PeerProtocol,
    reader: CharacteristicReader,
    writer: CharacteristicWriter,
    address: Address,
    l2cap: Option<oneshot::Receiver<SeqPacket>>,
    socket: Option<Arc<SeqPacket>>,
    data: ServerData,
}

impl BleLink for AcceptedLink {
    type Error = BluerError;
    type Source = BluerSource;
    type Sink = BluerSink;

    fn peer_protocol(&self) -> PeerProtocol {
        self.peer_protocol
    }

    fn address(&self) -> BleAddress {
        BleAddress::new(self.address.0)
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), BluerError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg.encode(&mut buf).ok_or(BluerError::ControlPduTooLarge)?;
        self.writer.write_all(&buf[..len]).await?;
        self.writer.flush().await?;
        crate::diagnostic_log::debug!("bluetooth: {} <- {msg:?}", self.address);
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
                crate::diagnostic_log::debug!("bluetooth: {} -> {control:?}", self.address);
                Ok(control)
            }
            None => {
                crate::diagnostic_log::warn!(
                    "bluetooth: {} sent an undecodable control write ({read} bytes)",
                    self.address
                );
                Err(BluerError::MalformedControl)
            }
        }
    }

    async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, BluerError> {
        let mut identity = [0u8; 16];
        self.reader.read_exact(&mut identity).await?;
        Ok(BleIdentity::new(identity))
    }

    async fn send_columba_identity(&mut self, identity: BleIdentity) -> Result<(), BluerError> {
        self.writer.write_all(identity.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), BluerError> {
        if self.peer_protocol == PeerProtocol::Columba {
            return Ok(());
        }
        match plan {
            L2capPlan::Accept => {
                let Some(inbound) = self.l2cap.take() else {
                    crate::diagnostic_log::warn!(
                        "bluetooth: {} has no L2CAP listener; settling on GATT",
                        self.address
                    );
                    return Err(BluerError::NotUpgraded);
                };
                crate::diagnostic_log::debug!(
                    "bluetooth: {} handshake settled, awaiting its inbound L2CAP CoC",
                    self.address
                );
                match tokio::time::timeout(L2CAP_UPGRADE_TIMEOUT, inbound).await {
                    Ok(Ok(connected)) => {
                        self.socket = Some(Arc::new(connected));
                        crate::diagnostic_log::debug!(
                            "bluetooth: {} L2CAP data plane up",
                            self.address
                        );
                        Ok(())
                    }
                    Ok(Err(_)) => {
                        crate::diagnostic_log::warn!(
                            "bluetooth: {} L2CAP accept channel closed; settling on GATT",
                            self.address
                        );
                        Err(BluerError::Closed)
                    }
                    Err(_) => {
                        crate::diagnostic_log::warn!(
                            "bluetooth: {} L2CAP accept timed out; settling on GATT",
                            self.address
                        );
                        Err(BluerError::L2capTimeout)
                    }
                }
            }
            L2capPlan::Open { .. } => {
                crate::diagnostic_log::debug!(
                    "bluetooth: {} stays on the GATT data plane (accepted link; Linux-opens-CoC-to-peer is the capability-role follow-up)",
                    self.address
                );
                Ok(())
            }
            L2capPlan::None => Ok(()),
        }
    }

    fn into_data(self) -> (BluerSource, BluerSink) {
        let Self {
            reader,
            writer,
            address,
            socket,
            data,
            ..
        } = self;
        match socket {
            Some(socket) => (
                BluerSource::L2cap(Box::new(L2capSource {
                    socket: Some(socket.clone()),
                    deframer: StreamDeframer::new(),
                    _gatt_lifetime: L2capGattLifetime::Accepted {
                        _control_reader: reader,
                        _control_writer: writer,
                        _data: Box::new(data),
                    },
                })),
                BluerSink::L2cap(L2capSink(Some(socket))),
            ),
            None => {
                crate::diagnostic_log::debug!("bluetooth: {address} GATT data plane up");
                let (rx, tx) = match data {
                    ServerData::TwoChar { writer, reader } => {
                        let rx = match reader {
                            DataRead::Ready(reader) => GattRx::Reader(reader),
                            DataRead::Pending(pending) => GattRx::Pending(pending),
                        };
                        let tx = match writer {
                            DataWrite::Ready(writer) => GattTx::Writer(writer),
                            DataWrite::Pending(pending) => GattTx::Pending(pending),
                        };
                        (rx, tx)
                    }
                    ServerData::Columba => (GattRx::Reader(reader), GattTx::Writer(writer)),
                };
                (
                    BluerSource::Gatt(Box::new(GattSource {
                        rx,
                        reassembler: Reassembler::new(),
                    })),
                    BluerSink::Gatt(GattSink::new(tx)),
                )
            }
        }
    }
}

enum L2capGattLifetime {
    Dialed,
    Accepted {
        _control_reader: CharacteristicReader,
        _control_writer: CharacteristicWriter,
        _data: Box<ServerData>,
    },
}

pub struct L2capSource {
    socket: Option<Arc<SeqPacket>>,
    deframer: StreamDeframer<{ 2 * L2CAP_SDU_LEN }>,
    _gatt_lifetime: L2capGattLifetime,
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
    Pending(oneshot::Receiver<CharacteristicReader>),
}

enum GattTx {
    RemoteRequest(RemoteCharacteristic),
    RemoteCommand(RemoteCharacteristic),
    Writer(CharacteristicWriter),
    Pending(oneshot::Receiver<CharacteristicWriter>),
}

impl GattTx {
    fn remote(peer_protocol: PeerProtocol, characteristic: RemoteCharacteristic) -> Self {
        match peer_protocol {
            PeerProtocol::Native => Self::RemoteRequest(characteristic),
            PeerProtocol::Columba => Self::RemoteCommand(characteristic),
        }
    }
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
                GattRx::Notify(notify) => notify
                    .next()
                    .await
                    .ok_or(BluerError::GattNotificationsEnded)?,
                GattRx::Reader(reader) => {
                    let mut scratch = [0u8; BLE_HW_MTU];
                    let read = reader.read(&mut scratch).await?;
                    if read == 0 {
                        return Err(BluerError::GattWriteChannelEnded);
                    }
                    scratch[..read].to_vec()
                }
                GattRx::Pending(pending) => {
                    let reader = pending
                        .await
                        .map_err(|_| BluerError::GattDataReaderUnavailable)?;
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

impl GattSink {
    fn new(tx: GattTx) -> Self {
        Self { tx }
    }
}

impl BleSink for GattSink {
    type Error = BluerError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), BluerError> {
        let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
        for fragment in fragments_of(frame, GATT_FRAGMENT_PAYLOAD) {
            let n = fragment.encode(&mut buf).ok_or(BluerError::FrameTooLarge)?;
            loop {
                match &mut self.tx {
                    GattTx::RemoteRequest(remote) => {
                        remote.write_ext(&buf[..n], &acknowledged_write()).await?;
                    }
                    GattTx::RemoteCommand(remote) => {
                        remote.write(&buf[..n]).await?;
                    }
                    GattTx::Writer(writer) => {
                        writer.write_all(&buf[..n]).await?;
                        writer.flush().await?;
                    }
                    GattTx::Pending(pending) => {
                        let writer = pending
                            .await
                            .map_err(|_| BluerError::GattDataWriterUnavailable)?;
                        self.tx = GattTx::Writer(writer);
                        continue;
                    }
                }
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prns_name_and_role_marker_recover_a_missing_bluez_uuid() {
        let manufacturer_data = HashMap::from([(u16::MAX, vec![3, 0])]);

        assert!(identifies_prns_fallback(
            Some(PRNS_DEVICE_NAME),
            Some(&manufacturer_data)
        ));
        assert!(!identifies_prns_fallback(
            Some("Other"),
            Some(&manufacturer_data)
        ));
    }

    #[test]
    fn an_inbound_connection_republishes_without_losing_advertising_demand() {
        let mut state = AdvertisementRegistration::<u8>::new();
        state.want();
        state.register(7);
        state.republish();

        assert_eq!(state, AdvertisementRegistration::Wanted);
    }

    #[test]
    fn advertisement_republishing_is_coalesced_by_inbound_address() {
        let now = Instant::now();
        let address = Address::new([0xAA; 6]);
        let mut consumers = AdvertisementConsumers::new();

        let first = consumers.begin(address, now);
        let duplicate = consumers.begin(address, now);
        let closed = consumers.close(address);
        let after_close = consumers.begin(address, now);
        let duplicate_after_close = consumers.begin(address, now);
        consumers.prune(now + ADVERTISEMENT_CONSUMER_TTL);
        let after_timeout = consumers.begin(address, now + ADVERTISEMENT_CONSUMER_TTL);

        assert_eq!(
            [
                first,
                duplicate,
                closed,
                after_close,
                duplicate_after_close,
                after_timeout,
            ],
            [true, false, true, true, false, true],
        );
    }

    #[test]
    fn an_older_link_generation_cannot_release_its_replacement() {
        let address = Address::new([0xAA; 6]);
        let mut links = ActiveLinkGenerations::new();
        links.activate(address);
        links.activate(address);

        assert!(matches!(
            links.close(address),
            LinkClosure::SupersededGeneration
        ));
        assert!(links.contains(address));
        assert!(matches!(
            links.close(address),
            LinkClosure::LastActiveGeneration
        ));
        assert!(!links.contains(address));
        assert!(matches!(links.close(address), LinkClosure::Untracked));
    }

    #[test]
    fn advertising_interval_meets_the_recovery_boundary() {
        let advertisement = BluerBackend::advertisement(default_group_tag());

        assert_eq!(
            (advertisement.min_interval, advertisement.max_interval),
            (
                Some(ADVERTISING_INTERVAL_MIN),
                Some(ADVERTISING_INTERVAL_MAX)
            )
        );
    }

    #[test]
    fn resweep_schedule_retains_its_deadline_until_restarted() {
        let now = Instant::now();
        let mut schedule = ResweepSchedule::new(now);
        let first_deadline = schedule.due_at;

        assert_eq!(schedule.due_at, first_deadline);

        schedule.restart(first_deadline);

        assert_eq!(schedule.due_at, first_deadline + RESWEEP_INTERVAL);
    }

    #[test]
    fn gatt_channels_one_is_safe() {
        assert_eq!(
            gatt_channels_setting_from_str("[GATT]\nChannels = 1\n"),
            Some(EattRisk::Safe)
        );
    }

    #[test]
    fn any_gatt_channels_above_one_is_risky() {
        assert_eq!(
            gatt_channels_setting_from_str("[GATT]\nChannels = 3\n"),
            Some(EattRisk::Risky)
        );
    }

    #[test]
    fn duplicate_gatt_section_with_stale_high_channels_is_risky() {
        assert_eq!(
            gatt_channels_setting_from_str("[GATT]\nChannels = 3\n\n[GATT]\nChannels = 1\n"),
            Some(EattRisk::Risky)
        );
    }

    #[test]
    fn native_gatt_uses_remote_acknowledgements() {
        assert_eq!(acknowledged_write().op_type, WriteOp::Request);
    }

    #[test]
    fn half_open_gatt_state_expires_on_monotonic_time() {
        let opened_at = Instant::now();
        let pending = PendingHalves {
            opened_at,
            reader: None,
            writer: None,
        };

        assert!(!pending.expired(opened_at + GATT_HALF_OPEN_TIMEOUT - Duration::from_nanos(1)));
        assert!(pending.expired(opened_at + GATT_HALF_OPEN_TIMEOUT));
    }

    #[test]
    fn half_open_gatt_admission_is_bounded_by_peer_capacity() {
        let mut entries = HashMap::new();
        for byte in 0..BluerBackend::MAX_PEERS {
            entries.insert(Address::new([byte as u8; 6]), ());
        }

        assert!(can_admit_address(&entries, Address::new([0; 6])));
        assert!(!can_admit_address(&entries, Address::new([0xff; 6])));
    }

    #[tokio::test]
    async fn pending_data_reader_survives_a_cancelled_receive() {
        let (sender, receiver) = oneshot::channel::<CharacteristicReader>();
        let mut source = GattSource {
            rx: GattRx::Pending(receiver),
            reassembler: Reassembler::new(),
        };
        let mut out = [0u8; BLE_HW_MTU];

        {
            let mut receive = Box::pin(source.recv_frame(&mut out));
            assert!(matches!(
                futures_util::poll!(&mut receive),
                std::task::Poll::Pending
            ));
        }

        drop(sender);
        assert!(matches!(
            source.recv_frame(&mut out).await,
            Err(BluerError::GattDataReaderUnavailable)
        ));
    }

    #[tokio::test]
    async fn pending_data_writer_survives_a_cancelled_send() {
        let (sender, receiver) = oneshot::channel::<CharacteristicWriter>();
        let mut sink = GattSink {
            tx: GattTx::Pending(receiver),
        };

        {
            let mut send = Box::pin(sink.send_frame(&[0xAA]));
            assert!(matches!(
                futures_util::poll!(&mut send),
                std::task::Poll::Pending
            ));
        }

        drop(sender);
        assert!(matches!(
            sink.send_frame(&[0xAA]).await,
            Err(BluerError::GattDataWriterUnavailable)
        ));
    }

    #[test]
    fn the_accept_router_delivers_each_socket_to_its_own_address() {
        let mut router = AcceptRouter::<u32>::new();
        let a = Address::new([0xAA; 6]);
        let b = Address::new([0xBB; 6]);
        let mut rx_a = router.register(a);
        let mut rx_b = router.register(b);

        assert!(router.deliver(b, 0xB).is_ok());
        assert_eq!(rx_b.try_recv(), Ok(0xB));
        assert_eq!(rx_a.try_recv(), Err(oneshot::error::TryRecvError::Empty));

        assert!(router.deliver(a, 0xA).is_ok());
        assert_eq!(rx_a.try_recv(), Ok(0xA));
    }

    #[test]
    fn the_accept_router_returns_an_unclaimed_socket_for_the_caller_to_drop() {
        let mut router = AcceptRouter::<u32>::new();
        let stranger = Address::new([0xCC; 6]);
        assert_eq!(router.deliver(stranger, 0xC), Err(0xC));
    }

    #[test]
    fn a_cancelled_registration_no_longer_receives() {
        let mut router = AcceptRouter::<u32>::new();
        let a = Address::new([0xAA; 6]);
        let _rx_a = router.register(a);
        router.cancel(&a);
        assert_eq!(router.deliver(a, 0xA), Err(0xA));
    }

    #[tokio::test]
    async fn independent_accept_delivery_unblocks_a_waiting_upgrade() {
        let address = Address::new([0xAA; 6]);
        let mut router = AcceptRouter::<u32>::new();
        let receiver = router.register(address);
        let router = Arc::new(std::sync::Mutex::new(router));
        let delivery_router = Arc::clone(&router);
        let delivery = tokio::spawn(async move {
            tokio::task::yield_now().await;
            match delivery_router.lock() {
                Ok(mut router) => router.deliver(address, 0xA).is_ok(),
                Err(_) => false,
            }
        });

        let received = tokio::time::timeout(Duration::from_secs(1), receiver).await;
        assert!(matches!(received, Ok(Ok(0xA))));
        assert!(matches!(delivery.await, Ok(true)));
    }
}
