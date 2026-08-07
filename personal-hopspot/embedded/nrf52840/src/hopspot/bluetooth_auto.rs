use core::cell::{Cell, UnsafeCell};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_futures::select::{select, select3, select4, Either, Either3};
use embassy_futures::yield_now;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Channel, Receiver, Sender, TrySendError};
use embassy_sync::semaphore::{FairSemaphore, Semaphore, SemaphoreReleaser};
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Timer};

use nrf_softdevice::ble::{
    central, gatt_client, gatt_server, l2cap, peripheral, Address, Connection, GattError,
};
use nrf_softdevice::{raw, RawError, SocEvent, Softdevice};

use personal_rns::bluetooth_auto::connection_slots::{
    ConnectionSlotDataOwners, ConnectionSlotLease, ConnectionSlotLinkLease, ConnectionSlotOwners,
    ConnectionSlotPool, ConnectionSlotSinkLease, ConnectionSlotSourceLease,
    ConnectionSlotWorkerLease, ReadyConnectionSlot, ReadyConnectionSlotParts,
};
use personal_rns::bluetooth_auto::{
    BluetoothAutoShared, BluetoothAutoStatus, FrameLease, FramePoolError, SharedFramePool,
};
use personal_rns::interfaces::bluetooth_auto::{
    columba_connection_role, columba_role_capabilities, contains_service, encode_advertisement,
    encode_stream_frame, fragments_of, BleAddress, BleIdentity, BleRoleCapabilities,
    ColumbaConnectionRole, Control, Fragment, L2capPlan, PeerProtocol, Reassembler, BLE_HW_MTU,
    CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN, STREAM_FRAME_PREFIX_LEN,
};
use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, DialOutcome, Origin,
    RadioMode, ScanningMode,
};
use personal_rns::interfaces::{InterfaceId, InterfaceKind};

pub(super) use super::bluetooth_gatt_server::Server;
use super::bluetooth_gatt_server::{ServerWrite, WriteDelivery, WriteTarget};

type Mtx = CriticalSectionRawMutex;
type GattValue = heapless09::Vec<u8, 244>;

pub(super) const MEMBERS: usize = NrfBleBackend::MAX_PEERS;
pub(super) const BLE_SUPERVISOR_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);

pub(super) const POOL: usize = MEMBERS + 2;
const _: () = assert!(POOL == 7, "serve_slot pool_size must equal POOL");

const CTRL_DEPTH: usize = 4;
const DATA_TOKEN_DEPTH: usize = 2;
const SHARED_FRAME_CAPACITY: usize = MEMBERS;
const SHARED_FRAME_WAITERS: usize = POOL;
const SIGHTING_DEPTH: usize = 4;
const SEEN_CAP: usize = 8;
const CENTRAL_RADIO_WAITERS: usize = POOL + 1;
type CentralRadio = FairSemaphore<Mtx, CENTRAL_RADIO_WAITERS>;
type CentralRadioPermit<'a> = SemaphoreReleaser<'a, CentralRadio>;
type BleSlotPool = ConnectionSlotPool<Mtx, POOL>;
type BleSlotLease = ConnectionSlotLease<Mtx>;
type BleSlotWorker = ConnectionSlotWorkerLease<Mtx>;
type BleSlotLink = ConnectionSlotLinkLease<Mtx>;
type BleSlotSource = ConnectionSlotSourceLease<Mtx>;
type BleSlotSink = ConnectionSlotSinkLease<Mtx>;
type BleReadySlot = ReadyConnectionSlot<Mtx>;
type SharedFrameLease = FrameLease<Mtx, BLE_HW_MTU, SHARED_FRAME_CAPACITY, SHARED_FRAME_WAITERS>;
type SharedFrames = SharedFramePool<Mtx, BLE_HW_MTU, SHARED_FRAME_CAPACITY, SHARED_FRAME_WAITERS>;
const GATT_FRAGMENT_PAYLOAD: usize = 180;
const GATT_REASSEMBLY_CAP: usize = BLE_HW_MTU;
const GATT_ATTRIBUTE_TABLE_BYTES: u32 = 3072;
const NOTIFY_BACKPRESSURE_RETRY: Duration = Duration::from_millis(1);
const PREFERRED_MIN_CONN_INTERVAL: u16 = 12;
const PREFERRED_MAX_CONN_INTERVAL: u16 = 24;
const PREFERRED_SLAVE_LATENCY: u16 = 0;
const PREFERRED_SUPERVISION_TIMEOUT: u16 = 400;
const SIGHTING_PACING: Duration = Duration::from_millis(200);
const SCAN_ERROR_BACKOFF: Duration = Duration::from_millis(500);
/// One scan window before the scanner releases the central-radio permit (10 ms units), so a pending
/// dial never waits longer than this for the radio. With no dial waiting the scanner re-takes it.
const SCAN_WINDOW_TICKS: u16 = 200;
const IDLE_SCAN_INTERVAL: u32 = 1600;
const IDLE_SCAN_WINDOW: u32 = 80;
/// How long a dial scans for its whitelisted peer before giving up (10 ms units). `central::connect`
/// defaults to scanning *forever*, so without this a dial to a peer that has stopped advertising holds
/// the central-radio permit indefinitely and starves both the scanner and every other dial.
const CONNECT_WINDOW_TICKS: u16 = 300;
const CONNECT_SCAN_INTERVAL: u32 = 160;
const CONNECT_SCAN_WINDOW: u32 = 128;

const L2CAP_PSM: u16 = 0x0080;
const L2CAP_MTU: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;
const L2CAP_MPS: u16 = 247;
const L2CAP_RX_QUEUE: u8 = 2;
const L2CAP_TX_QUEUE: u8 = 2;
const L2CAP_CREDITS: u16 = 4;
const L2CAP_POOL: usize = MEMBERS + 4;
const L2CAP_HANDSHAKE_WINDOW: Duration = Duration::from_secs(5);
const L2CAP_SETUP_RETRY: Duration = Duration::from_millis(150);

struct L2capPool {
    buffers: [UnsafeCell<[u8; L2CAP_MTU]>; L2CAP_POOL],
    free: [AtomicBool; L2CAP_POOL],
}

