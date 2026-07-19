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

use personal_rns::ble::BluetoothAutoShared;
use personal_rns::interfaces::bluetooth_auto::core::{
    contains_service, encode_advertisement, encode_stream_frame, fragments_of, BleAddress, Control,
    Dialect, Fragment, L2capPlan, Reassembler, BLE_HW_MTU, CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN,
    STREAM_FRAME_PREFIX_LEN,
};
use personal_rns::interfaces::bluetooth_auto::limits;
use personal_rns::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin,
};
use personal_rns::interfaces::{InterfaceId, InterfaceKind};

type Mtx = CriticalSectionRawMutex;
type FrameBytes = heapless09::Vec<u8, BLE_HW_MTU>;
type GattValue = heapless09::Vec<u8, 244>;

/// One channel-set per concurrent physical connection: the BLE_MEMBERS settleable peers plus a little
/// headroom for the brief double-connection a keeper duel opens before it evicts the loser. Each is
/// role-agnostic — a peripheral (accepted) or central (dialed) link claims whichever slot is free.
pub(super) const MEMBERS: usize = limits::T_ECHO_MAX_PEERS;
pub(super) const FLEET_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);

pub(super) const POOL: usize = MEMBERS + 2;
/// `serve_slot`'s `pool_size` is a literal the task macro needs at parse time; keep it equal to POOL.
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

    unsafe fn from_raw_parts(ptr: NonNull<u8>, len: usize) -> Self {
        Self { ptr, len }
    }
}

impl Drop for L2capPacket {
    fn drop(&mut self) {
        L2CAP_POOL_STORE.release(self.ptr);
    }
}

// The USB CDC now carries the Reticulum usb-auto wire instead of a diagnostic console, so there is no
// log sink; `diag!` compiles to nothing. The call sites stay as in-place documentation of the BLE
// plane's state transitions, ready to re-light if a second CDC (or RTT) console is ever added.
macro_rules! diag {
    ($($arg:tt)*) => {{}};
}
pub(super) use diag;

/// The reactor's outbound-commit wake for the BLE fleet lane: the egress signals it on every
/// commit so the supervisor's drain is roused. A same-core wake on this single-core executor,
/// but the mechanism is identical to the Heltec's cross-core one.
pub(super) static OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();

/// The BLE supervisor's shared aggregate + per-peer status, keyed by the fleet id so each settled peer
/// becomes a fleet member under it. One member slot per concurrent peer the radio carries.
pub(super) static BLE_SHARED: BluetoothAutoShared<MEMBERS> = BluetoothAutoShared::new(FLEET_ID);

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

#[nrf_softdevice::gatt_service(uuid = "37145b00-442d-4a94-917f-8f42c5da28e3")]
pub(super) struct ReticulumService {
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e7", write, notify)]
    control: GattValue,
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e8", write, notify)]
    data: GattValue,
}

#[nrf_softdevice::gatt_server]
pub(super) struct Server {
    rns: ReticulumService,
}

/// The central-side view of a peer's [`ReticulumService`]: `discover` resolves the handles on a
/// dialed connection, `*_cccd_write` subscribes to notifications (inbound), `*_write` pushes ours
/// out. The GATT twin of the peripheral `Server`, so a dialed link speaks the same wire.
#[nrf_softdevice::gatt_client(uuid = "37145b00-442d-4a94-917f-8f42c5da28e3")]
struct ReticulumClient {
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e7", write, notify)]
    control: GattValue,
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e8", write, notify)]
    data: GattValue,
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
    (unsafe { raw::sd_power_usbregstatus_get(&mut status) }) == raw::NRF_SUCCESS
        && status & 0x1 != 0
}

/// A peer the scanner saw advertising our service: the full [`Address`] (type + bytes, so the
/// dialer whitelists it exactly) and the report RSSI.
#[derive(Clone, Copy)]
struct SeenPeer {
    address: Address,
    rssi: i8,
}

/// The per-link channel set bridging one slot's serve task to the supervisor's [`NrfBleLink`].
/// Role-agnostic: peripheral and central loops pump the same four lanes; `link_dead` tears down.
struct LinkChannels {
    control_in: Channel<Mtx, Control, CTRL_DEPTH>,
    control_out: Channel<Mtx, Control, CTRL_DEPTH>,
    data_in: Channel<Mtx, FrameBytes, DATA_DEPTH>,
    data_out: Channel<Mtx, FrameBytes, DATA_DEPTH>,
    link_dead: Signal<Mtx, ()>,
    data_plane: Signal<Mtx, L2capPlan>,
    /// The connected peer's address, stashed by the slot worker the moment the connection lands (from
    /// `conn.peer_address()` for an accept, the dialed address for a dial) and read by [`link`](Self::link)
    /// so the supervisor's brain keys this peer correctly — it keys settled-peer lookup and dial/suppress
    /// backoff by address, so a stale all-zero address makes every peer collide on one backoff entry and
    /// hides an already-settled peer from sighting suppression (the redundant self-dial).
    address: BlockingMutex<Mtx, Cell<[u8; 6]>>,
}

