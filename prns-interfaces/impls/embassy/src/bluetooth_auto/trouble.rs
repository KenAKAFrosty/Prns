use core::cell::Cell;

use bt_hci::transport::Transport;
use bt_hci::FromHciBytesError;
use embassy_futures::select::{select, select3, select4, Either, Either3};
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as BridgeMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::semaphore::{FairSemaphore, Semaphore, SemaphoreReleaser};
use embassy_sync::signal::Signal;
use embassy_sync_07::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{with_timeout, Duration, Timer};
use heapless_09::Vec as GattVec;
use portable_atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use trouble_host::prelude::*;

use prns_core::interfaces::bluetooth_auto::{
    columba_connection_role, columba_role_capabilities, contains_service, encode_stream_frame,
    fragments_of, BleAddress, BleIdentity, BleRoleCapabilities, ColumbaConnectionRole, Control,
    Fragment, L2capPlan, PeerProtocol, Reassembler, BLE_HW_MTU, BLE_SERVICE_UUID_BYTES,
    CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN, STREAM_FRAME_PREFIX_LEN,
};
use prns_core::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin, ScanningMode,
};

use super::connection_slots::{
    ConnectionSlotDataOwners, ConnectionSlotLease, ConnectionSlotLinkLease, ConnectionSlotOwners,
    ConnectionSlotPool, ConnectionSlotSinkLease, ConnectionSlotSourceLease,
    ConnectionSlotWorkerLease, ReadyConnectionSlot, ReadyConnectionSlotParts,
};
use super::frame_pool::{FrameLease, SharedFramePool};

#[cfg(target_arch = "riscv32")]
pub const PEER_CAPACITY: usize = 8;
#[cfg(not(target_arch = "riscv32"))]
pub const PEER_CAPACITY: usize = 4;
const HCI_COMMAND_CAPACITY: usize = 20;
const ATTRIBUTE_TABLE: usize = 32;
const CCCD_TABLE: usize = 4;
pub const GATT_VALUE_CAP: usize = 244;
const MAX_SERVICES: usize = 2;

const CONTROL_UUID_LAST: u8 = 0xe7;
const DATA_UUID_LAST: u8 = 0xe8;
const COLUMBA_TX_UUID_LAST: u8 = 0xe4;
const COLUMBA_RX_UUID_LAST: u8 = 0xe5;
const COLUMBA_IDENTITY_UUID_LAST: u8 = 0xe6;
const SERVICE_UUID_LAST: u8 = 0xe3;

const GATT_REASSEMBLY_CAP: usize = 600;
#[cfg(target_arch = "riscv32")]
const GATT_FRAGMENT_PAYLOAD: usize = 120;
#[cfg(not(target_arch = "riscv32"))]
const GATT_FRAGMENT_PAYLOAD: usize = 180;

/// Gives the controller time to put each GATT fragment on air before queuing the next.
#[cfg(target_arch = "riscv32")]
const NOTIFY_PACING: Duration = Duration::from_millis(30);
#[cfg(not(target_arch = "riscv32"))]
const NOTIFY_PACING: Duration = Duration::from_millis(15);
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);
/// A bounded connect scan frees the slot and lets policy back off when a whitelisted peer is absent.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const GATT_SETUP_TIMEOUT: Duration = Duration::from_secs(6);
/// A wide connect scan catches dual-role peers that advertise only in short windows.
#[cfg(target_arch = "riscv32")]
const CONNECT_SCAN_INTERVAL: Duration = Duration::from_millis(200);
#[cfg(target_arch = "riscv32")]
const CONNECT_SCAN_WINDOW: Duration = Duration::from_millis(60);
#[cfg(not(target_arch = "riscv32"))]
const CONNECT_SCAN_INTERVAL: Duration = Duration::from_millis(100);
#[cfg(not(target_arch = "riscv32"))]
const CONNECT_SCAN_WINDOW: Duration = Duration::from_millis(80);
#[cfg(target_arch = "riscv32")]
const IDLE_SCAN_INTERVAL: Duration = Duration::from_millis(1500);
#[cfg(target_arch = "riscv32")]
const IDLE_SCAN_WINDOW: Duration = Duration::from_millis(60);
#[cfg(not(target_arch = "riscv32"))]
const IDLE_SCAN_INTERVAL: Duration = Duration::from_secs(1);
#[cfg(not(target_arch = "riscv32"))]
const IDLE_SCAN_WINDOW: Duration = Duration::from_millis(50);

/// Advertising and scanning alternate to avoid simultaneous-role controller limits; dial decisions made during an off-window remain buffered.
const ADV_WINDOW: Duration = Duration::from_millis(600);
#[cfg(target_arch = "riscv32")]
const SCAN_WINDOW: Duration = Duration::from_millis(300);
#[cfg(not(target_arch = "riscv32"))]
const SCAN_WINDOW: Duration = Duration::from_millis(600);
#[cfg(target_arch = "riscv32")]
const DISCOVERY_TURN_REST: Duration = Duration::from_millis(20);

/// Shallow per-slot lanes apply backpressure before BLE bursts crowd the USB and engine scheduler.
const CONTROL_QUEUE_DEPTH: usize = 2;
const FRAME_QUEUE_DEPTH: usize = 2;
const FRAME_POOL_CAPACITY: usize = PEER_CAPACITY;
const SIGHTING_DEPTH: usize = PEER_CAPACITY * 2;
const RADIO_WAITERS: usize = 2;

/// One L2CAP SDU carries one length-prefixed stream frame; modest credits and MPS keep two RX reservations inside the packet pool alongside GATT and TX.
pub const L2CAP_PSM: u16 = 0x0080;
const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;
#[cfg(target_arch = "riscv32")]
const L2CAP_MPS: u16 = 185;
#[cfg(not(target_arch = "riscv32"))]
const L2CAP_MPS: u16 = 247;
const L2CAP_CREDITS: u16 = 1;
const L2CAP_HANDSHAKE_WINDOW: Duration = Duration::from_secs(5);
const L2CAP_SETUP_RETRY: Duration = Duration::from_millis(150);
const PHY_UPDATE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_arch = "riscv32")]
const CONN_PARAM_UPDATE_TIMEOUT: Duration = Duration::from_secs(2);
/// Retains the address kind needed to whitelist a peer when policy identifies it by six address bytes.
const SEEN_CAP: usize = PEER_CAPACITY * 2;
const FRAME_CAP: usize = BLE_HW_MTU;