// SAFETY: A slot is handed out only after its AtomicBool changes true -> false with AcqRel, and it
// returns to the pool only when its unique L2capPacket is dropped. No two threads can access the
// same UnsafeCell while it is claimed.
unsafe impl Sync for L2capPool {}

static L2CAP_POOL_STORE: L2capPool = L2capPool {
    buffers: [const { UnsafeCell::new([0u8; L2CAP_MTU]) }; L2CAP_POOL],
    free: [const { AtomicBool::new(true) }; L2CAP_POOL],
};

impl L2capPool {
    fn claim(&self) -> Option<NonNull<u8>> {
        for slot in 0..L2CAP_POOL {
            if self.free[slot].swap(false, Ordering::AcqRel) {
                return NonNull::new(self.buffers[slot].get().cast());
            }
        }
        None
    }

    fn release(&self, ptr: NonNull<u8>) {
        let base = self.buffers.as_ptr() as usize;
        let slot =
            (ptr.as_ptr() as usize - base) / core::mem::size_of::<UnsafeCell<[u8; L2CAP_MTU]>>();
        if slot < L2CAP_POOL {
            self.free[slot].store(true, Ordering::Release);
        }
    }
}

pub(super) struct L2capPacket {
    ptr: NonNull<u8>,
    len: usize,
}

