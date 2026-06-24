use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_futures::join::{join3, join5};
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
use embassy_time::{Delay, Duration, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, Config as UsbConfig};
use static_cell::{ConstStaticCell, StaticCell};

use embedded_graphics::prelude::*;
use embedded_hal_bus::spi::ExclusiveDevice;
use epd_waveshare::color::Color as EpdColor;
use epd_waveshare::epd1in54_v2::Display1in54;

use nrf_softdevice::ble::{central, gatt_client, gatt_server, peripheral, Address, Connection};
use nrf_softdevice::{raw, SocEvent, Softdevice};

use personal_hopspot_ui as hopspot;
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
use personal_rns::interfaces::bluetooth_auto::core::{
    contains_service, encode_advertisement, fragments_of, BleAddress, BleIdentity, Control, Dialect,
    Endpoint, Fragment, L2capPlan, LinkCapabilities, Nrf52Host, Reassembler, BLE_HW_MTU,
    CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN,
};
use personal_rns::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin,
};
use personal_rns::interfaces::bluetooth_auto::{BluetoothAuto, BluetoothAutoShared, BluetoothAutoStatus};
use personal_rns::interfaces::rns_parity::lora::core::{channel_tag, DEFAULT_915_PROFILE};
use personal_rns::interfaces::rns_parity::lora::impls::embassy::LoRaInterface;
use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus, Membership,
};
use personal_rns::reactor::impls::embassy_reactor::{
    embassy_grant_lane, EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost,
    EmbassyInterfaceSeam, EmbassyInterfaceStatus, PooledEgress,
};
use personal_rns::reactor::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
use personal_rns::runtime::{
    EmbassyPrnsHandle, Fleet, MemberWire, PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe,
    ReactorPlumbing,
};
use personal_rns::subghz_rf::{BoardConfig, Sx126x, TcxoVoltage};
use personal_rns::wire::TransportId;

type Mtx = CriticalSectionRawMutex;
type FrameBytes = heapless09::Vec<u8, BLE_HW_MTU>;
type GattValue = heapless09::Vec<u8, 244>;
type LogLine = heapless09::String<96>;

/// One channel-set per concurrent physical connection: the BLE_MEMBERS settleable peers plus a little
/// headroom for the brief double-connection a keeper duel opens before it evicts the loser. Each is
/// role-agnostic — a peripheral (accepted) or central (dialed) link claims whichever slot is free.
const POOL: usize = crate::BLE_MEMBERS + 2;
/// `serve_slot`'s `pool_size` is a literal the task macro needs at parse time; keep it equal to POOL.
const _: () = assert!(POOL == 4, "serve_slot pool_size must equal POOL");

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

static LOG: Channel<Mtx, LogLine, 32> = Channel::new();

macro_rules! diag {
    ($($arg:tt)*) => {{
        let mut line: LogLine = heapless09::String::new();
        let _ = core::write!(&mut line, $($arg)*);
        let _ = LOG.try_send(line);
    }};
}

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    SPI2 => spim::InterruptHandler<peripherals::SPI2>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
});

/// The reactor's outbound-commit wake for the BLE fleet lane: the egress signals it on every commit
/// so the supervisor's drain is roused. On this single-core executor it is a same-core wake, but the
/// mechanism is identical to the Heltec's cross-core one — the egress producer holds it via
/// `set_outbound_wake`, the `MemberWire` carries the matching reference.
static OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();

/// The BLE supervisor's shared aggregate + per-peer status, keyed by the fleet id so each settled peer
/// becomes a fleet member under it. One member slot per concurrent peer the radio carries.
static BLE_SHARED: BluetoothAutoShared<{ crate::BLE_MEMBERS }> =
    BluetoothAutoShared::new(crate::BLE_FLEET_ID);

#[derive(Debug, Clone, Copy)]
pub struct Closed;

#[embassy_executor::task]
async fn softdevice_task(sd: &'static Softdevice, vbus: &'static SoftwareVbusDetect) -> ! {
    sd.run_with_callback(|event| match event {
        SocEvent::PowerUsbDetected => vbus.detected(true),
        SocEvent::PowerUsbPowerReady => vbus.ready(),
        SocEvent::PowerUsbRemoved => vbus.detected(false),
        _ => {}
    })
    .await
}

#[nrf_softdevice::gatt_service(uuid = "37145b00-442d-4a94-917f-8f42c5da28e3")]
struct ReticulumService {
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e7", write, notify)]
    control: GattValue,
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e8", write, notify)]
    data: GattValue,
}