type RadioArbiter = FairSemaphore<BridgeMutex, RADIO_WAITERS>;
type RadioPermit<'a> = SemaphoreReleaser<'a, RadioArbiter>;
type BleSlotPool = ConnectionSlotPool<BridgeMutex, PEER_CAPACITY>;
type BleSlotLease = ConnectionSlotLease<BridgeMutex>;
type BleSlotWorker = ConnectionSlotWorkerLease<BridgeMutex>;
type BleSlotLink = ConnectionSlotLinkLease<BridgeMutex>;
type BleSlotSource = ConnectionSlotSourceLease<BridgeMutex>;
type BleSlotSink = ConnectionSlotSinkLease<BridgeMutex>;
type BleReadySlot = ReadyConnectionSlot<BridgeMutex>;
type BleFramePool = SharedFramePool<BridgeMutex, FRAME_CAP, FRAME_POOL_CAPACITY>;
type BleFrameLease = FrameLease<BridgeMutex, FRAME_CAP, FRAME_POOL_CAPACITY>;
pub trait TroubleTransport: Transport<Error: From<FromHciBytesError>> {}
impl<T: Transport<Error: From<FromHciBytesError>>> TroubleTransport for T {}
pub type TroubleController<T> = ExternalController<T, HCI_COMMAND_CAPACITY>;
pub type TroubleStack<T> = Stack<'static, TroubleController<T>, DefaultPacketPool>;
pub type GattServer = AttributeServer<
    'static,
    NoopRawMutex,
    DefaultPacketPool,
    ATTRIBUTE_TABLE,
    CCCD_TABLE,
    PEER_CAPACITY,
>;
pub type GattCharacteristic = Characteristic<GattVec<u8, GATT_VALUE_CAP>>;
pub type ReticulumAttributeTable = AttributeTable<'static, NoopRawMutex, ATTRIBUTE_TABLE>;

fn reticulum_uuid(last: u8) -> Uuid {
    let mut bytes = BLE_SERVICE_UUID_BYTES;
    bytes[15] = last;
    Uuid::from(u128::from_be_bytes(bytes))
}

pub fn service_uuid() -> Uuid {
    reticulum_uuid(SERVICE_UUID_LAST)
}

pub fn control_uuid() -> Uuid {
    reticulum_uuid(CONTROL_UUID_LAST)
}

pub fn data_uuid() -> Uuid {
    reticulum_uuid(DATA_UUID_LAST)
}

pub fn columba_tx_uuid() -> Uuid {
    reticulum_uuid(COLUMBA_TX_UUID_LAST)
}

pub fn columba_rx_uuid() -> Uuid {
    reticulum_uuid(COLUMBA_RX_UUID_LAST)
}

pub fn columba_identity_uuid() -> Uuid {
    reticulum_uuid(COLUMBA_IDENTITY_UUID_LAST)
}

fn advertisement_parameters() -> AdvertisementParameters {
    #[cfg(target_arch = "riscv32")]
    {
        let mut params = AdvertisementParameters::default();
        params.interval_min = Duration::from_millis(240);
        params.interval_max = Duration::from_millis(320);
        params
    }
    #[cfg(not(target_arch = "riscv32"))]
    {
        AdvertisementParameters::default()
    }
}

fn preferred_conn_params() -> RequestedConnParams {
    #[cfg(target_arch = "riscv32")]
    {
        RequestedConnParams {
            min_connection_interval: Duration::from_millis(120),
            max_connection_interval: Duration::from_millis(120),
            max_latency: 0,
            min_event_length: Duration::from_millis(1),
            max_event_length: Duration::from_millis(4),
            supervision_timeout: Duration::from_secs(8),
        }
    }
    #[cfg(not(target_arch = "riscv32"))]
    {
        RequestedConnParams::default()
    }
}

#[derive(Debug)]
pub struct Closed;

#[derive(Clone, Copy)]
struct SeenPeer {
    kind: AddrKind,
    addr: BdAddr,
    rssi: i8,
}

#[derive(Clone, Copy)]
struct DialTarget {
    kind: AddrKind,
    addr: BdAddr,
}

enum SlotJob {
    Accept {
        connection: Connection<'static, DefaultPacketPool>,
        slot: BleSlotLease,
    },
    Dial {
        connection: Connection<'static, DefaultPacketPool>,
        slot: BleSlotLease,
    },
}

struct SlotChannels {
    control_in: Channel<BridgeMutex, Control, CONTROL_QUEUE_DEPTH>,
    control_out: Channel<BridgeMutex, Control, CONTROL_QUEUE_DEPTH>,
    data_in: Channel<BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    data_out: Channel<BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    identity_in: Channel<BridgeMutex, BleIdentity, 1>,
    identity_out: Channel<BridgeMutex, BleIdentity, 1>,
    data_plane: Signal<BridgeMutex, L2capPlan>,
    peer_addr: BlockingMutex<BridgeMutex, Cell<[u8; 6]>>,
    peer_protocol: BlockingMutex<BridgeMutex, Cell<PeerProtocol>>,
}

impl SlotChannels {
    const fn new() -> Self {
        Self {
            control_in: Channel::new(),
            control_out: Channel::new(),
            data_in: Channel::new(),
            data_out: Channel::new(),
            identity_in: Channel::new(),
            identity_out: Channel::new(),
            data_plane: Signal::new(),
            peer_addr: BlockingMutex::new(Cell::new([0u8; 6])),
            peer_protocol: BlockingMutex::new(Cell::new(PeerProtocol::Native)),
        }
    }

    fn set_peer_addr(&self, bytes: [u8; 6]) {
        self.peer_addr.lock(|cell| cell.set(bytes));
    }

    fn addr(&self) -> [u8; 6] {
        self.peer_addr.lock(|cell| cell.get())
    }

    fn set_peer_protocol(&self, peer_protocol: PeerProtocol) {
        self.peer_protocol.lock(|cell| cell.set(peer_protocol));
    }

    fn peer_protocol(&self) -> PeerProtocol {
        self.peer_protocol.lock(|cell| cell.get())
    }

    fn clear_lanes(&self) {
        self.data_plane.reset();
        self.control_in.clear();
        self.control_out.clear();
        self.data_in.clear();
        self.data_out.clear();
        self.identity_in.clear();
        self.identity_out.clear();
    }

    fn link(
        &'static self,
        slot: BleSlotLink,
        outbound_frames: &'static BleFramePool,
    ) -> EmbeddedBleLink {
        EmbeddedBleLink {
            peer_protocol: self.peer_protocol(),
            control_in: self.control_in.receiver(),
            control_out: self.control_out.sender(),
            data_in: self.data_in.receiver(),
            data_out: self.data_out.sender(),
            identity_in: self.identity_in.receiver(),
            identity_out: self.identity_out.sender(),
            data_plane: &self.data_plane,
            plan: L2capPlan::None,
            address: self.addr(),
            outbound_frames,
            slot,
        }
    }
}