impl L2capPacket {
    fn from_frame(frame: &[u8]) -> Option<Self> {
        let ptr = L2CAP_POOL_STORE.claim()?;
        // SAFETY: `claim` uniquely reserves this entire fixed-size pool slot until `L2capPacket`
        // releases it, and the pointer is aligned and valid for exactly L2CAP_MTU bytes.
        let buf = unsafe { core::slice::from_raw_parts_mut(ptr.as_ptr(), L2CAP_MTU) };
        match encode_stream_frame(frame, buf) {
            Some(len) => Some(Self { ptr, len }),
            None => {
                L2CAP_POOL_STORE.release(ptr);
                None
            }
        }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: `self` owns the claimed pool slot and `len` was produced by the bounded encoder
        // (or the L2CAP implementation under the from_raw_parts contract), so it is within the slot.
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl l2cap::Packet for L2capPacket {
    const MTU: usize = L2CAP_MTU;

    fn allocate() -> Option<NonNull<u8>> {
        L2CAP_POOL_STORE.claim()
    }

    fn into_raw_parts(self) -> (NonNull<u8>, usize) {
        let parts = (self.ptr, self.len);
        core::mem::forget(self);
        parts
    }

    /// # Safety
    ///
    /// `ptr` must be a uniquely claimed slot from `L2CAP_POOL_STORE`, and `len` must not exceed
    /// `L2CAP_MTU`. Ownership transfers to the returned packet, which releases the slot on drop.
    unsafe fn from_raw_parts(ptr: NonNull<u8>, len: usize) -> Self {
        Self { ptr, len }
    }
}

impl Drop for L2capPacket {
    fn drop(&mut self) {
        L2CAP_POOL_STORE.release(self.ptr);
    }
}

pub(super) static OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();

pub(super) static BLE_SHARED: BluetoothAutoShared<MEMBERS> =
    BluetoothAutoShared::new(BLE_SUPERVISOR_ID);
static INBOUND_FRAMES: SharedFrames = SharedFramePool::new();
static OUTBOUND_FRAMES: SharedFrames = SharedFramePool::new();

#[derive(Debug, PartialEq, Eq)]
enum IngressPressure {
    SharedPoolExhausted,
    TokenQueueFull,
    ControlQueueFull,
    FrameTooLarge,
    PoolUnavailable,
}

#[derive(Debug, PartialEq, Eq)]
enum IngressAdmission {
    Admitted,
    Pressured(IngressPressure),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Closed;

#[embassy_executor::task]
pub(super) async fn softdevice_task(
    sd: &'static Softdevice,
    vbus: &'static SoftwareVbusDetect,
) -> ! {
    sd.run_with_callback(|event| match event {
        SocEvent::PowerUsbDetected => vbus.detected(true),
        SocEvent::PowerUsbPowerReady => vbus.ready(),
        SocEvent::PowerUsbRemoved => vbus.detected(false),
        _ => {}
    })
    .await
}

#[nrf_softdevice::gatt_client(uuid = "37145b00-442d-4a94-917f-8f42c5da28e3")]
struct NativeReticulumClient {
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e7", write, notify)]
    control: GattValue,
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e8", write, notify)]
    data: GattValue,
}

#[nrf_softdevice::gatt_client(uuid = "37145b00-442d-4a94-917f-8f42c5da28e3")]
struct ColumbaReticulumClient {
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e4", read, notify)]
    tx: GattValue,
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e5", write)]
    rx: GattValue,
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e6", read)]
    identity: GattValue,
}

pub(super) fn set_columba_identity(sd: &Softdevice, server: &Server, identity: BleIdentity) {
    let _ = server.set_columba_identity(sd, identity.as_bytes());
}

pub(super) fn softdevice_config() -> nrf_softdevice::Config {
    nrf_softdevice::Config {
        clock: Some(raw::nrf_clock_lf_cfg_t {
            source: raw::NRF_CLOCK_LF_SRC_RC as u8,
            rc_ctiv: 16,
            rc_temp_ctiv: 2,
            accuracy: raw::NRF_CLOCK_LF_ACCURACY_500_PPM as u8,
        }),
        // The radio carries up to POOL concurrent links; conn_count is the SoftDevice's total
        // connection reservation (the role counts are per-role sub-caps, not the total). event_length
        // is the per-interval airtime each link is guaranteed.
        conn_gap: Some(raw::ble_gap_conn_cfg_t {
            conn_count: POOL as u8,
            event_length: 6,
        }),
        conn_gatt: Some(raw::ble_gatt_conn_cfg_t { att_mtu: 247 }),
        gatts_attr_tab_size: Some(raw::ble_gatts_cfg_attr_tab_size_t {
            attr_tab_size: GATT_ATTRIBUTE_TABLE_BYTES,
        }),
        conn_l2cap: Some(raw::ble_l2cap_conn_cfg_t {
            ch_count: 1,
            rx_mps: L2CAP_MPS,
            tx_mps: L2CAP_MPS,
            rx_queue_size: L2CAP_RX_QUEUE,
            tx_queue_size: L2CAP_TX_QUEUE,
        }),
        // Symmetric dual-role: BLE_MEMBERS peripheral slots (peers dial us) AND BLE_MEMBERS central
        // slots (we dial), so any settled peer can take either side — the keeper duel resolves each
        // link's role by identity, ~half each way, so both counts must cover the whole settled pool.
        // periph + central = 20 is the SoftDevice's combined ceiling.
        gap_role_count: Some(raw::ble_gap_cfg_role_count_t {
            adv_set_count: 1,
            periph_role_count: MEMBERS as u8,
            central_role_count: MEMBERS as u8,
            central_sec_count: 0,
            _bitfield_1: raw::ble_gap_cfg_role_count_t::new_bitfield_1(0),
        }),
        ..Default::default()
    }
}

pub(super) fn usb_vbus_present() -> bool {
    let mut status = 0u32;
    // SAFETY: `status` is a live, aligned u32 out-parameter for the duration of the synchronous
    // SoftDevice SVC; the SoftDevice has been enabled before this backend is queried.
    (unsafe { raw::sd_power_usbregstatus_get(&mut status) }) == raw::NRF_SUCCESS
        && status & 0x1 != 0
}

#[derive(Clone, Copy)]
struct SeenPeer {
    address: Address,
    rssi: i8,
}

struct LinkChannels {
    control_in: Channel<Mtx, Control, CTRL_DEPTH>,
    control_out: Channel<Mtx, Control, CTRL_DEPTH>,
    data_in: Channel<Mtx, SharedFrameLease, DATA_TOKEN_DEPTH>,
    data_out: Channel<Mtx, SharedFrameLease, DATA_TOKEN_DEPTH>,
    acknowledged_writes: Channel<Mtx, ServerWrite, 1>,
    identity_in: Signal<Mtx, BleIdentity>,
    identity_out: Channel<Mtx, BleIdentity, 1>,
    data_plane: Signal<Mtx, L2capPlan>,
    profile_ready: Signal<Mtx, PeerProtocol>,
    /// The connected peer's address, stashed by the slot worker the moment the connection lands (from
    /// `conn.peer_address()` for an accept, the dialed address for a dial) and read by [`link`](Self::link)
    /// so the supervisor's brain keys this peer correctly — it keys settled-peer lookup and dial/suppress
    /// backoff by address, so a stale all-zero address makes every peer collide on one backoff entry and
    /// hides an already-settled peer from sighting suppression (the redundant self-dial).
    address: BlockingMutex<Mtx, Cell<[u8; 6]>>,
    peer_protocol: BlockingMutex<Mtx, Cell<Option<PeerProtocol>>>,
}

impl LinkChannels {
    const fn new() -> Self {
        Self {
            control_in: Channel::new(),
            control_out: Channel::new(),
            data_in: Channel::new(),
            data_out: Channel::new(),
            acknowledged_writes: Channel::new(),
            identity_in: Signal::new(),
            identity_out: Channel::new(),
            data_plane: Signal::new(),
            profile_ready: Signal::new(),
            address: BlockingMutex::new(Cell::new([0u8; 6])),
            peer_protocol: BlockingMutex::new(Cell::new(None)),
        }
    }

    fn set_address(&self, bytes: [u8; 6]) {
        self.address.lock(|address| address.set(bytes));
    }

    fn address(&self) -> [u8; 6] {
        self.address.lock(|address| address.get())
    }

    fn set_peer_protocol(&self, protocol: PeerProtocol) {
        self.peer_protocol
            .lock(|current| current.set(Some(protocol)));
        self.profile_ready.signal(protocol);
    }

    fn peer_protocol(&self) -> Option<PeerProtocol> {
        self.peer_protocol.lock(|current| current.get())
    }

    fn reset(&self) {
        self.data_plane.reset();
        self.profile_ready.reset();
        self.peer_protocol.lock(|current| current.set(None));
        self.control_in.clear();
        self.control_out.clear();
        self.data_in.clear();
        self.data_out.clear();
        self.acknowledged_writes.clear();
        self.identity_in.reset();
        self.identity_out.clear();
    }

    fn link(&'static self, slot: BleSlotLink) -> NrfBleLink {
        NrfBleLink {
            peer_protocol: self.peer_protocol().unwrap_or(PeerProtocol::Native),
            control_in: self.control_in.receiver(),
            control_out: self.control_out.sender(),
            data_in: self.data_in.receiver(),
            data_out: self.data_out.sender(),
            identity_in: &self.identity_in,
            identity_out: self.identity_out.sender(),
            data_plane: &self.data_plane,
            plan: L2capPlan::None,
            address: self.address(),
            slot,
        }
    }
}

enum SlotJob {
    Accept {
        connection: Connection,
        slot: BleSlotLease,
    },
    Dial {
        address: Address,
        slot: BleSlotLease,
    },
}

pub(super) struct BleHub {
    slots: [LinkChannels; POOL],
    connection_slots: BleSlotPool,
    assign: [Channel<Mtx, SlotJob, 1>; POOL],
    ready: Channel<Mtx, BleReadySlot, POOL>,
    dial_failed: Channel<Mtx, [u8; 6], POOL>,
    /// The central-radio permit: a single token both the scanner and each dial must hold while using
    /// the SoftDevice's one scanner. `central::scan` and `central::connect` (which scans to find the
    /// whitelisted peer) cannot run at once — overlapping them fails the connect and can panic the
    /// shared connect portal — so this serializes them: one scan-or-dial on the radio at a time.
    central_radio: CentralRadio,
    advertise: Signal<Mtx, bool>,
    sightings: Channel<Mtx, SeenPeer, SIGHTING_DEPTH>,
    scan_enabled: Signal<Mtx, bool>,
    radio_enabled: AtomicBool,
}

impl BleHub {
    const fn new() -> Self {
        Self {
            slots: [const { LinkChannels::new() }; POOL],
            connection_slots: ConnectionSlotPool::new(),
            assign: [const { Channel::new() }; POOL],
            ready: Channel::new(),
            dial_failed: Channel::new(),
            central_radio: FairSemaphore::new(1),
            advertise: Signal::new(),
            sightings: Channel::new(),
            scan_enabled: Signal::new(),
            radio_enabled: AtomicBool::new(false),
        }
    }

    async fn acquire_central_radio(&self) -> CentralRadioPermit<'_> {
        loop {
            match self.central_radio.acquire(1).await {
                Ok(permit) => return permit,
                Err(_) => yield_now().await,
            }
        }
    }
}

pub(super) static HUB: BleHub = BleHub::new();

pub(super) struct NrfBleBackend {
    ready: Receiver<'static, Mtx, BleReadySlot, POOL>,
    dial_failed: Receiver<'static, Mtx, [u8; 6], POOL>,
    sightings: Receiver<'static, Mtx, SeenPeer, SIGHTING_DEPTH>,
    seen: heapless::Vec<Address, SEEN_CAP>,
    hub: &'static BleHub,
}

impl NrfBleBackend {
    pub(super) const MAX_PEERS: usize = 5;

    pub(super) fn new(hub: &'static BleHub) -> Self {
        Self {
            ready: hub.ready.receiver(),
            dial_failed: hub.dial_failed.receiver(),
            sightings: hub.sightings.receiver(),
            seen: heapless::Vec::new(),
            hub,
        }
    }

    fn remember(&mut self, address: Address) {
        if self.seen.iter().any(|seen| seen.bytes() == address.bytes()) {
            return;
        }
        if self.seen.push(address).is_err() {
            self.seen.remove(0);
            let _ = self.seen.push(address);
        }
    }

    fn resolve(&self, address: BleAddress) -> Option<Address> {
        self.seen
            .iter()
            .find(|seen| seen.bytes() == *address.octets())
            .copied()
    }
}

impl BleBackend<{ NrfBleBackend::MAX_PEERS }> for NrfBleBackend {
    type Error = Closed;
    type Link = NrfBleLink;

    async fn set_advertising(&mut self, mode: AdvertisingMode) -> Result<(), Closed> {
        self.hub.advertise.signal(mode.is_on());
        Ok(())
    }

    async fn set_scanning(&mut self, mode: ScanningMode) -> Result<(), Closed> {
        self.hub.scan_enabled.signal(mode.is_on());
        Ok(())
    }

    async fn set_radio_mode(&mut self, mode: RadioMode) -> Result<(), Closed> {
        let enabled = mode.is_on();
        self.hub.radio_enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.hub.advertise.signal(false);
            self.hub.scan_enabled.signal(false);
            for (index, assign) in self.hub.assign.iter().enumerate() {
                self.hub.connection_slots.request_close(index);
                assign.clear();
            }
            self.hub.ready.clear();
            self.hub.dial_failed.clear();
        }
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<NrfBleLink> {
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
                    Origin::Accepted => BleEvent::Inbound(self.hub.slots[index].link(link)),
                    Origin::Dialed => BleEvent::LinkReady {
                        link: self.hub.slots[index].link(link),
                        origin: Origin::Dialed,
                        peer_rssi: None,
                    },
                }
            }
            Either3::Second(peer) => {
                self.remember(peer.address);
                BleEvent::Sighting {
                    address: BleAddress::new(peer.address.bytes()),
                    rssi: Some(peer.rssi),
                }
            }
            Either3::Third(bytes) => BleEvent::DialFailed {
                address: BleAddress::new(bytes),
            },
        }
    }

    async fn dial(&mut self, address: BleAddress) -> DialOutcome {
        let Some(addr) = self.resolve(address) else {
            return DialOutcome::UnknownPeer;
        };
        if !self.hub.radio_enabled.load(Ordering::Relaxed) {
            return DialOutcome::RadioOff;
        }
        let slot = match self.hub.connection_slots.try_acquire() {
            Ok(Some(slot)) => slot,
            Ok(None) => return DialOutcome::Busy,
            Err(_) => return DialOutcome::InvariantViolation,
        };
        let index = slot.index();
        if self.hub.assign[index]
            .try_send(SlotJob::Dial {
                address: addr,
                slot,
            })
            .is_ok()
        {
            DialOutcome::Started
        } else {
            DialOutcome::InvariantViolation
        }
    }
}