#[nrf_softdevice::gatt_server]
struct Server {
    rns: ReticulumService,
}

/// The central-side view of a peer's [`ReticulumService`]: `discover` resolves these two
/// characteristics' handles on a dialed connection, `*_cccd_write` subscribes us to their
/// notifications (inbound), and `*_write` pushes our control/data out to the peer. The GATT twin of
/// the peripheral `Server`, so a dialed link speaks the same wire as an accepted one.
#[nrf_softdevice::gatt_client(uuid = "37145b00-442d-4a94-917f-8f42c5da28e3")]
struct ReticulumClient {
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e7", write, notify)]
    control: GattValue,
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e8", write, notify)]
    data: GattValue,
}

fn softdevice_config() -> nrf_softdevice::Config {
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
        // Symmetric dual-role: BLE_MEMBERS peripheral slots (peers dial us) AND BLE_MEMBERS central
        // slots (we dial), so any settled peer can take either side — the keeper duel resolves each
        // link's role by identity, ~half each way, so both counts must cover the whole settled pool.
        // periph + central = 20 is the SoftDevice's combined ceiling.
        gap_role_count: Some(raw::ble_gap_cfg_role_count_t {
            adv_set_count: 1,
            periph_role_count: crate::BLE_MEMBERS as u8,
            central_role_count: crate::BLE_MEMBERS as u8,
            central_sec_count: 0,
            _bitfield_1: raw::ble_gap_cfg_role_count_t::new_bitfield_1(0),
        }),
        ..Default::default()
    }
}

/// A peer the scanner saw advertising our service: the full [`Address`] (type + bytes, so the dialer
/// whitelists it exactly) and the report RSSI. The supervisor takes the bytes as a [`BleAddress`] for
/// the brain and stashes the full address for [`dial`](NrfBleBackend::dial).
#[derive(Clone, Copy)]
struct SeenPeer {
    address: Address,
    rssi: i8,
}

/// The per-link channel set bridging one slot's serve task (the SoftDevice GATT side) to the
/// supervisor's [`NrfBleLink`]. Role-agnostic: a peripheral serve loop or a central dial loop pumps
/// the same four lanes, and `link_dead` tears the supervisor's halves down when the connection drops.
struct LinkChannels {
    control_in: Channel<Mtx, Control, CTRL_DEPTH>,
    control_out: Channel<Mtx, Control, CTRL_DEPTH>,
    data_in: Channel<Mtx, FrameBytes, DATA_DEPTH>,
    data_out: Channel<Mtx, FrameBytes, DATA_DEPTH>,
    link_dead: Signal<Mtx, ()>,
}

impl LinkChannels {
    const fn new() -> Self {
        Self {
            control_in: Channel::new(),
            control_out: Channel::new(),
            data_in: Channel::new(),
            data_out: Channel::new(),
            link_dead: Signal::new(),
        }
    }

    fn link(&'static self) -> NrfBleLink {
        NrfBleLink {
            control_in: self.control_in.receiver(),
            control_out: self.control_out.sender(),
            data_in: self.data_in.receiver(),
            data_out: self.data_out.sender(),
            link_dead: &self.link_dead,
        }
    }
}

/// The work a free slot is handed: accept an inbound connection the acceptor already has in hand, or
/// dial a peer the brain decided to reach (carrying its full address so the central whitelists it
/// exactly). The slot worker serves whichever role the job names over the same `LinkChannels`.
enum SlotJob {
    Accept(Connection),
    Dial(Address),
}

/// The shared hub the whole BLE plane coordinates through: a pool of role-agnostic [`LinkChannels`],
/// the `assign`/`free`/`connected`/`dialed` plumbing that hands each new connection to an idle slot
/// and tells the supervisor which slot lit up (and whether it was accepted or dialed), plus the
/// radio-wide advertise/scan gates and the scanner's sighting funnel. One `static` so the slot tasks,
/// the acceptor, the scanner, and the supervisor all reference the same channels.
struct BleHub {
    slots: [LinkChannels; POOL],
    assign: [Channel<Mtx, SlotJob, 1>; POOL],
    free: Channel<Mtx, usize, POOL>,
    connected: Channel<Mtx, usize, POOL>,
    dialed: Channel<Mtx, usize, POOL>,
    /// The central-radio permit: a single token both the scanner and each dial must hold while using
    /// the SoftDevice's one scanner. `central::scan` and `central::connect` (which scans to find the
    /// whitelisted peer) cannot run at once — overlapping them fails the connect and can panic the
    /// shared connect portal — so this serializes them: one scan-or-dial on the radio at a time.
    central_token: Channel<Mtx, (), 1>,
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
            central_token: Channel::new(),
            advertise: Signal::new(),
            sightings: Channel::new(),
            scan_enabled: Signal::new(),
        }
    }
}