#[repr(u8)]
enum BleTaskPhase {
    Disabled,
    WaitingForSlot,
    WaitingForRadio,
    Starting,
    Active,
    Connecting,
    Dispatching,
    BackingOff,
}

struct BleDiagnostics {
    advertising_requested: AtomicBool,
    scanning_requested: AtomicBool,
    acceptor_phase: AtomicU8,
    dialer_phase: AtomicU8,
    active_slots: AtomicU8,
    last_progress_ms: AtomicU64,
    advertising_windows: AtomicU32,
    advertising_failures: AtomicU32,
    accepted_connections: AtomicU32,
    accept_failures: AtomicU32,
    scanning_windows: AtomicU32,
    scanning_failures: AtomicU32,
    dial_attempts: AtomicU32,
    dialed_connections: AtomicU32,
    dial_failures: AtomicU32,
    host_failures: AtomicU32,
}

impl BleDiagnostics {
    const fn new() -> Self {
        Self {
            advertising_requested: AtomicBool::new(false),
            scanning_requested: AtomicBool::new(false),
            acceptor_phase: AtomicU8::new(BleTaskPhase::Disabled as u8),
            dialer_phase: AtomicU8::new(BleTaskPhase::Disabled as u8),
            active_slots: AtomicU8::new(0),
            last_progress_ms: AtomicU64::new(0),
            advertising_windows: AtomicU32::new(0),
            advertising_failures: AtomicU32::new(0),
            accepted_connections: AtomicU32::new(0),
            accept_failures: AtomicU32::new(0),
            scanning_windows: AtomicU32::new(0),
            scanning_failures: AtomicU32::new(0),
            dial_attempts: AtomicU32::new(0),
            dialed_connections: AtomicU32::new(0),
            dial_failures: AtomicU32::new(0),
            host_failures: AtomicU32::new(0),
        }
    }

    fn touch(&self) {
        self.last_progress_ms
            .store(embassy_time::Instant::now().as_millis(), Ordering::Relaxed);
    }

    fn set_acceptor_phase(&self, phase: BleTaskPhase) {
        self.acceptor_phase.store(phase as u8, Ordering::Relaxed);
        self.touch();
    }

    fn set_dialer_phase(&self, phase: BleTaskPhase) {
        self.dialer_phase.store(phase as u8, Ordering::Relaxed);
        self.touch();
    }

    fn increment(counter: &AtomicU32) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct BleHub {
    slots: [SlotChannels; PEER_CAPACITY],
    connection_slots: BleSlotPool,
    assign: [Channel<BridgeMutex, SlotJob, 1>; PEER_CAPACITY],
    ready: Channel<BridgeMutex, BleReadySlot, PEER_CAPACITY>,
    dial_failed: Channel<BridgeMutex, [u8; 6], PEER_CAPACITY>,
    sightings: Channel<BridgeMutex, SeenPeer, SIGHTING_DEPTH>,
    dial_request: Channel<BridgeMutex, DialTarget, PEER_CAPACITY>,
    inbound_frames: BleFramePool,
    outbound_frames: BleFramePool,
    radio: RadioArbiter,
    advertise: Signal<BridgeMutex, bool>,
    scan_enabled: Signal<BridgeMutex, bool>,
    local_address: BlockingMutex<BridgeMutex, Cell<[u8; 6]>>,
    diagnostics: BleDiagnostics,
}

impl BleHub {
    pub const fn new() -> Self {
        Self {
            slots: [const { SlotChannels::new() }; PEER_CAPACITY],
            connection_slots: ConnectionSlotPool::new(),
            assign: [const { Channel::new() }; PEER_CAPACITY],
            ready: Channel::new(),
            dial_failed: Channel::new(),
            sightings: Channel::new(),
            dial_request: Channel::new(),
            inbound_frames: SharedFramePool::new(),
            outbound_frames: SharedFramePool::new(),
            radio: FairSemaphore::new(1),
            advertise: Signal::new(),
            scan_enabled: Signal::new(),
            local_address: BlockingMutex::new(Cell::new([0; 6])),
            diagnostics: BleDiagnostics::new(),
        }
    }

    pub fn set_local_address(&self, local_address: [u8; 6]) {
        self.local_address.lock(|cell| cell.set(local_address));
    }

    async fn acquire_radio(&self) -> RadioPermit<'_> {
        loop {
            match self.radio.acquire(1).await {
                Ok(permit) => return permit,
                Err(_) => yield_now().await,
            }
        }
    }

    pub fn backend(&'static self) -> EmbeddedBleBackend {
        EmbeddedBleBackend {
            hub: self,
            ready: self.ready.receiver(),
            dial_failed: self.dial_failed.receiver(),
            sightings: self.sightings.receiver(),
            dial_request: self.dial_request.sender(),
            seen: heapless::Vec::new(),
        }
    }
}

impl Default for BleHub {
    fn default() -> Self {
        Self::new()
    }
}

pub struct EmbeddedBleBackend {
    hub: &'static BleHub,
    ready: Receiver<'static, BridgeMutex, BleReadySlot, PEER_CAPACITY>,
    dial_failed: Receiver<'static, BridgeMutex, [u8; 6], PEER_CAPACITY>,
    sightings: Receiver<'static, BridgeMutex, SeenPeer, SIGHTING_DEPTH>,
    dial_request: Sender<'static, BridgeMutex, DialTarget, PEER_CAPACITY>,
    seen: heapless::Vec<DialTarget, SEEN_CAP>,
}

impl EmbeddedBleBackend {
    fn remember(&mut self, peer: SeenPeer) {
        let target = DialTarget {
            kind: peer.kind,
            addr: peer.addr,
        };
        if self
            .seen
            .iter()
            .any(|seen| seen.addr.into_inner() == peer.addr.into_inner())
        {
            return;
        }
        if self.seen.push(target).is_err() {
            self.seen.remove(0);
            let _ = self.seen.push(target);
        }
    }

    fn resolve(&self, address: BleAddress) -> Option<DialTarget> {
        self.seen
            .iter()
            .find(|seen| seen.addr.into_inner() == *address.octets())
            .copied()
    }
}