pub(super) struct NrfBleLink {
    peer_protocol: PeerProtocol,
    control_in: Receiver<'static, Mtx, Control, CTRL_DEPTH>,
    control_out: Sender<'static, Mtx, Control, CTRL_DEPTH>,
    data_in: Receiver<'static, Mtx, SharedFrameLease, DATA_TOKEN_DEPTH>,
    data_out: Sender<'static, Mtx, SharedFrameLease, DATA_TOKEN_DEPTH>,
    identity_in: &'static Signal<Mtx, BleIdentity>,
    identity_out: Sender<'static, Mtx, BleIdentity, 1>,
    data_plane: &'static Signal<Mtx, L2capPlan>,
    plan: L2capPlan,
    address: [u8; 6],
    slot: BleSlotLink,
}

impl BleLink for NrfBleLink {
    type Error = Closed;
    type Source = NrfBleSource;
    type Sink = NrfBleSink;

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
        match select(self.identity_in.wait(), self.slot.wait_for_close()).await {
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

    fn into_data(self) -> (NrfBleSource, NrfBleSink) {
        self.data_plane.signal(self.plan);
        let ConnectionSlotDataOwners {
            source: source_slot,
            sink: sink_slot,
        } = self.slot.into_data();
        (
            NrfBleSource {
                data_in: self.data_in,
                slot: source_slot,
            },
            NrfBleSink {
                data_out: self.data_out,
                slot: sink_slot,
            },
        )
    }
}

pub(super) struct NrfBleSource {
    data_in: Receiver<'static, Mtx, SharedFrameLease, DATA_TOKEN_DEPTH>,
    slot: BleSlotSource,
}

impl BleSource for NrfBleSource {
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

pub(super) struct NrfBleSink {
    data_out: Sender<'static, Mtx, SharedFrameLease, DATA_TOKEN_DEPTH>,
    slot: BleSlotSink,
}

impl BleSink for NrfBleSink {
    type Error = Closed;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Closed> {
        let admission = async {
            let lease = OUTBOUND_FRAMES.lease().await.map_err(|_| Closed)?;
            lease.fill(frame).await.map_err(|_| Closed)?;
            self.data_out.send(lease).await;
            Ok(())
        };
        match select(admission, self.slot.wait_for_close()).await {
            Either::First(result) => result,
            Either::Second(()) => Err(Closed),
        }
    }
}

fn l2cap_config() -> l2cap::Config {
    l2cap::Config {
        credits: L2CAP_CREDITS,
    }
}

fn preferred_conn_params() -> raw::ble_gap_conn_params_t {
    raw::ble_gap_conn_params_t {
        min_conn_interval: PREFERRED_MIN_CONN_INTERVAL,
        max_conn_interval: PREFERRED_MAX_CONN_INTERVAL,
        slave_latency: PREFERRED_SLAVE_LATENCY,
        conn_sup_timeout: PREFERRED_SUPERVISION_TIMEOUT,
    }
}

fn initiate_data_length_extension(conn: &mut Connection) {
    // `None` asks the SoftDevice for the largest data length supported by this build's connection
    // event and RAM configuration. Peers without DLE support retain the mandatory 27-byte floor.
    let _ = conn.data_length_update(None);
}

#[derive(Clone, Copy)]
enum ServerNotification {
    Control,
    NativeData,
    ColumbaData,
}

async fn notify_with_backpressure(
    server: &Server,
    conn: &Connection,
    target: ServerNotification,
    bytes: &[u8],
) -> Result<(), Closed> {
    loop {
        // Keep the 244-byte GATT value inside this synchronous scope. Retaining it across the
        // retry await would inflate every one of the seven pooled slot futures by that amount.
        let result = {
            let value = GattValue::from_slice(bytes).map_err(|_| Closed)?;
            match target {
                ServerNotification::Control => server.notify_control(conn, &value),
                ServerNotification::NativeData => server.notify_native_data(conn, &value),
                ServerNotification::ColumbaData => server.notify_columba_data(conn, &value),
            }
        };
        match result {
            Ok(()) => return Ok(()),
            Err(gatt_server::NotifyValueError::Raw(RawError::Resources)) => {
                // S140's default notification queue is intentionally one entry: increasing it for
                // all seven reserved connections would consume RAM the T-Echo does not have. Yield
                // until the next link event drains that entry, then submit this same fragment.
                Timer::after(NOTIFY_BACKPRESSURE_RETRY).await;
            }
            Err(_) => return Err(Closed),
        }
    }
}

fn record_ingress_pressure(pressure: IngressPressure) -> IngressAdmission {
    BluetoothAutoStatus::new(&BLE_SHARED).note_ingress_pressure();
    IngressAdmission::Pressured(pressure)
}

fn admit_inbound_frame(
    data_in_tx: &Sender<'static, Mtx, SharedFrameLease, DATA_TOKEN_DEPTH>,
    frame: &[u8],
) -> IngressAdmission {
    let lease = match INBOUND_FRAMES.try_lease() {
        Ok(Some(lease)) => lease,
        Ok(None) => return record_ingress_pressure(IngressPressure::SharedPoolExhausted),
        Err(_) => return record_ingress_pressure(IngressPressure::PoolUnavailable),
    };
    if let Err(error) = lease.try_fill(frame) {
        let pressure = match error {
            FramePoolError::FrameTooLarge { .. } => IngressPressure::FrameTooLarge,
            FramePoolError::SlotBusy
            | FramePoolError::WaitQueueFull
            | FramePoolError::PermitWithoutAvailableSlot => IngressPressure::PoolUnavailable,
        };
        return record_ingress_pressure(pressure);
    }
    if data_in_tx.try_send(lease).is_err() {
        return record_ingress_pressure(IngressPressure::TokenQueueFull);
    }
    BluetoothAutoStatus::new(&BLE_SHARED).note_successful_admission();
    IngressAdmission::Admitted
}

async fn admit_inbound_frame_with_backpressure(
    data_in_tx: &Sender<'static, Mtx, SharedFrameLease, DATA_TOKEN_DEPTH>,
    frame: &[u8],
) -> Result<(), Closed> {
    let lease = match INBOUND_FRAMES.try_lease() {
        Ok(Some(lease)) => lease,
        Ok(None) => {
            BluetoothAutoStatus::new(&BLE_SHARED).note_ingress_pressure();
            INBOUND_FRAMES.lease().await.map_err(|_| Closed)?
        }
        Err(_) => {
            BluetoothAutoStatus::new(&BLE_SHARED).note_ingress_pressure();
            return Err(Closed);
        }
    };
    lease.fill(frame).await.map_err(|_| Closed)?;
    if let Err(TrySendError::Full(lease)) = data_in_tx.try_send(lease) {
        BluetoothAutoStatus::new(&BLE_SHARED).note_ingress_pressure();
        data_in_tx.send(lease).await;
    }
    BluetoothAutoStatus::new(&BLE_SHARED).note_successful_admission();
    Ok(())
}

fn process_unacknowledged_write(
    write: &ServerWrite,
    slot: &'static LinkChannels,
    control_in_tx: &Sender<'static, Mtx, Control, CTRL_DEPTH>,
    data_in_tx: &Sender<'static, Mtx, SharedFrameLease, DATA_TOKEN_DEPTH>,
    reassembler: &mut Reassembler<GATT_REASSEMBLY_CAP>,
) -> IngressAdmission {
    match write.target() {
        WriteTarget::Control => {
            let Some(control) = Control::decode(write.value()) else {
                return IngressAdmission::Admitted;
            };
            if slot.peer_protocol().is_none() {
                slot.set_peer_protocol(PeerProtocol::Native);
            }
            if control_in_tx.try_send(control).is_err() {
                return record_ingress_pressure(IngressPressure::ControlQueueFull);
            }
        }
        WriteTarget::Data => {
            let Some(fragment) = Fragment::decode(write.value()) else {
                return IngressAdmission::Admitted;
            };
            if let Some(frame) = reassembler.absorb(&fragment) {
                return admit_inbound_frame(data_in_tx, frame);
            }
        }
        WriteTarget::ColumbaRx => {
            if slot.peer_protocol().is_none() && write.value().len() == 16 {
                let mut bytes = [0u8; 16];
                bytes.copy_from_slice(write.value());
                slot.identity_in.signal(BleIdentity::new(bytes));
                slot.set_peer_protocol(PeerProtocol::Columba);
            } else if slot.peer_protocol() == Some(PeerProtocol::Columba) {
                let Some(fragment) = Fragment::decode(write.value()) else {
                    return IngressAdmission::Admitted;
                };
                if let Some(frame) = reassembler.absorb(&fragment) {
                    return admit_inbound_frame(data_in_tx, frame);
                }
            }
        }
    }
    BluetoothAutoStatus::new(&BLE_SHARED).note_successful_admission();
    IngressAdmission::Admitted
}

async fn process_acknowledged_writes(slot: &'static LinkChannels) {
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();
    let mut reassembler: Reassembler<GATT_REASSEMBLY_CAP> = Reassembler::new();
    loop {
        let write = slot.acknowledged_writes.receive().await;
        let admitted = match write.target() {
            WriteTarget::Control => {
                if let Some(control) = Control::decode(write.value()) {
                    if slot.peer_protocol().is_none() {
                        slot.set_peer_protocol(PeerProtocol::Native);
                    }
                    control_in_tx.send(control).await;
                }
                true
            }
            WriteTarget::Data => {
                if let Some(fragment) = Fragment::decode(write.value()) {
                    match reassembler.absorb(&fragment) {
                        Some(frame) => admit_inbound_frame_with_backpressure(&data_in_tx, frame)
                            .await
                            .is_ok(),
                        None => true,
                    }
                } else {
                    true
                }
            }
            WriteTarget::ColumbaRx => {
                if slot.peer_protocol().is_none() && write.value().len() == 16 {
                    let mut bytes = [0u8; 16];
                    bytes.copy_from_slice(write.value());
                    slot.identity_in.signal(BleIdentity::new(bytes));
                    slot.set_peer_protocol(PeerProtocol::Columba);
                    true
                } else if slot.peer_protocol() == Some(PeerProtocol::Columba) {
                    if let Some(fragment) = Fragment::decode(write.value()) {
                        match reassembler.absorb(&fragment) {
                            Some(frame) => {
                                admit_inbound_frame_with_backpressure(&data_in_tx, frame)
                                    .await
                                    .is_ok()
                            }
                            None => true,
                        }
                    } else {
                        true
                    }
                } else {
                    true
                }
            }
        };
        if admitted {
            BluetoothAutoStatus::new(&BLE_SHARED).note_successful_admission();
            write.accept();
        } else {
            write.reject(GattError::ATTERR_INSUF_RESOURCES);
        }
    }
}

async fn l2cap_pump(
    channel: &l2cap::Channel<L2capPacket>,
    data_out_rx: Receiver<'static, Mtx, SharedFrameLease, DATA_TOKEN_DEPTH>,
    data_in_tx: Sender<'static, Mtx, SharedFrameLease, DATA_TOKEN_DEPTH>,
) {
    let outbound = async {
        loop {
            let frame = data_out_rx.receive().await;
            let frame = frame.lock().await;
            if let Some(packet) = L2capPacket::from_frame(&frame) {
                if channel.tx(packet).await.is_err() {
                    break;
                }
            }
        }
    };
    let inbound = async {
        loop {
            let packet = match channel.rx().await {
                Ok(packet) => packet,
                Err(_) => break,
            };
            let bytes = packet.bytes();
            if bytes.len() < STREAM_FRAME_PREFIX_LEN {
                continue;
            }
            let len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
            let frame = &bytes[STREAM_FRAME_PREFIX_LEN..];
            if frame.len() < len {
                continue;
            }
            if admit_inbound_frame_with_backpressure(&data_in_tx, &frame[..len])
                .await
                .is_err()
            {
                break;
            }
        }
    };
    let _ = select(outbound, inbound).await;
}

async fn serve_peripheral(
    l2cap: &'static l2cap::L2cap<L2capPacket>,
    server: &Server,
    conn: &Connection,
    slot: &'static LinkChannels,
    hub: &'static BleHub,
    link: BleSlotLink,
    worker: &BleSlotWorker,
) {
    let control_out_rx = slot.control_out.receiver();
    let data_out_rx = slot.data_out.receiver();
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();
    let mut unacknowledged_reassembler: Reassembler<GATT_REASSEMBLY_CAP> = Reassembler::new();

    let inbound = gatt_server::run(conn, server, |write| match write.delivery() {
        WriteDelivery::Acknowledged => {
            if let Err(TrySendError::Full(write)) = slot.acknowledged_writes.try_send(write) {
                BluetoothAutoStatus::new(&BLE_SHARED).note_ingress_pressure();
                write.reject(GattError::ATTERR_INSUF_RESOURCES);
            }
        }
        WriteDelivery::Unacknowledged => {
            let _ = process_unacknowledged_write(
                &write,
                slot,
                &control_in_tx,
                &data_in_tx,
                &mut unacknowledged_reassembler,
            );
            write.accept();
        }
    });
    let acknowledged = process_acknowledged_writes(slot);

    let ready = async {
        let _ = slot.profile_ready.wait().await;
        hub.ready.send(link.into_ready(Origin::Accepted)).await;
        core::future::pending::<()>().await;
    };

    let control_outbound = async {
        loop {
            let ctrl = control_out_rx.receive().await;
            let mut buf = [0u8; CONTROL_MAX_LEN];
            if let Some(n) = ctrl.encode(&mut buf) {
                if notify_with_backpressure(server, conn, ServerNotification::Control, &buf[..n])
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    };

    let data = async {
        let plan = slot.data_plane.wait().await;
        let protocol = slot.peer_protocol().unwrap_or(PeerProtocol::Native);
        let channel = match (protocol, plan) {
            (PeerProtocol::Native, L2capPlan::Accept) => with_timeout(
                L2CAP_HANDSHAKE_WINDOW,
                l2cap.listen_with(conn, &l2cap_config(), |psm| psm == L2CAP_PSM),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .map(|(_psm, channel)| channel),
            _ => None,
        };
        match channel {
            Some(channel) => l2cap_pump(&channel, data_out_rx, data_in_tx).await,
            None => loop {
                let frame = data_out_rx.receive().await;
                let frame = frame.lock().await;
                for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                    let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                    if let Some(n) = fragment.encode(&mut buf) {
                        let target = match protocol {
                            PeerProtocol::Native => ServerNotification::NativeData,
                            PeerProtocol::Columba => ServerNotification::ColumbaData,
                        };
                        if notify_with_backpressure(server, conn, target, &buf[..n])
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            },
        }
    };

    let _ = select4(
        select(select(inbound, acknowledged), ready),
        control_outbound,
        data,
        worker.wait_for_close(),
    )
    .await;
}

async fn serve_central(
    sd: &'static Softdevice,
    l2cap: &'static l2cap::L2cap<L2capPacket>,
    hub: &'static BleHub,
    addr: Address,
    slot: &'static LinkChannels,
    link: BleSlotLink,
    worker: &BleSlotWorker,
) {
    let central_radio = hub.acquire_central_radio().await;
    let whitelist = [&addr];
    let mut config = central::ConnectConfig::default();
    config.scan_config.whitelist = Some(&whitelist);
    config.scan_config.extended = false;
    config.scan_config.timeout = CONNECT_WINDOW_TICKS;
    config.scan_config.interval = CONNECT_SCAN_INTERVAL;
    config.scan_config.window = CONNECT_SCAN_WINDOW;
    config.conn_params = preferred_conn_params();
    let conn = match select(central::connect(sd, &config), worker.wait_for_close()).await {
        Either::First(Ok(mut conn)) => {
            initiate_data_length_extension(&mut conn);
            conn
        }
        Either::First(Err(_)) => {
            report_dial_failed(hub, worker, addr.bytes()).await;
            return;
        }
        Either::Second(()) => return,
    };
    slot.set_address(addr.bytes());
    let native = match select(
        gatt_client::discover::<NativeReticulumClient>(&conn),
        worker.wait_for_close(),
    )
    .await
    {
        Either::First(result) => result,
        Either::Second(()) => return,
    };
    if let Ok(client) = native {
        drop(central_radio);
        serve_native_central(l2cap, hub, slot, link, worker, conn, client).await;
        return;
    }
    let client = match select(
        gatt_client::discover::<ColumbaReticulumClient>(&conn),
        worker.wait_for_close(),
    )
    .await
    {
        Either::First(Ok(client)) => client,
        Either::First(Err(_)) => {
            report_dial_failed(hub, worker, addr.bytes()).await;
            return;
        }
        Either::Second(()) => return,
    };
    drop(central_radio);
    serve_columba_central(hub, addr, slot, link, worker, conn, client).await;
}

async fn report_dial_failed(hub: &BleHub, worker: &BleSlotWorker, address: [u8; 6]) {
    if !hub.radio_enabled.load(Ordering::Relaxed) {
        return;
    }
    match select(worker.wait_for_close(), hub.dial_failed.send(address)).await {
        Either::First(()) | Either::Second(()) => {}
    }
}

async fn serve_native_central(
    l2cap: &'static l2cap::L2cap<L2capPacket>,
    hub: &'static BleHub,
    slot: &'static LinkChannels,
    link: BleSlotLink,
    worker: &BleSlotWorker,
    conn: Connection,
    client: NativeReticulumClient,
) {
    if client.control_cccd_write(true).await.is_err() || client.data_cccd_write(true).await.is_err()
    {
        report_dial_failed(hub, worker, slot.address()).await;
        return;
    }
    slot.set_peer_protocol(PeerProtocol::Native);
    hub.ready.send(link.into_ready(Origin::Dialed)).await;

    let control_out_rx = slot.control_out.receiver();
    let data_out_rx = slot.data_out.receiver();
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();
    let mut reassembler: Reassembler<GATT_REASSEMBLY_CAP> = Reassembler::new();

    let inbound = gatt_client::run(&conn, &client, |event| match event {
        NativeReticulumClientEvent::ControlNotification(value) => {
            if let Some(ctrl) = Control::decode(&value) {
                if control_in_tx.try_send(ctrl).is_err() {
                    let _ = record_ingress_pressure(IngressPressure::ControlQueueFull);
                } else {
                    BluetoothAutoStatus::new(&BLE_SHARED).note_successful_admission();
                }
            }
        }
        NativeReticulumClientEvent::DataNotification(value) => {
            if let Some(fragment) = Fragment::decode(&value) {
                if let Some(frame) = reassembler.absorb(&fragment) {
                    let _ = admit_inbound_frame(&data_in_tx, frame);
                }
            }
        }
    });

    let control_outbound = async {
        loop {
            let ctrl = control_out_rx.receive().await;
            let mut buf = [0u8; CONTROL_MAX_LEN];
            if let Some(n) = ctrl.encode(&mut buf) {
                if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                    if client.control_write(&value).await.is_err() {
                        return;
                    }
                }
            }
        }
    };

    let data = async {
        let channel = match slot.data_plane.wait().await {
            L2capPlan::Open { psm } => with_timeout(L2CAP_HANDSHAKE_WINDOW, async {
                loop {
                    if let Ok(channel) = l2cap.setup(&conn, &l2cap_config(), psm.get()).await {
                        break channel;
                    }
                    Timer::after(L2CAP_SETUP_RETRY).await;
                }
            })
            .await
            .ok(),
            _ => None,
        };
        match channel {
            Some(channel) => l2cap_pump(&channel, data_out_rx, data_in_tx).await,
            None => loop {
                let frame = data_out_rx.receive().await;
                let frame = frame.lock().await;
                for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                    let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                    if let Some(n) = fragment.encode(&mut buf) {
                        if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                            if client.data_write(&value).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            },
        }
    };

    let _ = select4(inbound, control_outbound, data, worker.wait_for_close()).await;
}

async fn serve_columba_central(
    hub: &'static BleHub,
    addr: Address,
    slot: &'static LinkChannels,
    link: BleSlotLink,
    worker: &BleSlotWorker,
    conn: Connection,
    client: ColumbaReticulumClient,
) {
    let peer_identity = match client.identity_read().await {
        Ok(value) if value.len() == 16 => {
            let mut bytes = [0u8; 16];
            bytes.copy_from_slice(&value);
            BleIdentity::new(bytes)
        }
        _ => {
            report_dial_failed(hub, worker, addr.bytes()).await;
            return;
        }
    };
    if client.tx_cccd_write(true).await.is_err() {
        report_dial_failed(hub, worker, addr.bytes()).await;
        return;
    }
    slot.set_peer_protocol(PeerProtocol::Columba);
    slot.identity_in.signal(peer_identity);
    hub.ready.send(link.into_ready(Origin::Dialed)).await;

    let identity = match select(slot.identity_out.receive(), worker.wait_for_close()).await {
        Either::First(identity) => identity,
        Either::Second(()) => return,
    };
    let Ok(identity) = GattValue::from_slice(identity.as_bytes()) else {
        return;
    };
    if client.rx_write(&identity).await.is_err() {
        return;
    }

    let data_out_rx = slot.data_out.receiver();
    let data_in_tx = slot.data_in.sender();
    let mut reassembler: Reassembler<GATT_REASSEMBLY_CAP> = Reassembler::new();
    let inbound = gatt_client::run(&conn, &client, |event| match event {
        ColumbaReticulumClientEvent::TxNotification(value) => {
            if let Some(fragment) = Fragment::decode(&value) {
                if let Some(frame) = reassembler.absorb(&fragment) {
                    let _ = admit_inbound_frame(&data_in_tx, frame);
                }
            }
        }
    });
    let data = async {
        let _ = slot.data_plane.wait().await;
        loop {
            let frame = data_out_rx.receive().await;
            let frame = frame.lock().await;
            for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                if let Some(n) = fragment.encode(&mut buf) {
                    if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                        if client.rx_write_without_response(&value).await.is_err() {
                            return;
                        }
                    }
                }
            }
        }
    };
    let _ = select4(
        inbound,
        data,
        worker.wait_for_close(),
        core::future::pending::<()>(),
    )
    .await;
}

#[embassy_executor::task(pool_size = 7)]
pub(super) async fn serve_slot(
    idx: usize,
    sd: &'static Softdevice,
    l2cap: &'static l2cap::L2cap<L2capPacket>,
    server: &'static Server,
    hub: &'static BleHub,
) {
    let slot = &hub.slots[idx];
    loop {
        let job = hub.assign[idx].receive().await;
        if !hub.radio_enabled.load(Ordering::Relaxed) {
            drop(job);
            continue;
        }
        slot.reset();
        match job {
            SlotJob::Accept {
                connection: conn,
                slot: lease,
            } => {
                let ConnectionSlotOwners { worker, link } = lease.activate();
                slot.set_address(conn.peer_address().bytes());
                serve_peripheral(l2cap, server, &conn, slot, hub, link, &worker).await;
            }
            SlotJob::Dial {
                address,
                slot: lease,
            } => {
                let ConnectionSlotOwners { worker, link } = lease.activate();
                serve_central(sd, l2cap, hub, address, slot, link, &worker).await;
            }
        }
    }
}

pub(super) async fn acceptor(sd: &'static Softdevice, hub: &'static BleHub) -> ! {
    let mut enabled = false;
    loop {
        if !enabled {
            enabled = hub.advertise.wait().await;
            continue;
        }
        let slot = match hub.connection_slots.acquire().await {
            Ok(slot) => slot,
            Err(_) => {
                Timer::after(Duration::from_millis(500)).await;
                continue;
            }
        };
        let index = slot.index();

        let mut adv_buf = [0u8; 31];
        let adv_len =
            encode_advertisement(&mut adv_buf, BleRoleCapabilities::DualRole).unwrap_or(0);
        let scan_data = [0x05u8, 0x09, b'P', b'r', b'n', b's'];
        let adv = peripheral::ConnectableAdvertisement::ScannableUndirected {
            adv_data: &adv_buf[..adv_len],
            scan_data: &scan_data,
        };
        let adv_config = peripheral::Config::default();
        let advertise = peripheral::advertise_connectable(sd, adv, &adv_config);
        match select(advertise, hub.advertise.wait()).await {
            Either::First(Ok(conn)) => {
                hub.assign[index]
                    .send(SlotJob::Accept {
                        connection: conn,
                        slot,
                    })
                    .await;
            }
            Either::First(Err(_)) => {
                Timer::after(Duration::from_millis(500)).await;
            }
            Either::Second(new_state) => {
                enabled = new_state;
            }
        }
    }
}

pub(super) async fn scanner(sd: &'static Softdevice, hub: &'static BleHub) -> ! {
    let sightings = hub.sightings.sender();
    let local_address = BleAddress::new(nrf_softdevice::ble::get_address(sd).bytes());
    let mut enabled = false;
    loop {
        if !enabled {
            enabled = hub.scan_enabled.wait().await;
            continue;
        }
        let central_radio = hub.acquire_central_radio().await;
        let config = central::ScanConfig {
            active: false,
            extended: false,
            interval: IDLE_SCAN_INTERVAL,
            window: IDLE_SCAN_WINDOW,
            timeout: SCAN_WINDOW_TICKS,
            ..Default::default()
        };
        let scan = central::scan(sd, &config, |report| {
            if report.data.len == 0 {
                return None;
            }
            // SAFETY: The SoftDevice scan callback owns `report` for this invocation and guarantees
            // `p_data` addresses `len` initialized bytes; the slice does not escape the callback.
            let data = unsafe {
                core::slice::from_raw_parts(report.data.p_data, report.data.len as usize)
            };
            let address = Address::from_raw(report.peer_addr);
            let capabilities =
                columba_role_capabilities(data).unwrap_or(BleRoleCapabilities::DualRole);
            let should_dial = columba_connection_role(
                local_address,
                BleRoleCapabilities::DualRole,
                BleAddress::new(address.bytes()),
                capabilities,
            ) == ColumbaConnectionRole::Dial;
            if contains_service(data) && should_dial {
                Some(SeenPeer {
                    address,
                    rssi: report.rssi,
                })
            } else {
                None
            }
        });
        let outcome = select(scan, hub.scan_enabled.wait()).await;
        drop(central_radio);
        match outcome {
            Either::First(Ok(peer)) => {
                let _ = sightings.try_send(peer);
                Timer::after(SIGHTING_PACING).await;
            }
            Either::First(Err(central::ScanError::Timeout)) => {}
            Either::First(Err(_)) => {
                Timer::after(SCAN_ERROR_BACKOFF).await;
            }
            Either::Second(new_state) => {
                enabled = new_state;
            }
        }
    }
}
