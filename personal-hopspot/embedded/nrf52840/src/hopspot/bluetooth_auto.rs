use core::cell::{Cell, UnsafeCell};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_futures::select::{select, select4, Either, Either4};
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Timer};

use nrf_softdevice::ble::{
    central, gatt_client, gatt_server, l2cap, peripheral, Address, Connection,
};
use nrf_softdevice::{raw, SocEvent, Softdevice};

use personal_rns::bluetooth_auto::BluetoothAutoShared;
use personal_rns::interfaces::bluetooth_auto::{
    columba_connection_role, columba_role_capabilities, contains_service, encode_advertisement,
    encode_stream_frame, fragments_of, BleAddress, BleIdentity, BleRoleCapabilities,
    ColumbaConnectionRole, Control, Fragment, L2capPlan, PeerProtocol, Reassembler, BLE_HW_MTU,
    CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN, STREAM_FRAME_PREFIX_LEN,
};
use personal_rns::interfaces::bluetooth_auto::{
    AdvertisingMode, BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin, ScanningMode,
};
use personal_rns::interfaces::{InterfaceId, InterfaceKind};

type Mtx = CriticalSectionRawMutex;
type FrameBytes = heapless09::Vec<u8, BLE_HW_MTU>;
type GattValue = heapless09::Vec<u8, 244>;

pub(super) const MEMBERS: usize = NrfBleBackend::MAX_PEERS;
pub(super) const BLE_SUPERVISOR_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);

pub(super) const POOL: usize = MEMBERS + 2;
const _: () = assert!(POOL == 7, "serve_slot pool_size must equal POOL");

const CTRL_DEPTH: usize = 4;
const DATA_DEPTH: usize = 1;
const SIGHTING_DEPTH: usize = 4;
const SEEN_CAP: usize = 8;
const GATT_FRAGMENT_PAYLOAD: usize = 180;
const GATT_REASSEMBLY_CAP: usize = 600;
const NOTIFY_PACING: Duration = Duration::from_millis(15);
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

mod reticulum_service {
    #![allow(clippy::enum_variant_names)]

    use super::GattValue;

    #[nrf_softdevice::gatt_service(uuid = "37145b00-442d-4a94-917f-8f42c5da28e3")]
    pub(in crate::hopspot) struct ReticulumService {
        #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e4", read, notify)]
        pub(super) columba_tx: GattValue,
        #[characteristic(
            uuid = "37145b00-442d-4a94-917f-8f42c5da28e5",
            write,
            write_without_response
        )]
        pub(super) columba_rx: GattValue,
        #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e6", read)]
        pub(super) columba_identity: GattValue,
        #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e7", write, notify)]
        pub(super) control: GattValue,
        #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e8", write, notify)]
        pub(super) data: GattValue,
    }
}

use reticulum_service::{ReticulumService, ReticulumServiceEvent};

#[nrf_softdevice::gatt_server]
pub(super) struct Server {
    rns: ReticulumService,
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

pub(super) fn set_columba_identity(server: &Server, identity: BleIdentity) {
    if let Ok(value) = GattValue::from_slice(identity.as_bytes()) {
        let _ = server.rns.columba_identity_set(&value);
    }
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
    data_in: Channel<Mtx, FrameBytes, DATA_DEPTH>,
    data_out: Channel<Mtx, FrameBytes, DATA_DEPTH>,
    identity_in: Channel<Mtx, BleIdentity, 1>,
    identity_out: Channel<Mtx, BleIdentity, 1>,
    link_dead: Signal<Mtx, ()>,
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
            identity_in: Channel::new(),
            identity_out: Channel::new(),
            link_dead: Signal::new(),
            data_plane: Signal::new(),
            profile_ready: Signal::new(),
            address: BlockingMutex::new(Cell::new([0u8; 6])),
            peer_protocol: BlockingMutex::new(Cell::new(None)),
        }
    }

    fn set_address(&self, bytes: [u8; 6]) {
        self.address.lock(|address| address.set(bytes));
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
        self.link_dead.reset();
        self.data_plane.reset();
        self.profile_ready.reset();
        self.peer_protocol.lock(|current| current.set(None));
        self.control_in.clear();
        self.control_out.clear();
        self.data_in.clear();
        self.data_out.clear();
        self.identity_in.clear();
        self.identity_out.clear();
    }

    fn link(&'static self) -> NrfBleLink {
        NrfBleLink {
            peer_protocol: self.peer_protocol().unwrap_or(PeerProtocol::Native),
            control_in: self.control_in.receiver(),
            control_out: self.control_out.sender(),
            data_in: self.data_in.receiver(),
            data_out: self.data_out.sender(),
            identity_in: self.identity_in.receiver(),
            identity_out: self.identity_out.sender(),
            data_plane: &self.data_plane,
            plan: L2capPlan::None,
            fuse: LinkFuse::new(&self.link_dead),
            address: self.address.lock(|address| address.get()),
        }
    }
}