impl BleBackend<PEER_CAPACITY> for EmbeddedBleBackend {
    type Error = Closed;
    type Link = EmbeddedBleLink;

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), Closed> {
        self.hub
            .diagnostics
            .advertising_requested
            .store(mode.is_on(), Ordering::Relaxed);
        self.hub.diagnostics.touch();
        self.hub.advertise.signal(mode.is_on());
        Ok(())
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), Closed> {
        self.hub
            .diagnostics
            .scanning_requested
            .store(mode.is_on(), Ordering::Relaxed);
        self.hub.diagnostics.touch();
        self.hub.scan_enabled.signal(mode.is_on());
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<EmbeddedBleLink> {
        match select3(
            self.ready.receive(),
            self.sightings.receive(),
            self.dial_failed.receive(),
        )
        .await
        {
            Either3::First(ready) => {
                let ReadyConnectionSlotParts { origin, link } = ready.into_parts();
                let index = link.index();
                match origin {
                    Origin::Accepted => BleEvent::Inbound(
                        self.hub.slots[index].link(link, &self.hub.outbound_frames),
                    ),
                    Origin::Dialed => BleEvent::LinkReady {
                        link: self.hub.slots[index].link(link, &self.hub.outbound_frames),
                        origin: Origin::Dialed,
                        peer_rssi: None,
                    },
                }
            }
            Either3::Second(peer) => {
                self.remember(peer);
                BleEvent::Sighting {
                    address: BleAddress::new(peer.addr.into_inner()),
                    rssi: Some(peer.rssi),
                }
            }
            Either3::Third(bytes) => BleEvent::DialFailed {
                address: BleAddress::new(bytes),
            },
        }
    }

    async fn dial(&mut self, address: BleAddress) {
        if let Some(target) = self.resolve(address) {
            let _ = self.dial_request.try_send(target);
        }
    }
}

pub struct EmbeddedBleLink {
    peer_protocol: PeerProtocol,
    control_in: Receiver<'static, BridgeMutex, Control, CONTROL_QUEUE_DEPTH>,
    control_out: Sender<'static, BridgeMutex, Control, CONTROL_QUEUE_DEPTH>,
    data_in: Receiver<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    data_out: Sender<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    identity_in: Receiver<'static, BridgeMutex, BleIdentity, 1>,
    identity_out: Sender<'static, BridgeMutex, BleIdentity, 1>,
    data_plane: &'static Signal<BridgeMutex, L2capPlan>,
    plan: L2capPlan,
    address: [u8; 6],
    outbound_frames: &'static BleFramePool,
    slot: BleSlotLink,
}

impl BleLink for EmbeddedBleLink {
    type Error = Closed;
    type Source = EmbeddedBleSource;
    type Sink = EmbeddedBleSink;

    fn peer_protocol(&self) -> PeerProtocol {
        self.peer_protocol
    }