impl LinkChannels {
    const fn new() -> Self {
        Self {
            control_in: Channel::new(),
            control_out: Channel::new(),
            data_in: Channel::new(),
            data_out: Channel::new(),
            link_dead: Signal::new(),
            data_plane: Signal::new(),
            address: BlockingMutex::new(Cell::new([0u8; 6])),
        }
    }

    fn set_address(&self, bytes: [u8; 6]) {
        self.address.lock(|address| address.set(bytes));
    }

    fn link(&'static self) -> NrfBleLink {
        NrfBleLink {
            control_in: self.control_in.receiver(),
            control_out: self.control_out.sender(),
            data_in: self.data_in.receiver(),
            data_out: self.data_out.sender(),
            data_plane: &self.data_plane,
            plan: L2capPlan::None,
            fuse: LinkFuse::new(&self.link_dead),
            address: self.address.lock(|address| address.get()),
        }
    }
}

/// The work a free slot is handed: accept an inbound connection the acceptor has in hand, or
/// dial a peer (with its full address, whitelisted exactly), over the same `LinkChannels`.
enum SlotJob {
    Accept(Connection),
    Dial(Address),
}

/// The shared hub the whole BLE plane coordinates through: the role-agnostic [`LinkChannels`]
/// pool, the assign/free/connected/dialed plumbing, the radio-wide advertise/scan gates, and the
/// scanner's sighting funnel. One `static` so every task references the same channels.
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

    /// Remember a scanned peer's full address (type + bytes) so [`dial`](Self::dial) can whitelist it
    /// exactly — the brain only carries the 6 bytes. Keyed by bytes; the table is a tiny ring, since
    /// only a handful of distinct peers are ever mid-dial at once.
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

impl BleBackend for NrfBleBackend {
    const MAX_PEERS: usize = MEMBERS;
    type Error = Closed;
    type Link = NrfBleLink;

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), Closed> {
        diag!("backend: set_adv {}", enabled);
        self.hub.advertise.signal(enabled);
        Ok(())
    }

    async fn set_scanning(&mut self, enabled: bool) -> Result<(), Closed> {
        diag!("backend: set_scan {}", enabled);
        self.hub.scan_enabled.signal(enabled);
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
            diag!("dial: unseen {:02x}", address.octets()[0]);
            return;
        };
        let Ok(idx) = self.hub.free.try_receive() else {
            diag!("dial: pool full");
            return;
        };
        let _octets = addr.bytes();
        diag!("dial: slot {} -> {:02x}{:02x}", idx, _octets[0], _octets[1]);
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
    control_in: Receiver<'static, Mtx, Control, CTRL_DEPTH>,
    control_out: Sender<'static, Mtx, Control, CTRL_DEPTH>,
    data_in: Receiver<'static, Mtx, FrameBytes, DATA_DEPTH>,
    data_out: Sender<'static, Mtx, FrameBytes, DATA_DEPTH>,
    data_plane: &'static Signal<Mtx, L2capPlan>,
    plan: L2capPlan,
    fuse: LinkFuse,
    address: [u8; 6],
}

impl BleLink for NrfBleLink {
    type Error = Closed;
    type Source = NrfBleSource;
    type Sink = NrfBleSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
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

/// When the supervisor drops a link's halves — a keeper-duel loser it rejected, an incumbent it
/// evicted, or a member whose link already died — signal `link_dead` so the slot's serve loop returns
/// and its worker drops the physical connection (releasing the SoftDevice slot and the pool entry).
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

/// Serve one accepted peripheral connection over its slot's channels until it drops: the GATT
/// server routes the peer's control/data writes inbound (reassembling data fragments into whole
/// frames), and the outbound loop fans the supervisor's control/data out as notifications.
async fn serve_peripheral(
    l2cap: &'static l2cap::L2cap<L2capPacket>,
    server: &Server,
    conn: &Connection,
    slot: &'static LinkChannels,
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
                    let _ = control_in_tx.try_send(ctrl);
                } else {
                    diag!("gatt: control decode FAILED");
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
        },
    });

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
        let channel = match slot.data_plane.wait().await {
            L2capPlan::Accept => with_timeout(
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
                            let _ = server.rns.data_notify(conn, &value);
                        }
                    }
                    Timer::after(NOTIFY_PACING).await;
                }
            },
        }
    };

    let _ = select4(inbound, control_outbound, data, slot.link_dead.wait()).await;
}