enum SlotJob {
    Accept(Connection),
    Dial(Address),
}

pub(super) struct BleHub {
    slots: [LinkChannels; POOL],
    assign: [Channel<Mtx, SlotJob, 1>; POOL],
    pub(super) free: Channel<Mtx, usize, POOL>,
    connected: Channel<Mtx, usize, POOL>,
    dialed: Channel<Mtx, usize, POOL>,
    dial_failed: Channel<Mtx, [u8; 6], POOL>,
    /// The central-radio permit: a single token both the scanner and each dial must hold while using
    /// the SoftDevice's one scanner. `central::scan` and `central::connect` (which scans to find the
    /// whitelisted peer) cannot run at once — overlapping them fails the connect and can panic the
    /// shared connect portal — so this serializes them: one scan-or-dial on the radio at a time.
    pub(super) central_token: Channel<Mtx, (), 1>,
    advertise: Signal<Mtx, bool>,
    sightings: Channel<Mtx, SeenPeer, SIGHTING_DEPTH>,
    scan_enabled: Signal<Mtx, bool>,
}

impl BleHub {
    const fn new() -> Self {
        Self {
            slots: [const { LinkChannels::new() }; POOL],
            assign: [const { Channel::new() }; POOL],
            free: Channel::new(),
            connected: Channel::new(),
            dialed: Channel::new(),
            dial_failed: Channel::new(),
            central_token: Channel::new(),
            advertise: Signal::new(),
            sightings: Channel::new(),
            scan_enabled: Signal::new(),
        }
    }
}

pub(super) static HUB: BleHub = BleHub::new();

pub(super) struct NrfBleBackend {
    connected: Receiver<'static, Mtx, usize, POOL>,
    dialed: Receiver<'static, Mtx, usize, POOL>,
    dial_failed: Receiver<'static, Mtx, [u8; 6], POOL>,
    sightings: Receiver<'static, Mtx, SeenPeer, SIGHTING_DEPTH>,
    seen: heapless::Vec<Address, SEEN_CAP>,
    hub: &'static BleHub,
}

impl NrfBleBackend {
    pub(super) const MAX_PEERS: usize = 5;

