use core::time::Duration;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use bluer::adv::{Advertisement, AdvertisementHandle};
use bluer::gatt::local::{
    characteristic_control, service_control, Application, ApplicationHandle, Characteristic,
    CharacteristicControl, CharacteristicControlEvent, CharacteristicNotify,
    CharacteristicNotifyMethod, CharacteristicWrite, CharacteristicWriteMethod, Service,
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
use futures_util::{Stream, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

use crate::interfaces::bluetooth_auto::core::{
    encode_stream_frame, BleAddress, BleUuid, Control, Dialect, Psm, StreamDeframer, Transport,
    BLE_HW_MTU, BLE_SERVICE_UUID, CONTROL_MAX_LEN, NATIVE_CONTROL_UUID, STREAM_FRAME_PREFIX_LEN,
};
use crate::interfaces::bluetooth_auto::seam::{BleBackend, BleEvent, BleLink, BleSink, BleSource};

const ADVERTISED_NAME: &str = "Prns";

const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;

const SCAN_STOP_POLL: Duration = Duration::from_millis(20);
const SCAN_STOP_ATTEMPTS: usize = 25;

const DIAL_RETRY_BASE: Duration = Duration::from_secs(2);
const DIAL_RETRY_MAX: Duration = Duration::from_secs(30);
const DIAL_GIVEUP_AFTER: u32 = 5;
const DIAL_GIVEUP_COOLDOWN: Duration = Duration::from_secs(300);
const STABLE_LINK_UPTIME: Duration = Duration::from_secs(15);

enum DialAttempt {
    Connected { since: Instant, failures: u32 },
    Backoff { failures: u32, retry_at: Instant },
}

fn dial_retry_delay(failures: u32) -> Duration {
    let shift = failures.saturating_sub(1).min(4);
    (DIAL_RETRY_BASE * (1u32 << shift)).min(DIAL_RETRY_MAX)
}

#[derive(Debug)]
pub enum BluerError {
    Bluez(bluer::Error),
    Io(std::io::Error),
    NoControlCharacteristic,
    ControlPduTooLarge,
    MalformedControl,
    GattDataUnsupported,
    NotUpgraded,
    FrameTooLarge,
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

#[derive(Default)]
struct PendingHalves {
    reader: Option<CharacteristicReader>,
    writer: Option<CharacteristicWriter>,
}

enum Half {
    Reader(CharacteristicReader),
    Writer(CharacteristicWriter),
}

enum Observed {
    Candidate(Address),
    Greeting { address: Address, half: Half },
    RetryDue,
    Idle,
}

pub struct BluerBackend {
    adapter: Adapter,
    address: Address,
    address_type: AddressType,
    psm: Psm,
    attempts: HashMap<Address, DialAttempt>,
    pending: HashMap<Address, PendingHalves>,
    discovery: Option<Pin<Box<dyn Stream<Item = AdapterEvent> + Send>>>,
    control: Option<Pin<Box<CharacteristicControl>>>,
    listener: Option<Arc<SeqPacketListener>>,
    _advertisement: Option<AdvertisementHandle>,
    _application: Option<ApplicationHandle>,
}

impl BluerBackend {
    pub async fn open(psm: Psm) -> Result<Self, BluerError> {
        let session = Session::new().await?;
        let adapter = session.default_adapter().await?;
        adapter.set_powered(true).await?;
        let address = adapter.address().await?;
        let address_type = adapter.address_type().await?;
        Ok(Self {
            adapter,
            address,
            address_type,
            psm,
            attempts: HashMap::new(),
            pending: HashMap::new(),
            discovery: None,
            control: None,
            listener: None,
            _advertisement: None,
            _application: None,
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

    async fn await_scan_stopped(&self) {
        for _ in 0..SCAN_STOP_ATTEMPTS {
            if matches!(self.adapter.is_discovering().await, Ok(false)) {
                return;
            }
            tokio::time::sleep(SCAN_STOP_POLL).await;
        }
    }

    fn dial_suppressed(&self, address: Address) -> bool {
        match self.attempts.get(&address) {
            Some(DialAttempt::Connected { .. }) => true,
            Some(DialAttempt::Backoff { retry_at, .. }) => Instant::now() < *retry_at,
            None => false,
        }
    }

    fn next_wake_deadline(&self) -> Option<Instant> {
        self.attempts
            .values()
            .filter_map(|attempt| match attempt {
                DialAttempt::Backoff { retry_at, .. } => Some(*retry_at),
                DialAttempt::Connected { .. } => None,
            })
            .min()
    }

    fn next_redial(&self) -> Option<Address> {
        let now = Instant::now();
        self.attempts
            .iter()
            .find_map(|(address, attempt)| match attempt {
                DialAttempt::Backoff { retry_at, .. } if *retry_at <= now => Some(*address),
                _ => None,
            })
    }

    fn carried_failures(&self, target: Address) -> u32 {
        match self.attempts.get(&target) {
            Some(DialAttempt::Backoff { failures, .. }) => *failures,
            Some(DialAttempt::Connected { failures, .. }) => *failures,
            None => 0,
        }
    }

    fn schedule_redial(&mut self, target: Address, failures: u32) -> Duration {
        let (delay, carried) = if failures >= DIAL_GIVEUP_AFTER {
            (DIAL_GIVEUP_COOLDOWN, 0)
        } else {
            (dial_retry_delay(failures), failures)
        };
        self.attempts.insert(
            target,
            DialAttempt::Backoff {
                failures: carried,
                retry_at: Instant::now() + delay,
            },
        );
        delay
    }

    fn note_dial_failure(&mut self, target: Address) {
        let failures = self.carried_failures(target) + 1;
        let delay = self.schedule_redial(target, failures);
        if failures >= DIAL_GIVEUP_AFTER {
            log::warn!(
                "bluetooth: dial to {target} failed {failures}x; pausing dials to it for {delay:?}"
            );
        } else {
            log::warn!(
                "bluetooth: dial to {target} failed (attempt {failures}); retrying after {delay:?}"
            );
        }
    }

    async fn dial_inner(&mut self, target: Address) -> Result<BluerLink, BluerError> {
        let discovered = self.adapter.device(target)?;
        let peer_address_type = discovered.address_type().await?;
        log::info!("bluetooth: dialing {target} ({peer_address_type:?})");
        let device = if discovered.is_connected().await? {
            discovered
        } else {
            self.discovery = None;
            let _ = self.adapter.remove_device(target).await;
            self.await_scan_stopped().await;
            match self.adapter.connect_device(target, peer_address_type).await {
                Ok(device) => device,
                Err(error) => {
                    log::warn!("bluetooth: LE connect to {target} failed: {error}");
                    return Err(error.into());
                }
            }
        };
        let control = match find_native_control(&device).await {
            Ok(control) => control,
            Err(error) => {
                log::warn!("bluetooth: no native control characteristic on {target}: {error:?}");
                return Err(error);
            }
        };
        let notify = control.notify().await?;
        log::info!(
            "bluetooth: {target} connected over LE, control characteristic ready; handshaking"
        );
        Ok(BluerLink::Dialed(DialedLink {
            control,
            notify: Box::pin(notify),
            peer_address: target,
            peer_address_type,
            socket: None,
            _device: device,
        }))
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
            ) => Some(AcceptedLink {
                reader,
                writer,
                address,
                listener,
                socket: None,
            }),
            _ => None,
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

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => core::future::pending().await,
    }
}

impl BleBackend for BluerBackend {
    const MAX_PEERS: usize = 8;
    type Error = BluerError;
    type Link = BluerLink;

    async fn advertise(&mut self) -> Result<(), BluerError> {
        let advertisement = Advertisement {
            advertisement_type: bluer::adv::Type::Peripheral,
            service_uuids: [uuid_of(BLE_SERVICE_UUID)].into_iter().collect(),
            discoverable: Some(true),
            local_name: Some(ADVERTISED_NAME.to_string()),
            ..Default::default()
        };
        let advertisement = self.adapter.advertise(advertisement).await?;

        let (control, control_handle) = characteristic_control();
        let (_service_control, service_handle) = service_control();
        let application = Application {
            services: vec![Service {
                uuid: uuid_of(BLE_SERVICE_UUID),
                primary: true,
                characteristics: vec![Characteristic {
                    uuid: uuid_of(NATIVE_CONTROL_UUID),
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
                }],
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
        self.listener = listener.map(Arc::new);
        self._advertisement = Some(advertisement);
        self._application = Some(application);
        self.start_discovery().await?;
        log::info!(
            "bluetooth: advertising as {ADVERTISED_NAME}, scanning LE for Prns peers, control PSM {:#x}, listener {}",
            self.psm.get(),
            if self.listener.is_some() { "bound" } else { "unavailable" },
        );
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<BluerLink> {
        loop {
            if self.discovery.is_none() {
                if let Err(error) = self.start_discovery().await {
                    log::warn!("bluetooth: failed to (re)start scanning: {error:?}");
                }
            }
            let wake_deadline = self.next_wake_deadline();
            let observed = {
                let discovery = self.discovery.as_mut();
                let control = self.control.as_mut();
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
                    () = sleep_until_opt(wake_deadline) => Observed::RetryDue,
                }
            };
            match observed {
                Observed::Candidate(address) => {
                    let known = address == self.address || self.dial_suppressed(address);
                    if !known {
                        if self.advertises_our_service(address).await {
                            log::info!("bluetooth: sighted Prns peer {address}");
                            return BleEvent::Sighting(BleAddress::new(address.0));
                        }
                        log::debug!("bluetooth: discovered {address}, not advertising our service");
                    }
                }
                Observed::Greeting { address, half } => {
                    if let Some(link) = self.admit_greeting(address, half) {
                        log::info!("bluetooth: inbound link from {address}");
                        return BleEvent::Inbound(BluerLink::Accepted(link));
                    }
                }
                Observed::RetryDue => {
                    if let Some(address) = self.next_redial() {
                        log::info!("bluetooth: re-dialing {address}");
                        return BleEvent::Sighting(BleAddress::new(address.0));
                    }
                }
                Observed::Idle => {}
            }
        }
    }

    async fn dial(&mut self, address: BleAddress) -> Result<BluerLink, BluerError> {
        let target = Address::new(*address.octets());
        match self.dial_inner(target).await {
            Ok(link) => {
                let failures = self.carried_failures(target);
                self.attempts.insert(
                    target,
                    DialAttempt::Connected {
                        since: Instant::now(),
                        failures,
                    },
                );
                Ok(link)
            }
            Err(error) => {
                self.note_dial_failure(target);
                Err(error)
            }
        }
    }

    async fn on_link_closed(&mut self, address: BleAddress) {
        let target = Address::new(*address.octets());
        let uptime = match self.attempts.get(&target) {
            Some(DialAttempt::Connected { since, failures }) => Some((since.elapsed(), *failures)),
            _ => None,
        };
        let failures = match uptime {
            Some((up, prior)) if up < STABLE_LINK_UPTIME => prior + 1,
            _ => 0,
        };
        let delay = self.schedule_redial(target, failures);
        if failures >= DIAL_GIVEUP_AFTER {
            log::warn!(
                "bluetooth: {target} link keeps flapping; pausing dials to it for {delay:?}"
            );
        } else if let Some((up, _)) = uptime {
            log::info!("bluetooth: {target} link closed after {up:?}; re-dialing after {delay:?}");
        } else {
            log::info!("bluetooth: {target} link closed; re-dialing after {delay:?}");
        }
    }
}

async fn find_native_control(device: &Device) -> Result<RemoteCharacteristic, BluerError> {
    let service_uuid = uuid_of(BLE_SERVICE_UUID);
    let control_uuid = uuid_of(NATIVE_CONTROL_UUID);
    for service in device.services().await? {
        if service.uuid().await? == service_uuid {
            for characteristic in service.characteristics().await? {
                if characteristic.uuid().await? == control_uuid {
                    return Ok(characteristic);
                }
            }
        }
    }
    Err(BluerError::NoControlCharacteristic)
}

pub enum BluerLink {
    Dialed(DialedLink),
    Accepted(AcceptedLink),
}

impl BleLink for BluerLink {
    type Error = BluerError;
    type Source = L2capSource;
    type Sink = L2capSink;

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

    async fn upgrade(&mut self, transport: &Transport) -> Result<(), BluerError> {
        match self {
            BluerLink::Dialed(link) => link.upgrade(transport).await,
            BluerLink::Accepted(link) => link.upgrade(transport).await,
        }
    }

    fn into_data(self) -> (L2capSource, L2capSink) {
        match self {
            BluerLink::Dialed(link) => link.into_data(),
            BluerLink::Accepted(link) => link.into_data(),
        }
    }
}

pub struct DialedLink {
    control: RemoteCharacteristic,
    notify: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
    peer_address: Address,
    peer_address_type: AddressType,
    socket: Option<Arc<SeqPacket>>,
    _device: Device,
}

impl BleLink for DialedLink {
    type Error = BluerError;
    type Source = L2capSource;
    type Sink = L2capSink;

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

    async fn upgrade(&mut self, transport: &Transport) -> Result<(), BluerError> {
        match transport {
            Transport::L2cap { psm } => {
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
                let connected = match socket.connect(target).await {
                    Ok(connected) => connected,
                    Err(error) => {
                        log::warn!(
                            "bluetooth: {} L2CAP connect to PSM {:#x} failed: {error}",
                            self.peer_address,
                            psm.get()
                        );
                        return Err(error.into());
                    }
                };
                self.socket = Some(Arc::new(connected));
                log::info!("bluetooth: {} L2CAP data plane up", self.peer_address);
                Ok(())
            }
            Transport::Gatt => {
                log::warn!(
                    "bluetooth: {} handshake settled on GATT-only transport; the GATT data plane is not implemented on Linux yet, so this member carries no frames",
                    self.peer_address
                );
                Err(BluerError::GattDataUnsupported)
            }
        }
    }

    fn into_data(self) -> (L2capSource, L2capSink) {
        (
            L2capSource {
                socket: self.socket.clone(),
                deframer: StreamDeframer::new(),
            },
            L2capSink(self.socket),
        )
    }
}

pub struct AcceptedLink {
    reader: CharacteristicReader,
    writer: CharacteristicWriter,
    address: Address,
    listener: Arc<SeqPacketListener>,
    socket: Option<Arc<SeqPacket>>,
}

impl BleLink for AcceptedLink {
    type Error = BluerError;
    type Source = L2capSource;
    type Sink = L2capSink;

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

    async fn upgrade(&mut self, transport: &Transport) -> Result<(), BluerError> {
        match transport {
            Transport::L2cap { .. } => {
                log::info!(
                    "bluetooth: {} handshake settled, accepting L2CAP CoC on our listener",
                    self.address
                );
                let (connected, _peer) = self.listener.accept().await?;
                self.socket = Some(Arc::new(connected));
                log::info!("bluetooth: {} L2CAP data plane up", self.address);
                Ok(())
            }
            Transport::Gatt => {
                log::warn!(
                    "bluetooth: {} handshake settled on GATT-only transport; the GATT data plane is not implemented on Linux yet, so this member carries no frames",
                    self.address
                );
                Err(BluerError::GattDataUnsupported)
            }
        }
    }

    fn into_data(self) -> (L2capSource, L2capSink) {
        (
            L2capSource {
                socket: self.socket.clone(),
                deframer: StreamDeframer::new(),
            },
            L2capSink(self.socket),
        )
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