/// Dial a peer as a central over `slot`: connect (whitelisting the resolved address), discover
/// its [`ReticulumClient`] characteristics, subscribe, then tell the supervisor the slot lit up
/// as a *dialed* link and pump it. The central twin of [`serve_peripheral`].
async fn serve_central(
    sd: &'static Softdevice,
    l2cap: &'static l2cap::L2cap<L2capPacket>,
    hub: &'static BleHub,
    idx: usize,
    addr: Address,
    slot: &'static LinkChannels,
) {
    // Hold the central-radio permit across connect + discovery (both use the SoftDevice's one
    // scanner); release it before the per-connection notification run, which uses its own portal.
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
            diag!("dial: connect failed slot {}", idx);
            return;
        }
    };
    let client: ReticulumClient = match gatt_client::discover(&conn).await {
        Ok(client) => client,
        Err(_) => {
            let _ = hub.central_token.try_send(());
            let _ = hub.dial_failed.try_send(addr.bytes());
            diag!("dial: discover failed slot {}", idx);
            return;
        }
    };
    let _ = hub.central_token.try_send(());
    let _ = client.control_cccd_write(true).await;
    let _ = client.data_cccd_write(true).await;
    slot.set_address(addr.bytes());
    hub.dialed.send(idx).await;
    diag!("link: up slot {} (dialed)", idx);

    let control_out_rx = slot.control_out.receiver();
    let data_out_rx = slot.data_out.receiver();
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();
    let mut reassembler: Reassembler<GATT_REASSEMBLY_CAP> = Reassembler::new();

    let inbound = gatt_client::run(&conn, &client, |event| match event {
        ReticulumClientEvent::ControlNotification(value) => {
            if let Some(ctrl) = Control::decode(&value) {
                let _ = control_in_tx.try_send(ctrl);
            }
        }
        ReticulumClientEvent::DataNotification(value) => {
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

/// One pool slot's worker: park until the acceptor or the dialer hands it a job, serve it in
/// whichever role the job names (a dialed link surfaces only after connect + discovery settle),
/// then signal `link_dead` and return the slot to the free list. POOL of these run concurrently.
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
        slot.link_dead.reset();
        slot.data_plane.reset();
        match job {
            SlotJob::Accept(conn) => {
                slot.set_address(conn.peer_address().bytes());
                hub.connected.send(idx).await;
                diag!("link: up slot {} (accepted)", idx);
                serve_peripheral(l2cap, server, &conn, slot).await;
            }
            SlotJob::Dial(addr) => serve_central(sd, l2cap, hub, idx, addr, slot).await,
        }
        diag!("link: down slot {}", idx);
        slot.link_dead.signal(());
        let _ = hub.free.try_send(idx);
    }
}

/// Advertise and assign each accepted connection to a free slot: the one place that calls
/// `advertise_connectable`, so the single advertising set is never double-driven. Gated by the
/// brain's `set_advertising` exactly as the scanner is gated by `set_scanning`; a mid-advertise
/// `false` (the pool filled) drops the pending advertise and releases the reserved slot.
pub(super) async fn acceptor(sd: &'static Softdevice, hub: &'static BleHub) -> ! {
    let mut enabled = false;
    loop {
        if !enabled {
            enabled = hub.advertise.wait().await;
            continue;
        }
        let idx = hub.free.receive().await;

        let mut adv_buf = [0u8; 31];
        let mut adv_len = encode_advertisement(&mut adv_buf).unwrap_or(0);
        let name = b"Prns";
        adv_buf[adv_len] = (1 + name.len()) as u8;
        adv_buf[adv_len + 1] = 0x09;
        adv_buf[adv_len + 2..adv_len + 2 + name.len()].copy_from_slice(name);
        adv_len += 2 + name.len();
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
            Either::First(Err(_error)) => {
                diag!("adv: error {:?}", _error);
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

/// Scan for peers advertising our Reticulum service so the supervisor can dial them: the central
/// half of the dual-role radio. The brain gates it through [`set_scanning`](NrfBleBackend::set_scanning);
/// a mid-scan `false` drops the in-flight scan future, stopping the radio. Each match is
/// forwarded as a [`SeenPeer`] (full address + RSSI) and its address remembered for the dial.
pub(super) async fn scanner(sd: &'static Softdevice, hub: &'static BleHub) -> ! {
    let sightings = hub.sightings.sender();
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
            let data = unsafe {
                core::slice::from_raw_parts(report.data.p_data, report.data.len as usize)
            };
            if contains_service(data) {
                Some(SeenPeer {
                    address: Address::from_raw(report.peer_addr),
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
                let _octets = peer.address.bytes();
                diag!("scan: saw {:02x}{:02x}", _octets[0], _octets[1]);
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