    fn address(&self) -> BleAddress {
        BleAddress::new(self.address)
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), Closed> {
        match select(self.control_out.send(*msg), self.slot.wait_for_close()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn control_recv(&mut self) -> Result<Control, Closed> {
        match select(self.control_in.receive(), self.slot.wait_for_close()).await {
            Either::First(msg) => Ok(msg),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, Closed> {
        match select(self.identity_in.receive(), self.slot.wait_for_close()).await {
            Either::First(identity) => Ok(identity),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn send_columba_identity(&mut self, identity: BleIdentity) -> Result<(), Closed> {
        match select(self.identity_out.send(identity), self.slot.wait_for_close()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), Closed> {
        self.plan = *plan;
        Ok(())
    }

    fn into_data(self) -> (EmbeddedBleSource, EmbeddedBleSink) {
        self.data_plane.signal(self.plan);
        let ConnectionSlotDataOwners {
            source: source_slot,
            sink: sink_slot,
        } = self.slot.into_data();
        (
            EmbeddedBleSource {
                data_in: self.data_in,
                slot: source_slot,
            },
            EmbeddedBleSink {
                data_out: self.data_out,
                frames: self.outbound_frames,
                slot: sink_slot,
            },
        )
    }
}

pub struct EmbeddedBleSource {
    data_in: Receiver<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    slot: BleSlotSource,
}

impl BleSource for EmbeddedBleSource {
    type Error = Closed;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Closed> {
        match select(self.data_in.receive(), self.slot.wait_for_close()).await {
            Either::First(frame) => {
                let frame = frame.lock().await;
                let len = frame.len().min(out.len());
                out[..len].copy_from_slice(&frame[..len]);
                Ok(len)
            }
            Either::Second(()) => Err(Closed),
        }
    }
}

pub struct EmbeddedBleSink {
    data_out: Sender<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    frames: &'static BleFramePool,
    slot: BleSlotSink,
}

impl BleSink for EmbeddedBleSink {
    type Error = Closed;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Closed> {
        let lease = match select(self.frames.lease(), self.slot.wait_for_close()).await {
            Either::First(lease) => lease,
            Either::Second(()) => return Err(Closed),
        };
        lease.fill(frame).await.map_err(|_| Closed)?;
        match select(self.data_out.send(lease), self.slot.wait_for_close()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }
}

struct ScanFunnel {
    sightings: Sender<'static, BridgeMutex, SeenPeer, SIGHTING_DEPTH>,
    local_address: BleAddress,
}

impl EventHandler for ScanFunnel {
    fn on_adv_reports(&self, reports: LeAdvReportsIter) {
        for report in reports {
            let Ok(report) = report else { continue };
            let peer_address = BleAddress::new(report.addr.into_inner());
            let capabilities =
                columba_role_capabilities(report.data).unwrap_or(BleRoleCapabilities::DualRole);
            let should_dial = columba_connection_role(
                self.local_address,
                BleRoleCapabilities::DualRole,
                peer_address,
                capabilities,
            ) == ColumbaConnectionRole::Dial;
            if contains_service(report.data) && should_dial {
                let _ = self.sightings.try_send(SeenPeer {
                    kind: report.addr_kind,
                    addr: report.addr,
                    rssi: report.rssi,
                });
            }
        }
    }
}

pub fn reticulum_attribute_table(
    control_store: &'static mut [u8; GATT_VALUE_CAP],
    data_store: &'static mut [u8; GATT_VALUE_CAP],
    columba_rx_store: &'static mut [u8; GATT_VALUE_CAP],
    columba_tx_store: &'static mut [u8; GATT_VALUE_CAP],
    columba_identity_store: &'static mut [u8; GATT_VALUE_CAP],
    identity: BleIdentity,
) -> Option<(
    ReticulumAttributeTable,
    GattCharacteristic,
    GattCharacteristic,
    GattCharacteristic,
    GattCharacteristic,
)> {
    let mut table: ReticulumAttributeTable = AttributeTable::new();
    if let Err(error) = GapConfig::Peripheral(PeripheralConfig {
        name: "Prns",
        appearance: &appearance::UNKNOWN,
    })
    .build(&mut table)
    {
        crate::diagnostic_log::warn!("ble gap config failed: {error}");
        return None;
    }
    let props = [
        CharacteristicProp::Write,
        CharacteristicProp::WriteWithoutResponse,
        CharacteristicProp::Notify,
    ];
    let (control, data, columba_rx, columba_tx) = {
        let mut service = table.add_service(Service::new(service_uuid()));
        let control = service
            .add_characteristic(
                control_uuid(),
                &props[..],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                control_store,
            )
            .build();
        let data = service
            .add_characteristic(
                data_uuid(),
                &props[..],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                data_store,
            )
            .build();
        let columba_rx = service
            .add_characteristic(
                columba_rx_uuid(),
                [
                    CharacteristicProp::Write,
                    CharacteristicProp::WriteWithoutResponse,
                ],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                columba_rx_store,
            )
            .build();
        let columba_tx = service
            .add_characteristic(
                columba_tx_uuid(),
                [CharacteristicProp::Read, CharacteristicProp::Notify],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                columba_tx_store,
            )
            .build();
        let mut identity_value = GattVec::<u8, GATT_VALUE_CAP>::new();
        identity_value.extend_from_slice(identity.as_bytes()).ok()?;
        service
            .add_characteristic(
                columba_identity_uuid(),
                [CharacteristicProp::Read],
                identity_value,
                columba_identity_store,
            )
            .build();
        service.build();
        (control, data, columba_rx, columba_tx)
    };
    Some((table, control, data, columba_rx, columba_tx))
}

fn l2cap_config() -> L2capChannelConfig {
    L2capChannelConfig {
        mtu: Some(L2CAP_SDU_LEN as u16),
        mps: Some(L2CAP_MPS),
        flow_policy: CreditFlowPolicy::default(),
        initial_credits: Some(L2CAP_CREDITS),
    }
}

fn try_queue_inbound_frame(
    pool: &'static BleFramePool,
    queue: &Sender<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    frame: &[u8],
) {
    let Some(lease) = pool.try_lease() else {
        return;
    };
    if lease.try_fill(frame).is_ok() {
        let _ = queue.try_send(lease);
    }
}

async fn l2cap_pump<T: TroubleTransport>(
    stack: &'static TroubleStack<T>,
    channel: L2capChannel<'static, DefaultPacketPool>,
    inbound_frames: &'static BleFramePool,
    data_out_rx: Receiver<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
    data_in_tx: Sender<'static, BridgeMutex, BleFrameLease, FRAME_QUEUE_DEPTH>,
) {
    let (mut writer, mut reader) = channel.split();
    let outbound = async {
        let mut tx = alloc::boxed::Box::new([0u8; L2CAP_SDU_LEN]);
        loop {
            let frame = data_out_rx.receive().await;
            let frame = frame.lock().await;
            let Some(len) = encode_stream_frame(&frame, tx.as_mut()) else {
                continue;
            };
            if writer.send(stack, &tx[..len]).await.is_err() {
                break;
            }
        }
    };
    let inbound = async {
        let mut rx = alloc::boxed::Box::new([0u8; L2CAP_SDU_LEN]);
        loop {
            let read = match reader.receive(stack, rx.as_mut()).await {
                Ok(read) => read,
                Err(_) => break,
            };
            if read < STREAM_FRAME_PREFIX_LEN {
                continue;
            }
            let len = u16::from_be_bytes([rx[0], rx[1]]) as usize;
            let body = &rx[STREAM_FRAME_PREFIX_LEN..read];
            if body.len() < len {
                continue;
            }
            let frame = inbound_frames.lease().await;
            if frame.fill(&body[..len]).await.is_ok() {
                data_in_tx.send(frame).await;
            }
        }
    };
    let _ = select(outbound, inbound).await;
}

#[derive(Clone, Copy)]
pub struct ReticulumGattCharacteristics<'a> {
    pub control: &'a GattCharacteristic,
    pub data: &'a GattCharacteristic,
    pub columba_rx: &'a GattCharacteristic,
    pub columba_tx: &'a GattCharacteristic,
}

#[derive(Clone, Copy)]
pub struct ReticulumGattUuids<'a> {
    pub service: &'a Uuid,
    pub control: &'a Uuid,
    pub data: &'a Uuid,
    pub columba_rx: &'a Uuid,
    pub columba_tx: &'a Uuid,
    pub columba_identity: &'a Uuid,
}

async fn serve_peripheral<T: TroubleTransport>(
    hub: &'static BleHub,
    stack: &'static TroubleStack<T>,
    slot: &'static SlotChannels,
    link: BleSlotLink,
    worker: &BleSlotWorker,
    connection: &GattConnection<'_, '_, DefaultPacketPool>,
    characteristics: ReticulumGattCharacteristics<'_>,
) {
    let ReticulumGattCharacteristics {
        control,
        data,
        columba_rx,
        columba_tx,
    } = characteristics;
    #[cfg(target_arch = "riscv32")]
    {
        let _ = with_timeout(
            CONN_PARAM_UPDATE_TIMEOUT,
            connection
                .raw()
                .update_connection_params(stack, &preferred_conn_params()),
        )
        .await;
    }

    let peer_protocol = loop {
        match connection.next().await {
            GattConnectionEvent::Disconnected { .. } => return,
            GattConnectionEvent::Gatt { event } => {
                let protocol = match &event {
                    GattEvent::Write(write) if write.handle() == control.handle => {
                        Control::decode(write.data()).map(|message| {
                            let _ = slot.control_in.try_send(message);
                            PeerProtocol::Native
                        })
                    }
                    GattEvent::Write(write)
                        if write.handle() == columba_rx.handle && write.data().len() == 16 =>
                    {
                        let mut bytes = [0u8; 16];
                        bytes.copy_from_slice(write.data());
                        let _ = slot.identity_in.try_send(BleIdentity::new(bytes));
                        Some(PeerProtocol::Columba)
                    }
                    _ => None,
                };
                if let Ok(reply) = event.accept() {
                    reply.send().await;
                }
                if let Some(protocol) = protocol {
                    break protocol;
                }
            }
            _ => {}
        }
    };
    slot.set_peer_protocol(peer_protocol);
    hub.ready.send(link.into_ready(Origin::Accepted)).await;

    let control_out_rx = slot.control_out.receiver();
    let data_out_rx = slot.data_out.receiver();
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();

    let inbound = async move {
        let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
        loop {
            match connection.next().await {
                GattConnectionEvent::Disconnected { .. } => break,
                GattConnectionEvent::Gatt { event } => {
                    if let GattEvent::Write(write) = &event {
                        if peer_protocol == PeerProtocol::Native && write.handle() == control.handle
                        {
                            if let Some(message) = Control::decode(write.data()) {
                                let _ = control_in_tx.try_send(message);
                            }
                        } else if (peer_protocol == PeerProtocol::Native
                            && write.handle() == data.handle)
                            || (peer_protocol == PeerProtocol::Columba
                                && write.handle() == columba_rx.handle)
                        {
                            if let Some(fragment) = Fragment::decode(write.data()) {
                                if let Some(frame) = reassembler.absorb(&fragment) {
                                    try_queue_inbound_frame(
                                        &hub.inbound_frames,
                                        &data_in_tx,
                                        frame,
                                    );
                                }
                            }
                        }
                    }
                    if let Ok(reply) = event.accept() {
                        reply.send().await;
                    }
                }
                _ => {}
            }
        }
    };

    let control_outbound = async move {
        if peer_protocol == PeerProtocol::Columba {
            core::future::pending::<()>().await;
        }
        loop {
            let message = control_out_rx.receive().await;
            let mut buf = [0u8; CONTROL_MAX_LEN];
            if let Some(len) = message.encode(&mut buf) {
                let mut value = GattVec::<u8, GATT_VALUE_CAP>::new();
                let _ = value.extend_from_slice(&buf[..len]);
                let _ = with_timeout(NOTIFY_TIMEOUT, control.notify(connection, &value)).await;
            }
        }
    };

    // Heap allocation keeps the L2CAP state machine out of the main-task future arena, which otherwise steals core-0 stack space.
    let data_lane = alloc::boxed::Box::pin(async move {
        let plan = slot.data_plane.wait().await;
        crate::diagnostic_log::debug!("ble: plan (accepted) = {plan:?}");
        let channel = match (peer_protocol, plan) {
            (PeerProtocol::Native, L2capPlan::Accept) => match with_timeout(
                L2CAP_HANDSHAKE_WINDOW,
                L2capChannel::accept(stack, connection.raw(), &[L2CAP_PSM], &l2cap_config()),
            )
            .await
            {
                Ok(Ok(channel)) => Some(channel),
                Ok(Err(e)) => {
                    crate::diagnostic_log::debug!("ble: L2CAP accept err: {e:?}");
                    None
                }
                Err(_) => {
                    crate::diagnostic_log::debug!("ble: L2CAP accept timed out");
                    None
                }
            },
            _ => None,
        };
        match channel {
            Some(channel) => {
                crate::diagnostic_log::debug!("ble: L2CAP up (accepted)");
                l2cap_pump(stack, channel, &hub.inbound_frames, data_out_rx, data_in_tx).await;
            }
            None => loop {
                let frame = data_out_rx.receive().await;
                let frame = frame.lock().await;
                let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                    let Some(len) = fragment.encode(&mut buf) else {
                        continue;
                    };
                    let mut value = GattVec::<u8, GATT_VALUE_CAP>::new();
                    let _ = value.extend_from_slice(&buf[..len]);
                    let characteristic = match peer_protocol {
                        PeerProtocol::Native => data,
                        PeerProtocol::Columba => columba_tx,
                    };
                    match with_timeout(NOTIFY_TIMEOUT, characteristic.notify(connection, &value))
                        .await
                    {
                        Ok(Ok(())) => {}
                        _ => break,
                    }
                    Timer::after(NOTIFY_PACING).await;
                }
            },
        }
    });

    let _ = select4(
        inbound,
        control_outbound,
        data_lane,
        worker.wait_for_close(),
    )
    .await;
}

/// The per-dial GATT client is boxed because its notification pubsub would otherwise inflate the task frame; a discovery failure reports `dial_failed` so policy can back off.
async fn serve_central<T: TroubleTransport>(
    hub: &'static BleHub,
    stack: &'static TroubleStack<T>,
    link: BleSlotLink,
    worker: &BleSlotWorker,
    connection: Connection<'static, DefaultPacketPool>,
    uuids: ReticulumGattUuids<'_>,
) {
    let slot = &hub.slots[link.index()];
    let addr = connection.peer_address().into_inner();
    let fail = || {
        let _ = hub.dial_failed.try_send(addr);
    };

    let client = match with_timeout(
        GATT_SETUP_TIMEOUT,
        GattClient::<TroubleController<T>, DefaultPacketPool, MAX_SERVICES>::new(
            stack,
            &connection,
        ),
    )
    .await
    {
        Ok(Ok(client)) => alloc::boxed::Box::new(client),
        _ => {
            fail();
            return;
        }
    };

    let discovered = with_timeout(GATT_SETUP_TIMEOUT, async {
        let discover = async {
            let services = client.services_by_uuid(uuids.service).await.ok()?;
            let service = services.first()?.clone();
            let native_control: Option<Characteristic<GattVec<u8, GATT_VALUE_CAP>>> = client
                .characteristic_by_uuid(&service, uuids.control)
                .await
                .ok();
            if let Some(control) = native_control {
                let data: Characteristic<GattVec<u8, GATT_VALUE_CAP>> = client
                    .characteristic_by_uuid(&service, uuids.data)
                    .await
                    .ok()?;
                let control_listener = client.subscribe(&control, false).await.ok()?;
                let data_listener = client.subscribe(&data, false).await.ok()?;
                Some((
                    PeerProtocol::Native,
                    control,
                    data,
                    Some(control_listener),
                    data_listener,
                    None,
                ))
            } else {
                let rx: Characteristic<GattVec<u8, GATT_VALUE_CAP>> = client
                    .characteristic_by_uuid(&service, uuids.columba_rx)
                    .await
                    .ok()?;
                let tx: Characteristic<GattVec<u8, GATT_VALUE_CAP>> = client
                    .characteristic_by_uuid(&service, uuids.columba_tx)
                    .await
                    .ok()?;
                let identity: Characteristic<GattVec<u8, GATT_VALUE_CAP>> = client
                    .characteristic_by_uuid(&service, uuids.columba_identity)
                    .await
                    .ok()?;
                let mut bytes = [0u8; 16];
                let read = client
                    .read_characteristic(&identity, &mut bytes)
                    .await
                    .ok()?;
                if read != bytes.len() {
                    return None;
                }
                let data_listener = client.subscribe(&tx, false).await.ok()?;
                Some((
                    PeerProtocol::Columba,
                    rx.clone(),
                    rx,
                    None,
                    data_listener,
                    Some(BleIdentity::new(bytes)),
                ))
            }
        };
        // ATT responses require the client's receive task to run during discovery.
        match select(discover, client.task()).await {
            Either::First(Some(parts)) => Some(parts),
            _ => None,
        }
    })
    .await;
    let (peer_protocol, control, data, control_listener, mut data_listener, peer_identity) =
        match discovered {
            Ok(Some(parts)) => parts,
            _ => {
                fail();
                return;
            }
        };

    slot.set_peer_addr(addr);
    slot.set_peer_protocol(peer_protocol);
    if let Some(peer_identity) = peer_identity {
        let _ = slot.identity_in.try_send(peer_identity);
    }
    let phy_2m = with_timeout(PHY_UPDATE_TIMEOUT, connection.set_phy(stack, PhyKind::Le2M)).await;
    crate::diagnostic_log::debug!("ble: 2M PHY request ok={}", matches!(phy_2m, Ok(Ok(()))));
    let mut reassembler = alloc::boxed::Box::new(Reassembler::<GATT_REASSEMBLY_CAP>::new());
    let control_out_rx = slot.control_out.receiver();
    let data_out_rx = slot.data_out.receiver();
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();
    let data_in_tx_l2cap = slot.data_in.sender();
    hub.ready.send(link.into_ready(Origin::Dialed)).await;

    let inbound = async {
        match control_listener {
            Some(mut control_listener) => loop {
                match select(control_listener.next(), data_listener.next()).await {
                    Either::First(notification) => {
                        if let Some(message) = Control::decode(notification.as_ref()) {
                            let _ = control_in_tx.try_send(message);
                        }
                    }
                    Either::Second(notification) => {
                        if let Some(fragment) = Fragment::decode(notification.as_ref()) {
                            if let Some(frame) = reassembler.absorb(&fragment) {
                                try_queue_inbound_frame(&hub.inbound_frames, &data_in_tx, frame);
                            }
                        }
                    }
                }
            },
            None => loop {
                let notification = data_listener.next().await;
                if let Some(fragment) = Fragment::decode(notification.as_ref()) {
                    if let Some(frame) = reassembler.absorb(&fragment) {
                        try_queue_inbound_frame(&hub.inbound_frames, &data_in_tx, frame);
                    }
                }
            },
        }
    };

    let control_outbound = async {
        if peer_protocol == PeerProtocol::Columba {
            let identity = slot.identity_out.receive().await;
            let _ = with_timeout(
                NOTIFY_TIMEOUT,
                client.write_characteristic(&control, identity.as_bytes()),
            )
            .await;
            core::future::pending::<()>().await;
        }
        loop {
            let message = control_out_rx.receive().await;
            let mut buf = [0u8; CONTROL_MAX_LEN];
            if let Some(len) = message.encode(&mut buf) {
                let _ = with_timeout(
                    NOTIFY_TIMEOUT,
                    client.write_characteristic_without_response(&control, &buf[..len]),
                )
                .await;
            }
        }
    };

    // Heap allocation keeps the L2CAP state machine out of the main-task future arena and preserves core-0 stack space.
    let data_lane = alloc::boxed::Box::pin(async {
        let plan = slot.data_plane.wait().await;
        crate::diagnostic_log::debug!("ble: plan (dialed) = {plan:?}");
        let channel = match (peer_protocol, plan) {
            (PeerProtocol::Native, L2capPlan::Open { psm }) => {
                let opened = with_timeout(L2CAP_HANDSHAKE_WINDOW, async {
                    loop {
                        match L2capChannel::create(stack, &connection, psm.get(), &l2cap_config())
                            .await
                        {
                            Ok(channel) => break channel,
                            Err(e) => crate::diagnostic_log::debug!("ble: L2CAP create err: {e:?}"),
                        }
                        Timer::after(L2CAP_SETUP_RETRY).await;
                    }
                })
                .await;
                if opened.is_err() {
                    crate::diagnostic_log::debug!(
                        "ble: L2CAP create timed out (peer never accepted)"
                    );
                }
                opened.ok()
            }
            _ => None,
        };
        match channel {
            Some(channel) => {
                crate::diagnostic_log::debug!("ble: L2CAP up (opened)");
                l2cap_pump(
                    stack,
                    channel,
                    &hub.inbound_frames,
                    data_out_rx,
                    data_in_tx_l2cap,
                )
                .await;
            }
            None => loop {
                let frame = data_out_rx.receive().await;
                let frame = frame.lock().await;
                let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                    let Some(len) = fragment.encode(&mut buf) else {
                        continue;
                    };
                    match with_timeout(
                        NOTIFY_TIMEOUT,
                        client.write_characteristic_without_response(&data, &buf[..len]),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        _ => break,
                    }
                    Timer::after(NOTIFY_PACING).await;
                }
            },
        }
    });

    let _ = select4(
        client.task(),
        inbound,
        select(control_outbound, data_lane),
        worker.wait_for_close(),
    )
    .await;
}

pub async fn serve_slot<T: TroubleTransport>(
    idx: usize,
    hub: &'static BleHub,
    stack: &'static TroubleStack<T>,
    server: &GattServer,
    characteristics: ReticulumGattCharacteristics<'_>,
    uuids: ReticulumGattUuids<'_>,
) {
    let slot = &hub.slots[idx];
    loop {
        let job = hub.assign[idx].receive().await;
        slot.clear_lanes();
        match job {
            SlotJob::Accept {
                connection,
                slot: lease,
            } => {
                let ConnectionSlotOwners { worker, link } = lease.activate();
                slot.set_peer_addr(connection.peer_address().into_inner());
                match connection.with_attribute_server(server) {
                    Ok(connection) => {
                        serve_peripheral(
                            hub,
                            stack,
                            slot,
                            link,
                            &worker,
                            &connection,
                            characteristics,
                        )
                        .await;
                    }
                    Err(error) => {
                        crate::diagnostic_log::warn!("ble attribute server bind failed: {error:?}")
                    }
                }
            }
            SlotJob::Dial {
                connection,
                slot: lease,
            } => {
                let ConnectionSlotOwners { worker, link } = lease.activate();
                serve_central(hub, stack, link, &worker, connection, uuids).await;
            }
        }
        hub.diagnostics.active_slots.fetch_sub(1, Ordering::Relaxed);
        hub.diagnostics.touch();
    }
}

pub async fn acceptor<T: TroubleTransport>(
    hub: &'static BleHub,
    peripheral: &mut Peripheral<'static, TroubleController<T>, DefaultPacketPool>,
    adv_data: &[u8],
) {
    let mut enabled = false;
    hub.diagnostics.set_acceptor_phase(BleTaskPhase::Disabled);
    loop {
        if !enabled {
            enabled = hub.advertise.wait().await;
            continue;
        }
        hub.diagnostics
            .set_acceptor_phase(BleTaskPhase::WaitingForSlot);
        let lease = match select(hub.connection_slots.acquire(), hub.advertise.wait()).await {
            Either::First(lease) => lease,
            Either::Second(state) => {
                enabled = state;
                if !enabled {
                    hub.diagnostics.set_acceptor_phase(BleTaskPhase::Disabled);
                }
                continue;
            }
        };
        let idx = lease.index();
        hub.diagnostics
            .set_acceptor_phase(BleTaskPhase::WaitingForRadio);
        let radio = hub.acquire_radio().await;
        hub.diagnostics.set_acceptor_phase(BleTaskPhase::Starting);
        let advertiser = match peripheral
            .advertise(
                &advertisement_parameters(),
                Advertisement::ConnectableScannableUndirected {
                    adv_data,
                    scan_data: &[],
                },
            )
            .await
        {
            Ok(advertiser) => advertiser,
            Err(error) => {
                BleDiagnostics::increment(&hub.diagnostics.advertising_failures);
                hub.diagnostics.set_acceptor_phase(BleTaskPhase::BackingOff);
                crate::diagnostic_log::warn!("ble advertise failed: {error:?}");
                drop(radio);
                Timer::after(Duration::from_millis(500)).await;
                continue;
            }
        };
        BleDiagnostics::increment(&hub.diagnostics.advertising_windows);
        hub.diagnostics.set_acceptor_phase(BleTaskPhase::Active);
        match select3(
            advertiser.accept(),
            Timer::after(ADV_WINDOW),
            hub.advertise.wait(),
        )
        .await
        {
            Either3::First(Ok(connection)) => {
                if hub.assign[idx]
                    .try_send(SlotJob::Accept {
                        connection,
                        slot: lease,
                    })
                    .is_err()
                {
                    BleDiagnostics::increment(&hub.diagnostics.accept_failures);
                } else {
                    hub.diagnostics.active_slots.fetch_add(1, Ordering::Relaxed);
                    BleDiagnostics::increment(&hub.diagnostics.accepted_connections);
                }
            }
            Either3::First(Err(error)) => {
                BleDiagnostics::increment(&hub.diagnostics.accept_failures);
                crate::diagnostic_log::warn!("ble accept failed: {error:?}");
            }
            Either3::Second(()) => {}
            Either3::Third(state) => {
                enabled = state;
            }
        }
        hub.diagnostics
            .set_acceptor_phase(BleTaskPhase::Dispatching);
        drop(radio);
        #[cfg(target_arch = "riscv32")]
        Timer::after(DISCOVERY_TURN_REST).await;
    }
}

pub async fn dialer<T: TroubleTransport>(
    hub: &'static BleHub,
    mut central: Central<'static, TroubleController<T>, DefaultPacketPool>,
) {
    let mut enabled = false;
    hub.diagnostics.set_dialer_phase(BleTaskPhase::Disabled);
    loop {
        if !enabled {
            enabled = hub.scan_enabled.wait().await;
            continue;
        }
        hub.diagnostics
            .set_dialer_phase(BleTaskPhase::WaitingForRadio);
        let radio = hub.acquire_radio().await;
        hub.diagnostics.set_dialer_phase(BleTaskPhase::Starting);
        let mut scanner = Scanner::new(central);
        let target = {
            match scanner
                .scan(&ScanConfig {
                    active: false,
                    interval: IDLE_SCAN_INTERVAL,
                    window: IDLE_SCAN_WINDOW,
                    ..Default::default()
                })
                .await
            {
                Ok(_session) => {
                    BleDiagnostics::increment(&hub.diagnostics.scanning_windows);
                    hub.diagnostics.set_dialer_phase(BleTaskPhase::Active);
                    match select3(
                        hub.dial_request.receive(),
                        Timer::after(SCAN_WINDOW),
                        hub.scan_enabled.wait(),
                    )
                    .await
                    {
                        Either3::First(target) => Some(target),
                        Either3::Second(()) => None,
                        Either3::Third(state) => {
                            enabled = state;
                            if !enabled {
                                hub.diagnostics.set_dialer_phase(BleTaskPhase::Disabled);
                            }
                            None
                        }
                    }
                }
                Err(error) => {
                    BleDiagnostics::increment(&hub.diagnostics.scanning_failures);
                    hub.diagnostics.set_dialer_phase(BleTaskPhase::BackingOff);
                    crate::diagnostic_log::warn!("ble scan failed: {error:?}");
                    Timer::after(Duration::from_millis(500)).await;
                    None
                }
            }
        };
        central = scanner.into_inner();
        if let Some(target) = target {
            let Some(lease) = hub.connection_slots.try_acquire() else {
                hub.diagnostics.set_dialer_phase(BleTaskPhase::Dispatching);
                drop(radio);
                #[cfg(target_arch = "riscv32")]
                Timer::after(DISCOVERY_TURN_REST).await;
                continue;
            };
            let idx = lease.index();
            let bd = target.addr;
            let whitelist = [(target.kind, &bd)];
            let mut config = ConnectConfig {
                scan_config: ScanConfig {
                    active: false,
                    filter_accept_list: &whitelist,
                    ..Default::default()
                },
                connect_params: preferred_conn_params(),
            };
            config.scan_config.timeout = CONNECT_TIMEOUT;
            config.scan_config.interval = CONNECT_SCAN_INTERVAL;
            config.scan_config.window = CONNECT_SCAN_WINDOW;
            BleDiagnostics::increment(&hub.diagnostics.dial_attempts);
            hub.diagnostics.set_dialer_phase(BleTaskPhase::Connecting);
            match central.connect(&config).await {
                Ok(connection) => {
                    if hub.assign[idx]
                        .try_send(SlotJob::Dial {
                            connection,
                            slot: lease,
                        })
                        .is_err()
                    {
                        BleDiagnostics::increment(&hub.diagnostics.dial_failures);
                    } else {
                        hub.diagnostics.active_slots.fetch_add(1, Ordering::Relaxed);
                        BleDiagnostics::increment(&hub.diagnostics.dialed_connections);
                    }
                }
                Err(_) => {
                    BleDiagnostics::increment(&hub.diagnostics.dial_failures);
                    let _ = hub.dial_failed.try_send(bd.into_inner());
                }
            }
        }
        hub.diagnostics.set_dialer_phase(BleTaskPhase::Dispatching);
        drop(radio);
        #[cfg(target_arch = "riscv32")]
        Timer::after(DISCOVERY_TURN_REST).await;
    }
}

pub async fn host_runner<T: TroubleTransport>(
    hub: &'static BleHub,
    mut runner: Runner<'static, TroubleController<T>, DefaultPacketPool>,
) {
    let funnel = ScanFunnel {
        sightings: hub.sightings.sender(),
        local_address: BleAddress::new(hub.local_address.lock(|cell| cell.get())),
    };
    loop {
        if let Err(error) = runner.run_with_handler(&funnel).await {
            BleDiagnostics::increment(&hub.diagnostics.host_failures);
            hub.diagnostics.touch();
            crate::diagnostic_log::warn!("ble host runner exited: {error:?}");
            Timer::after(Duration::from_millis(100)).await;
        }
    }
}