    pub(super) fn new(hub: &'static BleHub) -> Self {
        Self {
            connected: hub.connected.receiver(),
            dialed: hub.dialed.receiver(),
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

    async fn next_event(&mut self) -> BleEvent<NrfBleLink> {
        match select4(
            self.connected.receive(),
            self.dialed.receive(),
            self.sightings.receive(),
            self.dial_failed.receive(),
        )
        .await
        {
            Either4::First(slot) => BleEvent::Inbound(self.hub.slots[slot].link()),
            Either4::Second(slot) => BleEvent::LinkReady {
                link: self.hub.slots[slot].link(),
                origin: Origin::Dialed,
                peer_rssi: None,
            },
            Either4::Third(peer) => {
                self.remember(peer.address);
                BleEvent::Sighting {
                    address: BleAddress::new(peer.address.bytes()),
                    rssi: Some(peer.rssi),
                }
            }
            Either4::Fourth(bytes) => BleEvent::DialFailed {
                address: BleAddress::new(bytes),
            },
        }
    }

    async fn dial(&mut self, address: BleAddress) {
        let Some(addr) = self.resolve(address) else {
            return;
        };
        let Ok(idx) = self.hub.free.try_receive() else {
            return;
        };
        if self.hub.assign[idx].try_send(SlotJob::Dial(addr)).is_err() {
            let _ = self.hub.free.try_send(idx);
        }
    }
}

struct LinkFuse {
    dead: &'static Signal<Mtx, ()>,
    armed: bool,
}

impl LinkFuse {
    fn new(dead: &'static Signal<Mtx, ()>) -> Self {
        Self { dead, armed: true }
    }

    fn signal(&self) -> &'static Signal<Mtx, ()> {
        self.dead
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for LinkFuse {
    fn drop(&mut self) {
        if self.armed {
            self.dead.signal(());
        }
    }
}

pub(super) struct NrfBleLink {
    peer_protocol: PeerProtocol,
    control_in: Receiver<'static, Mtx, Control, CTRL_DEPTH>,
    control_out: Sender<'static, Mtx, Control, CTRL_DEPTH>,
    data_in: Receiver<'static, Mtx, FrameBytes, DATA_DEPTH>,
    data_out: Sender<'static, Mtx, FrameBytes, DATA_DEPTH>,
    identity_in: Receiver<'static, Mtx, BleIdentity, 1>,
    identity_out: Sender<'static, Mtx, BleIdentity, 1>,
    data_plane: &'static Signal<Mtx, L2capPlan>,
    plan: L2capPlan,
    fuse: LinkFuse,
    address: [u8; 6],
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
        match select(self.control_out.send(*msg), self.fuse.signal().wait()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn control_recv(&mut self) -> Result<Control, Closed> {
        match select(self.control_in.receive(), self.fuse.signal().wait()).await {
            Either::First(msg) => Ok(msg),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn receive_columba_peer_identity(&mut self) -> Result<BleIdentity, Closed> {
        match select(self.identity_in.receive(), self.fuse.signal().wait()).await {
            Either::First(identity) => Ok(identity),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn send_columba_identity(&mut self, identity: BleIdentity) -> Result<(), Closed> {
        match select(self.identity_out.send(identity), self.fuse.signal().wait()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), Closed> {
        self.plan = *plan;
        Ok(())
    }

    fn into_data(mut self) -> (NrfBleSource, NrfBleSink) {
        let link_dead = self.fuse.signal();
        self.data_plane.signal(self.plan);
        self.fuse.disarm();
        (
            NrfBleSource {
                data_in: self.data_in,
                link_dead,
            },
            NrfBleSink {
                data_out: self.data_out,
                link_dead,
            },
        )
    }
}

pub(super) struct NrfBleSource {
    data_in: Receiver<'static, Mtx, FrameBytes, DATA_DEPTH>,
    link_dead: &'static Signal<Mtx, ()>,
}

impl BleSource for NrfBleSource {
    type Error = Closed;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Closed> {
        match select(self.data_in.receive(), self.link_dead.wait()).await {
            Either::First(frame) => {
                let len = frame.len().min(out.len());
                out[..len].copy_from_slice(&frame[..len]);
                Ok(len)
            }
            Either::Second(()) => Err(Closed),
        }
    }
}

pub(super) struct NrfBleSink {
    data_out: Sender<'static, Mtx, FrameBytes, DATA_DEPTH>,
    link_dead: &'static Signal<Mtx, ()>,
}

impl BleSink for NrfBleSink {
    type Error = Closed;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Closed> {
        let mut bytes = FrameBytes::new();
        bytes.extend_from_slice(frame).map_err(|_| Closed)?;
        match select(self.data_out.send(bytes), self.link_dead.wait()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }
}

impl Drop for NrfBleSink {
    fn drop(&mut self) {
        self.link_dead.signal(());
    }
}

fn l2cap_config() -> l2cap::Config {
    l2cap::Config {
        credits: L2CAP_CREDITS,
    }
}

async fn l2cap_pump(
    channel: &l2cap::Channel<L2capPacket>,
    data_out_rx: Receiver<'static, Mtx, FrameBytes, DATA_DEPTH>,
    data_in_tx: Sender<'static, Mtx, FrameBytes, DATA_DEPTH>,
) {
    let outbound = async {
        loop {
            let frame = data_out_rx.receive().await;
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
            let mut bytes = FrameBytes::new();
            if bytes.extend_from_slice(&frame[..len]).is_ok() {
                data_in_tx.send(bytes).await;
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
    idx: usize,
) {
    let control_out_rx = slot.control_out.receiver();
    let data_out_rx = slot.data_out.receiver();
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();
    let mut reassembler: Reassembler<GATT_REASSEMBLY_CAP> = Reassembler::new();

    let inbound = gatt_server::run(conn, server, |event| match event {
        ServerEvent::Rns(rns) => match rns {
            ReticulumServiceEvent::ControlWrite(value) => {
                if let Some(ctrl) = Control::decode(&value) {
                    if slot.peer_protocol().is_none() {
                        slot.set_peer_protocol(PeerProtocol::Native);
                    }
                    let _ = control_in_tx.try_send(ctrl);
                }
            }
            ReticulumServiceEvent::ControlCccdWrite { .. } => {}
            ReticulumServiceEvent::DataWrite(value) => {
                if let Some(fragment) = Fragment::decode(&value) {
                    if let Some(frame) = reassembler.absorb(&fragment) {
                        let mut bytes = FrameBytes::new();
                        if bytes.extend_from_slice(frame).is_ok() {
                            let _ = data_in_tx.try_send(bytes);
                        }
                    }
                }
            }
            ReticulumServiceEvent::DataCccdWrite { .. } => {}
            ReticulumServiceEvent::ColumbaRxWrite(value) => {
                if slot.peer_protocol().is_none() && value.len() == 16 {
                    let mut bytes = [0u8; 16];
                    bytes.copy_from_slice(&value);
                    let _ = slot.identity_in.try_send(BleIdentity::new(bytes));
                    slot.set_peer_protocol(PeerProtocol::Columba);
                } else if slot.peer_protocol() == Some(PeerProtocol::Columba) {
                    if let Some(fragment) = Fragment::decode(&value) {
                        if let Some(frame) = reassembler.absorb(&fragment) {
                            let mut bytes = FrameBytes::new();
                            if bytes.extend_from_slice(frame).is_ok() {
                                let _ = data_in_tx.try_send(bytes);
                            }
                        }
                    }
                }
            }
            ReticulumServiceEvent::ColumbaTxCccdWrite { .. } => {}
        },
    });

    let ready = async {
        let _ = slot.profile_ready.wait().await;
        hub.connected.send(idx).await;
        core::future::pending::<()>().await;
    };

    let control_outbound = async {
        loop {
            let ctrl = control_out_rx.receive().await;
            let mut buf = [0u8; CONTROL_MAX_LEN];
            if let Some(n) = ctrl.encode(&mut buf) {
                if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                    let _ = server.rns.control_notify(conn, &value);
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
                for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                    let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                    if let Some(n) = fragment.encode(&mut buf) {
                        if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                            match protocol {
                                PeerProtocol::Native => {
                                    let _ = server.rns.data_notify(conn, &value);
                                }
                                PeerProtocol::Columba => {
                                    let _ = server.rns.columba_tx_notify(conn, &value);
                                }
                            }
                        }
                    }
                    Timer::after(NOTIFY_PACING).await;
                }
            },
        }
    };

    let _ = select4(
        select(inbound, ready),
        control_outbound,
        data,
        slot.link_dead.wait(),
    )
    .await;
}

async fn serve_central(
    sd: &'static Softdevice,
    l2cap: &'static l2cap::L2cap<L2capPacket>,
    hub: &'static BleHub,
    idx: usize,
    addr: Address,
    slot: &'static LinkChannels,
) {
    hub.central_token.receive().await;
    let whitelist = [&addr];
    let mut config = central::ConnectConfig::default();
    config.scan_config.whitelist = Some(&whitelist);
    config.scan_config.extended = false;
    config.scan_config.timeout = CONNECT_WINDOW_TICKS;
    config.scan_config.interval = CONNECT_SCAN_INTERVAL;
    config.scan_config.window = CONNECT_SCAN_WINDOW;
    let conn = match central::connect(sd, &config).await {
        Ok(conn) => conn,
        Err(_) => {
            let _ = hub.central_token.try_send(());
            let _ = hub.dial_failed.try_send(addr.bytes());
            return;
        }
    };
    if let Ok(client) = gatt_client::discover::<NativeReticulumClient>(&conn).await {
        let _ = hub.central_token.try_send(());
        serve_native_central(l2cap, hub, idx, addr, slot, conn, client).await;
        return;
    }
    let client = match gatt_client::discover::<ColumbaReticulumClient>(&conn).await {
        Ok(client) => client,
        Err(_) => {
            let _ = hub.central_token.try_send(());
            let _ = hub.dial_failed.try_send(addr.bytes());
            return;
        }
    };
    let _ = hub.central_token.try_send(());
    serve_columba_central(hub, idx, addr, slot, conn, client).await;
}

async fn serve_native_central(
    l2cap: &'static l2cap::L2cap<L2capPacket>,
    hub: &'static BleHub,
    idx: usize,
    addr: Address,
    slot: &'static LinkChannels,
    conn: Connection,
    client: NativeReticulumClient,
) {
    let _ = client.control_cccd_write(true).await;
    let _ = client.data_cccd_write(true).await;
    slot.set_address(addr.bytes());
    slot.set_peer_protocol(PeerProtocol::Native);
    hub.dialed.send(idx).await;

    let control_out_rx = slot.control_out.receiver();
    let data_out_rx = slot.data_out.receiver();
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();
    let mut reassembler: Reassembler<GATT_REASSEMBLY_CAP> = Reassembler::new();

    let inbound = gatt_client::run(&conn, &client, |event| match event {
        NativeReticulumClientEvent::ControlNotification(value) => {
            if let Some(ctrl) = Control::decode(&value) {
                let _ = control_in_tx.try_send(ctrl);
            }
        }
        NativeReticulumClientEvent::DataNotification(value) => {
            if let Some(fragment) = Fragment::decode(&value) {
                if let Some(frame) = reassembler.absorb(&fragment) {
                    let mut bytes = FrameBytes::new();
                    if bytes.extend_from_slice(frame).is_ok() {
                        let _ = data_in_tx.try_send(bytes);
                    }
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
                    let _ = client.control_write(&value).await;
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
                for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                    let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                    if let Some(n) = fragment.encode(&mut buf) {
                        if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                            let _ = client.data_write(&value).await;
                        }
                    }
                    Timer::after(NOTIFY_PACING).await;
                }
            },
        }
    };

    let _ = select4(inbound, control_outbound, data, slot.link_dead.wait()).await;
}

async fn serve_columba_central(
    hub: &'static BleHub,
    idx: usize,
    addr: Address,
    slot: &'static LinkChannels,
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
            let _ = hub.dial_failed.try_send(addr.bytes());
            return;
        }
    };
    if client.tx_cccd_write(true).await.is_err() {
        let _ = hub.dial_failed.try_send(addr.bytes());
        return;
    }
    slot.set_address(addr.bytes());
    slot.set_peer_protocol(PeerProtocol::Columba);
    let _ = slot.identity_in.try_send(peer_identity);
    hub.dialed.send(idx).await;

    let identity = match select(slot.identity_out.receive(), slot.link_dead.wait()).await {
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
                    let mut bytes = FrameBytes::new();
                    if bytes.extend_from_slice(frame).is_ok() {
                        let _ = data_in_tx.try_send(bytes);
                    }
                }
            }
        }
    });
    let data = async {
        let _ = slot.data_plane.wait().await;
        loop {
            let frame = data_out_rx.receive().await;
            for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                if let Some(n) = fragment.encode(&mut buf) {
                    if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                        let _ = client.rx_write_without_response(&value).await;
                    }
                }
                Timer::after(NOTIFY_PACING).await;
            }
        }
    };
    let _ = select4(
        inbound,
        data,
        slot.link_dead.wait(),
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
        slot.reset();
        match job {
            SlotJob::Accept(conn) => {
                slot.set_address(conn.peer_address().bytes());
                serve_peripheral(l2cap, server, &conn, slot, hub, idx).await;
            }
            SlotJob::Dial(addr) => serve_central(sd, l2cap, hub, idx, addr, slot).await,
        }
        slot.link_dead.signal(());
        let _ = hub.free.try_send(idx);
    }
}

pub(super) async fn acceptor(sd: &'static Softdevice, hub: &'static BleHub) -> ! {
    let mut enabled = false;
    loop {
        if !enabled {
            enabled = hub.advertise.wait().await;
            continue;
        }
        let idx = hub.free.receive().await;

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
                let _ = hub.assign[idx].try_send(SlotJob::Accept(conn));
            }
            Either::First(Err(_)) => {
                let _ = hub.free.try_send(idx);
                Timer::after(Duration::from_millis(500)).await;
            }
            Either::Second(new_state) => {
                enabled = new_state;
                let _ = hub.free.try_send(idx);
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
        hub.central_token.receive().await;
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
        let _ = hub.central_token.try_send(());
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