static HUB: BleHub = BleHub::new();

struct NrfBleBackend {
    connected: Receiver<'static, Mtx, usize, POOL>,
    dialed: Receiver<'static, Mtx, usize, POOL>,
    sightings: Receiver<'static, Mtx, SeenPeer, SIGHTING_DEPTH>,
    seen: heapless::Vec<Address, SEEN_CAP>,
    hub: &'static BleHub,
}

impl NrfBleBackend {
    fn new(hub: &'static BleHub) -> Self {
        Self {
            connected: hub.connected.receiver(),
            dialed: hub.dialed.receiver(),
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
    const MAX_PEERS: usize = crate::BLE_MEMBERS;
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
        match select3(
            self.connected.receive(),
            self.dialed.receive(),
            self.sightings.receive(),
        )
        .await
        {
            Either3::First(slot) => BleEvent::Inbound(self.hub.slots[slot].link()),
            Either3::Second(slot) => BleEvent::LinkReady {
                link: self.hub.slots[slot].link(),
                origin: Origin::Dialed,
                peer_rssi: None,
            },
            Either3::Third(peer) => {
                self.remember(peer.address);
                BleEvent::Sighting {
                    address: BleAddress::new(peer.address.bytes()),
                    rssi: Some(peer.rssi),
                }
            }
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
        let octets = addr.bytes();
        diag!("dial: slot {} -> {:02x}{:02x}", idx, octets[0], octets[1]);
        if self.hub.assign[idx].try_send(SlotJob::Dial(addr)).is_err() {
            let _ = self.hub.free.try_send(idx);
        }
    }
}

struct NrfBleLink {
    control_in: Receiver<'static, Mtx, Control, CTRL_DEPTH>,
    control_out: Sender<'static, Mtx, Control, CTRL_DEPTH>,
    data_in: Receiver<'static, Mtx, FrameBytes, DATA_DEPTH>,
    data_out: Sender<'static, Mtx, FrameBytes, DATA_DEPTH>,
    link_dead: &'static Signal<Mtx, ()>,
}

impl BleLink for NrfBleLink {
    type Error = Closed;
    type Source = NrfBleSource;
    type Sink = NrfBleSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        BleAddress::new([0u8; 6])
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), Closed> {
        match select(self.control_out.send(*msg), self.link_dead.wait()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn control_recv(&mut self) -> Result<Control, Closed> {
        match select(self.control_in.receive(), self.link_dead.wait()).await {
            Either::First(msg) => Ok(msg),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn upgrade(&mut self, _plan: &L2capPlan) -> Result<(), Closed> {
        Ok(())
    }

    fn into_data(self) -> (NrfBleSource, NrfBleSink) {
        (
            NrfBleSource {
                data_in: self.data_in,
                link_dead: self.link_dead,
            },
            NrfBleSink {
                data_out: self.data_out,
                link_dead: self.link_dead,
            },
        )
    }
}

struct NrfBleSource {
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

struct NrfBleSink {
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
/// The sink is the always-present half of the settled pair, so signalling here covers every teardown.
impl Drop for NrfBleSink {
    fn drop(&mut self) {
        self.link_dead.signal(());
    }
}

/// Serve one accepted peripheral connection over its slot's channels until it drops: the GATT server
/// routes the peer's control/data writes inbound (reassembling data fragments into whole frames), and
/// the outbound loop fans the supervisor's control/data back out as GATT notifications. This is the
/// body the old single-connection `driver` ran, now parameterized by slot so POOL of them serve at
/// once. Returns when the GATT server reports the link disconnected.
async fn serve_peripheral(server: &Server, conn: &Connection, slot: &'static LinkChannels) {
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

    let outbound = async {
        loop {
            match select(control_out_rx.receive(), data_out_rx.receive()).await {
                Either::First(ctrl) => {
                    let mut buf = [0u8; CONTROL_MAX_LEN];
                    if let Some(n) = ctrl.encode(&mut buf) {
                        if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                            let _ = server.rns.control_notify(conn, &value);
                        }
                    }
                }
                Either::Second(frame) => {
                    for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                        let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                        if let Some(n) = fragment.encode(&mut buf) {
                            if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                                let _ = server.rns.data_notify(conn, &value);
                            }
                        }
                        Timer::after(NOTIFY_PACING).await;
                    }
                }
            }
        }
    };

    let _ = select3(inbound, outbound, slot.link_dead.wait()).await;
}

/// Dial a peer as a central over `slot`: connect (whitelisting the resolved address), discover its
/// [`ReticulumClient`] characteristics, subscribe to their notifications, then tell the supervisor the
/// slot lit up as a *dialed* link and pump it. Returns (freeing the slot) if the connect or discovery
/// fails, or when the link drops. The central twin of [`serve_peripheral`].
async fn serve_central(
    sd: &'static Softdevice,
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
    let conn = match central::connect(sd, &config).await {
        Ok(conn) => conn,
        Err(_) => {
            let _ = hub.central_token.try_send(());
            diag!("dial: connect failed slot {}", idx);
            return;
        }
    };
    let client: ReticulumClient = match gatt_client::discover(&conn).await {
        Ok(client) => client,
        Err(_) => {
            let _ = hub.central_token.try_send(());
            diag!("dial: discover failed slot {}", idx);
            return;
        }
    };
    let _ = hub.central_token.try_send(());
    let _ = client.control_cccd_write(true).await;
    let _ = client.data_cccd_write(true).await;
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

    let outbound = async {
        loop {
            match select(control_out_rx.receive(), data_out_rx.receive()).await {
                Either::First(ctrl) => {
                    let mut buf = [0u8; CONTROL_MAX_LEN];
                    if let Some(n) = ctrl.encode(&mut buf) {
                        if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                            let _ = client.control_write(&value).await;
                        }
                    }
                }
                Either::Second(frame) => {
                    for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                        let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                        if let Some(n) = fragment.encode(&mut buf) {
                            if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                                let _ = client.data_write(&value).await;
                            }
                        }
                        Timer::after(NOTIFY_PACING).await;
                    }
                }
            }
        }
    };

    let _ = select3(inbound, outbound, slot.link_dead.wait()).await;
}

/// One pool slot's worker: park until the acceptor or the dialer hands it a job, mark the slot live,
/// serve it in whichever role the job names (an accepted link surfaces to the supervisor at once; a
/// dialed one only after its connect + discovery settle), then signal `link_dead` and return the slot
/// to the free list. POOL of these run concurrently — the embedded twin of the desktop supervisor's
/// per-connection tasks.
#[embassy_executor::task(pool_size = 4)]
async fn serve_slot(idx: usize, sd: &'static Softdevice, server: &'static Server, hub: &'static BleHub) {
    let slot = &hub.slots[idx];
    loop {
        let job = hub.assign[idx].receive().await;
        slot.link_dead.reset();
        match job {
            SlotJob::Accept(conn) => {
                hub.connected.send(idx).await;
                diag!("link: up slot {} (accepted)", idx);
                serve_peripheral(server, &conn, slot).await;
            }
            SlotJob::Dial(addr) => serve_central(sd, hub, idx, addr, slot).await,
        }
        diag!("link: down slot {}", idx);
        slot.link_dead.signal(());
        let _ = hub.free.try_send(idx);
    }
}

/// Advertise and assign each accepted connection to a free slot — the one place that calls
/// `advertise_connectable`, so the single advertising set is never double-driven. Gated by the brain's
/// `set_advertising` (the `bool` on `advertise`) exactly as the scanner is gated by `set_scanning`: it
/// reserves a free slot, advertises into it, hands the connection to that slot's worker, then loops to
/// fill the next. A mid-advertise `false` (the pool filled) drops the pending advertise and releases
/// the reserved slot.
async fn acceptor(sd: &'static Softdevice, hub: &'static BleHub) -> ! {
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
            Either::First(Err(e)) => {
                diag!("adv: error {:?}", e);
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

/// Scan for peers advertising our Reticulum service so the supervisor can dial them — the central
/// half of the radio, run alongside the peripheral `driver` on the dual-role SoftDevice. The brain
/// gates it through [`set_scanning`](NrfBleBackend::set_scanning): a `true`/`false` lands on
/// `scan_enabled`, and a mid-scan `false` drops the in-flight scan future, stopping the radio. Each
/// matched peer is forwarded as a [`SeenPeer`] (full address + RSSI); the supervisor turns that into
/// a `Sighting` for the brain and remembers the address for the dial.
async fn scanner(sd: &'static Softdevice, hub: &'static BleHub) -> ! {
    let sightings = hub.sightings.sender();
    let mut enabled = false;
    loop {
        if !enabled {
            enabled = hub.scan_enabled.wait().await;
            continue;
        }
        hub.central_token.receive().await;
        let mut config = central::ScanConfig::default();
        config.extended = false;
        config.timeout = SCAN_WINDOW_TICKS;
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
                let octets = peer.address.bytes();
                diag!("scan: saw {:02x}{:02x}", octets[0], octets[1]);
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

/// Build the card set for the e-ink: the LoRa wire, the BLE supervisor aggregate, and one card per
/// settled BLE peer — the same shape the Heltec and desktop faces render.
fn build_cards(lora: &EmbassyInterfaceStatus) -> heapless::Vec<hopspot::Card, 8> {
    let ble = BluetoothAutoStatus::new(&BLE_SHARED);
    let lora_id = lora.id();
    let classify = |id: InterfaceId| -> Option<(hopspot::CardKind, hopspot::CardLabel)> {
        if id == lora_id {
            Some((hopspot::CardKind::LoRa, hopspot::card_label("LoRa")))
        } else if id == crate::BLE_FLEET_ID {
            Some((hopspot::CardKind::Ble, hopspot::card_label("BLE")))
        } else {
            let bytes = id.as_bytes();
            let mut label = hopspot::CardLabel::new();
            let _ = write!(label, "Peer {:02x}{:02x}", bytes[1], bytes[2]);
            Some((hopspot::CardKind::Peer, label))
        }
    };
    let mut entries: heapless::Vec<(&dyn InterfaceStatus, Membership), 8> = heapless::Vec::new();
    let _ = entries.push((lora, Membership::Independent));
    let supervisor_id = ble.id();
    let _ = entries.push((&ble, Membership::Independent));
    for member in ble.members() {
        let _ = entries.push((member, Membership::FleetMember { supervisor_id }));
    }
    let mut snapshots: heapless::Vec<InterfaceSnapshot, 8> = heapless::Vec::new();
    for (status, membership) in &entries {
        let id = status.id();
        let counts = crate::INTERFACE_COUNTS.counts(id);
        let _ = snapshots.push(InterfaceSnapshot {
            id,
            connection: status.connection(),
            rx_bytes: status.rx_bytes(),
            tx_bytes: status.tx_bytes(),
            transfer_rates: status.transfer_rates(),
            destinations: counts.destinations,
            links: counts.links,
            transported_links: counts.transported_links,
            membership: *membership,
        });
    }
    hopspot::snapshots_to_cards(&snapshots, classify)
}

/// Stand the T-Echo up as a real engine node carrying both LoRa and BLE: the SX1262 on slot 0 and the
/// [`BluetoothAuto`] supervisor's one shared fleet lane on slot 1. The SoftDevice owns CLOCK/POWER and
/// feeds USB vbus over its SoC events, so USB uses a [`SoftwareVbusDetect`]; the SX1262 and e-ink SPI
/// peripherals are not SD-reserved, so they coexist with the BLE radio. A settled BLE central becomes
/// a fleet member, lighting the BLE card and carrying Reticulum frames exactly like the WiFi peers do
/// on the Heltec. Never returns: this frame is the board's whole I/O + engine + radio drive.
#[allow(clippy::too_many_lines)]
pub async fn run(spawner: Spawner) -> ! {
    let mut nrf_config = config::Config::default();
    nrf_config.gpiote_interrupt_priority = Priority::P2;
    nrf_config.time_interrupt_priority = Priority::P2;
    let p = embassy_nrf::init(nrf_config);

    let _eink_rail = Output::new(p.P0_12, Level::High, OutputDrive::Standard);
    let mut led = Output::new(p.P1_01, Level::High, OutputDrive::Standard);

    // The SoftDevice reserves P0/P1/P4; keep every app interrupt off those. USB at P2 (matches the
    // validated bring-up); the two SPI buses at P3 so a BLE radio event can preempt them.
    interrupt::USBD.set_priority(Priority::P2);
    interrupt::SPI2.set_priority(Priority::P3);
    interrupt::TWISPI0.set_priority(Priority::P3);

    static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
    let vbus = SOFTWARE_VBUS.init(SoftwareVbusDetect::new(true, true));

    let usb_driver = Driver::new(p.USBD, Irqs, &*vbus);
    let mut usb_config = UsbConfig::new(0x1209, 0x0001);
    usb_config.manufacturer = Some("Stay Personal");
    usb_config.product = Some("Personal Hopspot (T-Echo BLE)");
    usb_config.serial_number = Some("PERSONAL-RNS-TECHO-BLE");
    usb_config.max_packet_size_0 = 64;
    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static USB_STATE: StaticCell<State> = StaticCell::new();
    let mut builder = Builder::new(
        usb_driver,
        usb_config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 256]),
        CONTROL_BUF.init([0; 64]),
    );
    let mut class = CdcAcmClass::new(&mut builder, USB_STATE.init(State::new()), 64);
    let mut usb = builder.build();

    // The SoftDevice owns the radio + CLOCK/POWER, and feeds the USB vbus detector over its SoC
    // events; bring it up here (before the dalek-heavy engine construction) so its boot matches the
    // validated first-light ordering. Constructing the engine afterward is fine — the SD's own
    // high-priority interrupts keep the radio alive across the synchronous build.
    diag!("boot: techo ble node");
    diag!("sd: enabling");
    let sd = Softdevice::enable(&softdevice_config());
    static SERVER: StaticCell<Server> = StaticCell::new();
    let server: &'static Server = SERVER.init(Server::new(sd).unwrap());
    spawner.spawn(softdevice_task(sd, vbus).expect("softdevice task fits"));
    diag!("sd: enabled");

    // The connection-slot pool: one worker per slot, parked until the acceptor or the dialer hands it
    // a connection. Pre-fill the free list so the acceptor has slots to advertise into, and seed the
    // single central-radio permit so exactly one scan-or-dial uses the SoftDevice's scanner at a time.
    let _ = HUB.central_token.try_send(());
    for idx in 0..POOL {
        let _ = HUB.free.try_send(idx);
        spawner.spawn(serve_slot(idx, sd, server, &HUB).expect("serve slot fits"));
    }

    // SX1262 LoRa radio on TWISPI0 (the T-Echo's radio bus).
    let mut radio_spim_config = spim::Config::default();
    radio_spim_config.frequency = spim::Frequency::M4;
    let radio_bus = Spim::new(
        p.TWISPI0,
        Irqs,
        p.P0_19,
        p.P0_23,
        p.P0_22,
        radio_spim_config,
    );
    let radio_cs = Output::new(p.P0_24, Level::High, OutputDrive::Standard);
    let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
    let radio_busy = Input::new(p.P0_17, Pull::None);
    let radio_dio1 = Input::new(p.P0_20, Pull::None);
    let radio_reset = Output::new(p.P0_25, Level::High, OutputDrive::Standard);
    let radio = Sx126x::new(
        radio_spi,
        radio_busy,
        radio_dio1,
        radio_reset,
        Delay,
        BoardConfig {
            tcxo_voltage: Some(TcxoVoltage::V1_8),
            use_dcdc: true,
            rx_boost: true,
            dio2_as_rf_switch: true,
        },
    );

    // The 1.54" e-ink on SPI2.
    let mut eink_spim_config = spim::Config::default();
    eink_spim_config.frequency = spim::Frequency::M4;
    let eink_bus = Spim::new(p.SPI2, Irqs, p.P0_31, p.P1_06, p.P0_29, eink_spim_config);
    let eink_cs = Output::new(p.P0_30, Level::High, OutputDrive::Standard);
    let eink_dc = Output::new(p.P0_28, Level::Low, OutputDrive::Standard);
    let eink_rst = Output::new(p.P0_02, Level::High, OutputDrive::Standard);
    let eink_busy = Input::new(p.P0_03, Pull::None);
    Timer::after(Duration::from_millis(150)).await;
    let eink_spi = ExclusiveDevice::new(eink_bus, eink_cs, Delay).unwrap();
    let mut panel = Display1in54::default();
    let eink = crate::ssd1681::Ssd1681::new(eink_spi, eink_busy, eink_dc, eink_rst, Delay).ok();

    // Self-identity: the same fixture keypair the LoRa-only build uses, so the board keeps one
    // destination across builds. The BLE identity is the transport id (16 bytes).
    let secret_key = crate::techo_secret_key();
    let (self_destination, transport_id) = {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = personal_rns::routing::announce::expand_name("lxmf", &["delivery"])
            .expect("valid name");
        let destination = personal_rns::routing::announce::derive_destination_hash(
            &signer.identity_hash(),
            &name,
        );
        let transport = TransportId::new(*signer.identity_hash().as_bytes());
        (destination, transport)
    };
    let node_identity: [u8; 16] = *transport_id.as_bytes();
    let seed = self_destination.as_bytes();
    crate::ENTROPY_STATE.store(
        u32::from_le_bytes([seed[0], seed[1], seed[2], seed[3]]) | 1,
        core::sync::atomic::Ordering::Relaxed,
    );

    // The reactor's slot pool: LoRa on slot 0, the BLE fleet's one shared lane on slot 1. The fleet
    // slot's egress producer carries the outbound wake so a committed frame rouses the supervisor.
    static IN_BUF: [ConstStaticCell<crate::LaneBuf>; crate::IFACES] =
        [const { ConstStaticCell::new([crate::EMPTY_SLOT; crate::LANE_DEPTH]) }; crate::IFACES];
    static IN_CH: [StaticCell<crate::LaneChannel>; crate::IFACES] =
        [const { StaticCell::new() }; crate::IFACES];
    static OUT_BUF: [ConstStaticCell<crate::LaneBuf>; crate::IFACES] =
        [const { ConstStaticCell::new([crate::EMPTY_SLOT; crate::LANE_DEPTH]) }; crate::IFACES];
    static OUT_CH: [StaticCell<crate::LaneChannel>; crate::IFACES] =
        [const { StaticCell::new() }; crate::IFACES];

    let mut inbound: crate::ReactorInbound = heapless::Vec::new();
    let mut egress_lanes: crate::ReactorEgressLanes = heapless::Vec::new();
    let mut iface_halves: [Option<(
        EmbassyGrantProducer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
        EmbassyGrantConsumer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
    )>; crate::IFACES] = [const { None }; crate::IFACES];
    for slot in 0..crate::IFACES {
        let in_ch = IN_CH[slot].init(zerocopy_channel::Channel::new(IN_BUF[slot].take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH[slot].init(zerocopy_channel::Channel::new(OUT_BUF[slot].take()));
        let (mut out_producer, out_consumer) = embassy_grant_lane(out_ch);
        if slot == crate::BLE_FLEET_SLOT {
            out_producer.set_outbound_wake(&OUTBOUND_WAKE);
        }
        let _ = inbound.push((crate::FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((crate::FREE_SLOT, out_producer));
        iface_halves[slot] = Some((in_producer, out_consumer));
    }

    let lora_profile = DEFAULT_915_PROFILE;
    let lora_id = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&lora_profile));
    static LORA_STATUS: StaticCell<EmbassyInterfaceStatus> = StaticCell::new();
    let lora_status: &'static EmbassyInterfaceStatus = LORA_STATUS.init(
        EmbassyInterfaceStatus::new(lora_id, ConnectionState::Initializing),
    );
    let lora = LoRaInterface::new(
        radio,
        lora_profile,
        &crate::LORA_CONTROL,
        lora_status,
        crate::LIFECYCLE.dyn_sender(),
    );

    let handle = EmbassyPrnsHandle::new(crate::COMMANDS.sender(), &crate::COMPLETION);
    let plumbing = ReactorPlumbing::new(
        inbound,
        PooledEgress::new(egress_lanes),
        crate::NOTIFY.receiver(),
        crate::COMMANDS.receiver(),
        crate::LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new(crate::seeded_entropy as fn(&mut [u8]));
    static NODE: StaticCell<crate::Node> = StaticCell::new();
    let node: &'static mut crate::Node = NODE.init(Prns::new(
        PrnsRecipe {
            transport: Some(transport_id),
            pre_configured_destinations: [PreConfiguredDestination::Single {
                resource_strategy:
                    personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
                app_name: "lxmf",
                aspects: &["delivery"],
                identity: secret_key,
                announce_app_data: crate::ANNOUNCE_APP_DATA,
                proof: personal_rns::routing::ProofStrategy::ProveAll,
                ratchet: RatchetPolicy::Ratcheted,
            }],
            app_state: (),
            storage: crate::storage::TechoStorage,
            routes: personal_rns::routes![],
            interfaces: personal_rns::interfaces![],
            on_event: crate::ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
        },
        plumbing,
        host,
        heapless::Vec::new(),
    ));
    node.activate(crate::LORA_SLOT, lora.descriptor());
    node.activate_fleet(crate::BLE_FLEET_SLOT, crate::BLE_FLEET_ID);
    node.set_interface_store(&crate::INTERFACE_COUNTS);

    let (lora_in_producer, lora_out_consumer) =
        iface_halves[crate::LORA_SLOT].take().expect("lora slot half");
    let lora_seam = EmbassyInterfaceSeam::new(
        lora_id,
        lora_in_producer,
        crate::NOTIFY.sender(),
        lora_out_consumer,
    );

    let (ble_in_producer, ble_out_consumer) = iface_halves[crate::BLE_FLEET_SLOT]
        .take()
        .expect("ble fleet half");
    let fleet: Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, { crate::NOTIFY_CAP }, { crate::LIFECYCLE_CAP }> =
        Fleet::new(
            MemberWire {
                inbound: ble_in_producer,
                outbound: ble_out_consumer,
                notify: crate::NOTIFY.sender(),
                outbound_wake: &OUTBOUND_WAKE,
            },
            crate::LIFECYCLE.sender(),
        );

    // The bridged backend the supervisor drives. Advertising is the supervisor's to enable — it calls
    // `set_advertising(true)` at startup — so no manual signal here (that would race it).
    let backend = NrfBleBackend::new(&HUB);
    let supervisor = BluetoothAuto::new(
        backend,
        BleIdentity::new(node_identity),
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        LinkCapabilities {
            l2cap: None,
            link_mtu: BLE_HW_MTU as u16,
        },
        &BLE_SHARED,
    );

    let button = Input::new(p.P1_10, Pull::Up);
    let frontlight = Output::new(p.P1_11, Level::Low, OutputDrive::Standard);

    let usb_fut = usb.run();

    let log_writer = async {
        loop {
            let line = LOG.receive().await;
            for chunk in line.as_bytes().chunks(60) {
                let _ = class.write_packet(chunk).await;
            }
            let _ = class.write_packet(b"\r\n").await;
        }
    };

    let heartbeat = async {
        let mut n = 0u32;
        loop {
            Timer::after(Duration::from_secs(1)).await;
            n = n.wrapping_add(1);
            if n & 1 == 0 {
                led.set_low();
            } else {
                led.set_high();
            }
            diag!("alive {}", n);
        }
    };

    let ui_handle = EmbassyPrnsHandle::new(crate::COMMANDS.sender(), &crate::COMPLETION);
    let render = async move {
        let mut epd = match eink {
            Some(epd) => epd,
            None => core::future::pending().await,
        };
        let mut ui_state = hopspot::UiState::new();
        let mut working_lora_profile = DEFAULT_915_PROFILE;
        let mut since_full = 0u32;
        let mut displayed_hash = 0u64;
        let mut have_displayed = false;
        loop {
            let cards = build_cards(lora_status);
            let card_count = cards.len();
            ui_state.sync_card_count(card_count);

            let _ = panel.clear(EpdColor::White);
            hopspot::draw_with_state(
                &mut crate::EinkScreen { panel: &mut panel },
                &cards,
                hopspot::BatteryState::Unknown,
                &ui_state,
            );
            let hash = crate::frame_hash(panel.buffer());
            if !have_displayed || hash != displayed_hash {
                if !have_displayed || since_full >= crate::FULL_REFRESH_INTERVAL {
                    let _ = epd.full_update(panel.buffer());
                    since_full = 0;
                } else {
                    let _ = epd.partial_update(panel.buffer());
                }
                since_full += 1;
                displayed_hash = hash;
                have_displayed = true;
            }

            match select3(
                crate::BUTTON_EVENTS.receive(),
                crate::INTERFACE_COUNTS.changed(),
                Timer::after(crate::STATS_POLL),
            )
            .await
            {
                Either3::First(event) => {
                    let selected_kind = ui_state
                        .selected_card(card_count)
                        .and_then(|index| cards.get(index))
                        .map(|card| card.kind);
                    match ui_state.handle_input(event, card_count, selected_kind) {
                        hopspot::UiAction::Announce => {
                            let _ = ui_handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
                                destination: self_destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Registered,
                            }));
                        }
                        hopspot::UiAction::ToggleSelectedInterface => {
                            if let Some(card) = ui_state
                                .selected_card(card_count)
                                .and_then(|index| cards.get(index))
                            {
                                if card.id == lora_status.id() {
                                    lora_status.set_enabled(!lora_status.is_enabled());
                                }
                            }
                        }
                        hopspot::UiAction::OpenLoRaEditor => {
                            ui_state.open_lora_editor(working_lora_profile);
                        }
                        hopspot::UiAction::SetLoRaProfile(profile) => {
                            working_lora_profile = profile;
                            crate::LORA_CONTROL.signal(profile);
                        }
                        hopspot::UiAction::None => {}
                    }
                }
                Either3::Second(()) => {}
                Either3::Third(()) => {}
            }
        }
    };

    diag!("join: entering");
    let io = join5(
        usb_fut,
        log_writer,
        heartbeat,
        crate::drive_button(button),
        crate::drive_frontlight(frontlight),
    );
    let ble_plane = join3(
        acceptor(sd, &HUB),
        scanner(sd, &HUB),
        supervisor.run(fleet),
    );
    let mesh = join3(node.run_reactor(), lora.run(lora_seam), render);
    join3(io, ble_plane, mesh).await;
    loop {}
}
