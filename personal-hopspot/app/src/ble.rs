//! The Heltec V4 / T-Beam (ESP32-S3) native-Bluetooth backend: trouble-host bridged to the engine's
//! [`BleBackend`] seam, driven by the embassy [`BluetoothAuto`] supervisor so a settled BLE peer
//! becomes a real engine interface (a fleet member) exactly like the WiFi/USB ones. Dual-role: the
//! board both **advertises** a GATT server (a central dials us → `Inbound`) AND **scans + dials** as a
//! central (we find a peer advertising our service → `LinkReady{Dialed}`), so two boards mesh directly
//! and the shared brain's keeper duel resolves which side keeps the link.
//!
//! trouble's `GattConnection`/`GattClient` are lifetime-bound to the stack, so they cannot move to a
//! `'static` task. Instead the radio loops run as joined *driver* futures that demultiplex the one live
//! connection over a `'static` bridge: control-characteristic traffic to a control channel, data
//! reassembled/fragmented onto a data channel. The seam ([`EmbeddedBleBackend`]/`Link`/`Source`/`Sink`)
//! reads those stack-local channels, decoupled from the connection's lifetime; link death is a
//! level-triggered [`Signal`] the driver raises on disconnect and clears per connection. The radio
//! carries one connection at a time (`CONNECTIONS = 1`), so there is one bridge, served by whichever
//! role won it (peripheral serve-loop or central serve-loop).

use core::cell::Cell;
use embassy_futures::join::join;
use embassy_futures::select::{select, select4, Either, Either4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as BridgeMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_sync_07::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{with_timeout, Duration, Timer};
use esp_radio::ble::controller::BleConnector;
use heapless_09::Vec as GattVec;

use personal_rns::interfaces::bluetooth_auto::core::{
    contains_service, encode_advertisement, fragments_of, BleAddress, BleIdentity, Control,
    Dialect, Endpoint, Esp32Host, Fragment, L2capPlan, LinkCapabilities, Reassembler, BLE_HW_MTU,
    BLE_SERVICE_UUID_BYTES, CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN, MAX_ADVERTISEMENT_LEN,
};
use personal_rns::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource, Origin,
};
use personal_rns::interfaces::bluetooth_auto::{BluetoothAuto, BluetoothAutoShared};
use personal_rns::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use personal_rns::runtime::Fleet;
use static_cell::StaticCell;
use trouble_host::prelude::*;

use crate::esp32s3::{BLE_MEMBERS, LIFECYCLE_CAP, NOTIFY_CAP};

type BleFleet = Fleet<BridgeMutex, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP>;

const HCI_COMMAND_SLOTS: usize = 20;
const CONNECTIONS: usize = 1;
const L2CAP_CHANNELS: usize = 2;
const ATTRIBUTE_TABLE: usize = 32;
const CCCD_TABLE: usize = 4;
const GATT_VALUE_CAP: usize = 244;
/// The central side discovers exactly our one [`ReticulumService`]; a tiny known-services table fits it.
const MAX_SERVICES: usize = 2;

const CONTROL_UUID_LAST: u8 = 0xe7;
const DATA_UUID_LAST: u8 = 0xe8;
const SERVICE_UUID_LAST: u8 = 0xe3;

/// The GATT data lane's fragmentation, byte-identical to the Android/nRF backends so they interoperate:
/// reassemble inbound writes up to [`GATT_REASSEMBLY_CAP`], fragment outbound frames to
/// [`GATT_FRAGMENT_PAYLOAD`]-byte chunks under the 5-byte fragment header.
const GATT_REASSEMBLY_CAP: usize = 600;
const GATT_FRAGMENT_PAYLOAD: usize = 180;

/// Pace the GATT data fragments so a multi-fragment frame does not blast the controller's TX queue
/// back-to-back: the controller gets a moment to put each fragment on air before the next is queued,
/// keeping the radio stable under sustained announce traffic instead of overrunning it.
const NOTIFY_PACING: Duration = Duration::from_millis(15);
/// A single notify/write that never resolves must not wedge the driver — and through the shared
/// controller, the whole radio — so each is bounded; on timeout the frame is dropped and the link left
/// to recover rather than blocking forever.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);
/// A dial whitelists the scanned peer and scans for it before connecting. `central::connect` with a
/// zero timeout scans *forever*, so a dial to a peer that has since stopped advertising would wedge the
/// central loop (no further scanning or dialing). Bound it: on timeout the connect errors, the brain
/// marks the address Unreachable, and scanning resumes.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
/// A dialed peer that connects but stalls the GATT bring-up (MTU exchange / discovery / subscribe) must
/// not wedge the radio, so the whole bring-up is bounded; on timeout the link is dropped and the brain
/// backs the address off.
const GATT_SETUP_TIMEOUT: Duration = Duration::from_secs(6);

/// The radio carries one connection at a time, so advertising (peripheral) and scanning (central)
/// time-share it in alternating windows rather than running at once: this keeps only one serve frame
/// (peripheral OR central) on core 0's stack instead of both, and sidesteps any controller limit on
/// simultaneous advertise+scan. Two boards alternating overlap within a cycle, so discovery still
/// converges; a `Dial` the brain decides during an off-window is buffered and served at the next scan
/// window.
const ADV_WINDOW: Duration = Duration::from_millis(600);
const SCAN_WINDOW: Duration = Duration::from_millis(600);

/// The bridge channels' depths and frame buffer. Control is lockstep (handshake), so a shallow lane
/// suffices; data buffers a few frames so a slow reactor never stalls the GATT read.
const CTRL_DEPTH: usize = 4;
const DATA_DEPTH: usize = 4;
const SIGHTING_DEPTH: usize = 4;
/// Recently-scanned peers, kept so [`dial`](EmbeddedBleBackend::dial) (which the brain calls with only
/// the 6 address bytes) can recover the full `(AddrKind, BdAddr)` the central must whitelist to connect.
const SEEN_CAP: usize = 8;
const FRAME_CAP: usize = BLE_HW_MTU;

type FrameBytes = heapless::Vec<u8, FRAME_CAP>;

fn reticulum_uuid(last: u8) -> Uuid {
    let mut bytes = BLE_SERVICE_UUID_BYTES;
    bytes[15] = last;
    Uuid::from(u128::from_be_bytes(bytes))
}

/// The seam's error: the link is gone (the peer disconnected, or the bridge frame would not fit).
#[derive(Debug)]
struct Closed;

/// A peer the scanner saw advertising our service: the full `(AddrKind, BdAddr)` (so the dialer
/// whitelists it exactly) and the report RSSI. The backend stashes the address for [`dial`] and turns
/// the bytes into a [`BleAddress`] sighting for the brain.
#[derive(Clone, Copy)]
struct SeenPeer {
    kind: AddrKind,
    addr: BdAddr,
    rssi: i8,
}

/// The full radio address the central must whitelist to dial a peer, carried from a sighting through
/// the brain's `Dial` back to the central loop.
#[derive(Clone, Copy)]
struct DialTarget {
    kind: AddrKind,
    addr: BdAddr,
}

/// The `'static` bridge between the radio driver futures and the supervisor. A `CriticalSectionRawMutex`
/// guards each lane because the halves run on different executors. One connection at a time, so one of
/// each data lane; the role-coordination signals (advertise/scan enable, dial request, sighting funnel,
/// up-notifications) live here too so the acceptor, the central loop, the scan event handler, and the
/// supervisor all reference the same `static`.
struct BleBridge {
    connected: Channel<BridgeMutex, (), 2>,
    dialed: Channel<BridgeMutex, (), 2>,
    dial_failed: Channel<BridgeMutex, [u8; 6], 2>,
    control_in: Channel<BridgeMutex, Control, CTRL_DEPTH>,
    control_out: Channel<BridgeMutex, Control, CTRL_DEPTH>,
    data_in: Channel<BridgeMutex, FrameBytes, DATA_DEPTH>,
    data_out: Channel<BridgeMutex, FrameBytes, DATA_DEPTH>,
    link_dead: Signal<BridgeMutex, ()>,
    advertise: Signal<BridgeMutex, bool>,
    scan_enabled: Signal<BridgeMutex, bool>,
    sightings: Channel<BridgeMutex, SeenPeer, SIGHTING_DEPTH>,
    dial_request: Channel<BridgeMutex, DialTarget, 2>,
    /// The connected peer's address, stashed by the serve loop the moment the link lands (from
    /// `conn.peer_address()` for an accept, the dialed address for a dial) and read by [`link`] so the
    /// brain keys this peer correctly — it keys settled-peer lookup and dial/suppress backoff by
    /// address, so a stale all-zero address would collide every peer on one backoff entry.
    peer_addr: BlockingMutex<BridgeMutex, Cell<[u8; 6]>>,
}

impl BleBridge {
    const fn new() -> Self {
        Self {
            connected: Channel::new(),
            dialed: Channel::new(),
            dial_failed: Channel::new(),
            control_in: Channel::new(),
            control_out: Channel::new(),
            data_in: Channel::new(),
            data_out: Channel::new(),
            link_dead: Signal::new(),
            advertise: Signal::new(),
            scan_enabled: Signal::new(),
            sightings: Channel::new(),
            dial_request: Channel::new(),
            peer_addr: BlockingMutex::new(Cell::new([0u8; 6])),
        }
    }

    fn set_peer_addr(&self, bytes: [u8; 6]) {
        self.peer_addr.lock(|cell| cell.set(bytes));
    }

    fn clear_lanes(&self) {
        self.link_dead.reset();
        self.control_in.clear();
        self.control_out.clear();
        self.data_in.clear();
        self.data_out.clear();
    }
}

/// The trouble→seam bridge as a [`BleBackend`]: it surfaces the one live link (whichever role won it)
/// reading/writing the `'static` bridge channels, the scanner's sightings, and dial failures. `dial`
/// resolves the brain's address to the full scanned target and hands it to the central loop.
struct EmbeddedBleBackend {
    bridge: &'static BleBridge,
    connected: Receiver<'static, BridgeMutex, (), 2>,
    dialed: Receiver<'static, BridgeMutex, (), 2>,
    dial_failed: Receiver<'static, BridgeMutex, [u8; 6], 2>,
    sightings: Receiver<'static, BridgeMutex, SeenPeer, SIGHTING_DEPTH>,
    dial_request: Sender<'static, BridgeMutex, DialTarget, 2>,
    seen: heapless::Vec<DialTarget, SEEN_CAP>,
}

impl EmbeddedBleBackend {
    /// Remember a scanned peer's full address so [`dial`](Self::dial) can whitelist it — the brain only
    /// carries the 6 bytes. A tiny ring keyed by bytes; only a handful are ever mid-dial at once.
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

    fn link(&self) -> EmbeddedBleLink {
        EmbeddedBleLink {
            control_in: self.bridge.control_in.receiver(),
            control_out: self.bridge.control_out.sender(),
            data_in: self.bridge.data_in.receiver(),
            data_out: self.bridge.data_out.sender(),
            link_dead: &self.bridge.link_dead,
            address: self.bridge.peer_addr.lock(|cell| cell.get()),
        }
    }
}

impl BleBackend for EmbeddedBleBackend {
    const MAX_PEERS: usize = 1;
    type Error = Closed;
    type Link = EmbeddedBleLink;

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), Closed> {
        self.bridge.advertise.signal(enabled);
        Ok(())
    }

    async fn set_scanning(&mut self, enabled: bool) -> Result<(), Closed> {
        self.bridge.scan_enabled.signal(enabled);
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<EmbeddedBleLink> {
        match select4(
            self.connected.receive(),
            self.dialed.receive(),
            self.sightings.receive(),
            self.dial_failed.receive(),
        )
        .await
        {
            Either4::First(()) => BleEvent::Inbound(self.link()),
            Either4::Second(()) => BleEvent::LinkReady {
                link: self.link(),
                origin: Origin::Dialed,
                peer_rssi: None,
            },
            Either4::Third(peer) => {
                self.remember(peer);
                BleEvent::Sighting {
                    address: BleAddress::new(peer.addr.into_inner()),
                    rssi: Some(peer.rssi),
                }
            }
            Either4::Fourth(bytes) => BleEvent::DialFailed {
                address: BleAddress::new(bytes),
            },
        }
    }

    async fn dial(&mut self, address: BleAddress) {
        if let Some(target) = self.resolve(address) {
            let _ = self.dial_request.try_send(target);
        }
    }

    async fn on_link_closed(&mut self, _address: BleAddress) {
        // The supervisor rejected/closed the link (handshake timeout/abort, keeper-duel loss, or a
        // settled member dropping). Raise link_dead so the active serve loop (peripheral or central)
        // returns and the radio driver resumes advertising/scanning instead of pumping a dead link.
        self.bridge.link_dead.signal(());
    }
}

/// The one live link over the bridge channels: the control lane carries the handshake, and
/// [`into_data`](BleLink::into_data) splits the data lane into source/sink halves.
struct EmbeddedBleLink {
    control_in: Receiver<'static, BridgeMutex, Control, CTRL_DEPTH>,
    control_out: Sender<'static, BridgeMutex, Control, CTRL_DEPTH>,
    data_in: Receiver<'static, BridgeMutex, FrameBytes, DATA_DEPTH>,
    data_out: Sender<'static, BridgeMutex, FrameBytes, DATA_DEPTH>,
    link_dead: &'static Signal<BridgeMutex, ()>,
    address: [u8; 6],
}

impl BleLink for EmbeddedBleLink {
    type Error = Closed;
    type Source = EmbeddedBleSource;
    type Sink = EmbeddedBleSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        BleAddress::new(self.address)
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

    fn into_data(self) -> (EmbeddedBleSource, EmbeddedBleSink) {
        (
            EmbeddedBleSource {
                data_in: self.data_in,
                link_dead: self.link_dead,
            },
            EmbeddedBleSink {
                data_out: self.data_out,
                link_dead: self.link_dead,
            },
        )
    }
}

struct EmbeddedBleSource {
    data_in: Receiver<'static, BridgeMutex, FrameBytes, DATA_DEPTH>,
    link_dead: &'static Signal<BridgeMutex, ()>,
}

impl BleSource for EmbeddedBleSource {
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

struct EmbeddedBleSink {
    data_out: Sender<'static, BridgeMutex, FrameBytes, DATA_DEPTH>,
    link_dead: &'static Signal<BridgeMutex, ()>,
}

impl BleSink for EmbeddedBleSink {
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

/// trouble-host surfaces scan results through an [`EventHandler`] the runner invokes on every LE
/// advertising report (not a per-scan callback like the nRF). This funnel filters reports to ones
/// carrying our service UUID and pushes each as a [`SeenPeer`] to the bridge for the backend to turn
/// into a brain `Sighting`. `&self`/sync, so it holds a `'static` sender and `try_send`s (drops on a
/// full funnel — the next report re-surfaces a still-present peer).
struct ScanFunnel {
    sightings: Sender<'static, BridgeMutex, SeenPeer, SIGHTING_DEPTH>,
}

impl EventHandler for ScanFunnel {
    fn on_adv_reports(&self, reports: LeAdvReportsIter) {
        for report in reports {
            let Ok(report) = report else { continue };
            if contains_service(report.data) {
                let _ = self.sightings.try_send(SeenPeer {
                    kind: report.addr_kind,
                    addr: report.addr,
                    rssi: report.rssi,
                });
            }
        }
    }
}

/// Serve one accepted peripheral connection (a central dialed us) over the bridge until it drops: the
/// GATT server routes the peer's control/data writes inbound (reassembling data fragments), and the
/// outbound loop fans the supervisor's control/data back as GATT notifications. Signals `connected`
/// once bound so the supervisor settles the link as `Inbound`.
async fn serve_peripheral<'a>(
    connection: &GattConnection<'a, 'a, DefaultPacketPool>,
    bridge: &'static BleBridge,
    control: &Characteristic<GattVec<u8, GATT_VALUE_CAP>>,
    data: &Characteristic<GattVec<u8, GATT_VALUE_CAP>>,
) {
    bridge.clear_lanes();
    bridge.set_peer_addr(connection.raw().peer_address().into_inner());
    let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
    let control_out_rx = bridge.control_out.receiver();
    let data_out_rx = bridge.data_out.receiver();
    let control_in_tx = bridge.control_in.sender();
    let data_in_tx = bridge.data_in.sender();
    bridge.connected.send(()).await;

    loop {
        match select4(
            connection.next(),
            control_out_rx.receive(),
            data_out_rx.receive(),
            bridge.link_dead.wait(),
        )
        .await
        {
            Either4::First(GattConnectionEvent::Disconnected { .. }) => {
                bridge.link_dead.signal(());
                break;
            }
            Either4::First(GattConnectionEvent::Gatt { event }) => {
                if let GattEvent::Write(write) = &event {
                    if write.handle() == control.handle {
                        if let Some(message) = Control::decode(write.data()) {
                            let _ = control_in_tx.try_send(message);
                        }
                    } else if write.handle() == data.handle {
                        if let Some(fragment) = Fragment::decode(write.data()) {
                            if let Some(frame) = reassembler.absorb(&fragment) {
                                let mut bytes = FrameBytes::new();
                                if bytes.extend_from_slice(frame).is_ok() {
                                    let _ = data_in_tx.try_send(bytes);
                                }
                            }
                        }
                    }
                }
                if let Ok(reply) = event.accept() {
                    reply.send().await;
                }
            }
            Either4::First(_) => {}
            Either4::Second(message) => {
                let mut buf = [0u8; CONTROL_MAX_LEN];
                if let Some(len) = message.encode(&mut buf) {
                    let mut value = GattVec::<u8, GATT_VALUE_CAP>::new();
                    let _ = value.extend_from_slice(&buf[..len]);
                    let _ = with_timeout(NOTIFY_TIMEOUT, control.notify(connection, &value)).await;
                }
            }
            Either4::Third(frame) => {
                let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                    let Some(len) = fragment.encode(&mut buf) else {
                        continue;
                    };
                    let mut value = GattVec::<u8, GATT_VALUE_CAP>::new();
                    let _ = value.extend_from_slice(&buf[..len]);
                    match with_timeout(NOTIFY_TIMEOUT, data.notify(connection, &value)).await {
                        Ok(Ok(())) => {}
                        _ => break,
                    }
                    Timer::after(NOTIFY_PACING).await;
                }
            }
            Either4::Fourth(()) => break,
        }
    }
}

/// Dial a peer as a central over the bridge (the central twin of [`serve_peripheral`]): connect
/// (whitelisting the scanned address), discover its [`ReticulumService`] control/data characteristics,
/// subscribe to their notifications, signal `dialed` so the supervisor settles the link as `Dialed`,
/// then pump it until it drops. On a connect/discovery failure the address is reported `dial_failed` so
/// the brain backs off. The `GattClient::task` must run concurrently for notifications to flow.
async fn serve_central<'a, C: Controller>(
    stack: &'a Stack<'a, C, DefaultPacketPool>,
    central: &mut Central<'a, C, DefaultPacketPool>,
    target: DialTarget,
    bridge: &'static BleBridge,
    service_uuid: &Uuid,
    control_uuid: &Uuid,
    data_uuid: &Uuid,
) {
    let bd = target.addr;
    let whitelist = [(target.kind, &bd)];
    let mut config = ConnectConfig {
        scan_config: ScanConfig {
            active: false,
            filter_accept_list: &whitelist,
            ..Default::default()
        },
        connect_params: Default::default(),
    };
    config.scan_config.timeout = CONNECT_TIMEOUT;

    let fail = || {
        let _ = bridge.dial_failed.try_send(bd.into_inner());
    };

    let Ok(connection) = central.connect(&config).await else {
        fail();
        return;
    };

    // Bound the GATT bring-up (MTU exchange, then discovery + subscribe). A peer that accepts the
    // connection but stalls the GATT layer would otherwise hang here with no timeout — before the
    // supervisor is even aware of the link, so its close path can't help — wedging the single radio.
    // The GATT client carries trouble-host's `Notification<512>` pubsub (~1.3 KiB); the peripheral
    // side's equivalent (`AttributeServer`) is a static, but the client is per-dial, so it is boxed
    // onto the heap (esp-alloc falls through to PSRAM) to keep this frame near the peripheral serve
    // loop's instead of making `serve_central` overflow core 0's stack.
    let client = match with_timeout(
        GATT_SETUP_TIMEOUT,
        GattClient::<C, DefaultPacketPool, MAX_SERVICES>::new(stack, &connection),
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
            let services = client.services_by_uuid(service_uuid).await.ok()?;
            let service = services.first()?.clone();
            let control: Characteristic<GattVec<u8, GATT_VALUE_CAP>> = client
                .characteristic_by_uuid(&service, control_uuid)
                .await
                .ok()?;
            let data: Characteristic<GattVec<u8, GATT_VALUE_CAP>> = client
                .characteristic_by_uuid(&service, data_uuid)
                .await
                .ok()?;
            let control_listener = client.subscribe(&control, false).await.ok()?;
            let data_listener = client.subscribe(&data, false).await.ok()?;
            Some((control, data, control_listener, data_listener))
        };
        // Discovery needs the client's rx task running (ATT responses ride it), so race the two.
        match select(discover, client.task()).await {
            Either::First(Some(parts)) => Some(parts),
            _ => None,
        }
    })
    .await;
    let (control, data, mut control_listener, mut data_listener) = match discovered {
        Ok(Some(parts)) => parts,
        _ => {
            fail();
            return;
        }
    };

    bridge.clear_lanes();
    bridge.set_peer_addr(bd.into_inner());
    let mut reassembler = alloc::boxed::Box::new(Reassembler::<GATT_REASSEMBLY_CAP>::new());
    let control_out_rx = bridge.control_out.receiver();
    let data_out_rx = bridge.data_out.receiver();
    let control_in_tx = bridge.control_in.sender();
    let data_in_tx = bridge.data_in.sender();
    bridge.dialed.send(()).await;

    let inbound = async {
        loop {
            match select(control_listener.next(), data_listener.next()).await {
                Either::First(notification) => {
                    if let Some(message) = Control::decode(notification.as_ref()) {
                        let _ = control_in_tx.try_send(message);
                    }
                }
                Either::Second(notification) => {
                    if let Some(fragment) = Fragment::decode(notification.as_ref()) {
                        if let Some(frame) = reassembler.absorb(&fragment) {
                            let mut bytes = FrameBytes::new();
                            if bytes.extend_from_slice(frame).is_ok() {
                                let _ = data_in_tx.try_send(bytes);
                            }
                        }
                    }
                }
            }
        }
    };

    let outbound = async {
        loop {
            match select(control_out_rx.receive(), data_out_rx.receive()).await {
                Either::First(message) => {
                    let mut buf = [0u8; CONTROL_MAX_LEN];
                    if let Some(len) = message.encode(&mut buf) {
                        let _ = with_timeout(
                            NOTIFY_TIMEOUT,
                            client.write_characteristic_without_response(&control, &buf[..len]),
                        )
                        .await;
                    }
                }
                Either::Second(frame) => {
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
                }
            }
        }
    };

    let _ = select4(client.task(), inbound, outbound, bridge.link_dead.wait()).await;
    bridge.link_dead.signal(());
}

/// Stand the native-Bluetooth interface up on the board's BLE controller. Builds trouble's dual-role
/// host (peripheral GATT server + central), bridges it to the [`BluetoothAuto`] supervisor over a
/// `'static` bridge, and joins the HCI host (carrying the scan event handler), the radio driver (the
/// acceptor + the scan/dial loop), and the supervisor on the main executor (core 0's large thread-mode
/// stack — the handshake crypto, GATT-client, and frame buffers need it). The reactor (core 1) commits
/// a frame to `fleet` and signals the cross-core outbound wake; a light relay on core 0's interrupt
/// executor kicks the supervisor. A settled peer (dialed or accepted) joins `fleet` and lights the BLE
/// card. Never returns.
pub async fn run(
    connector: BleConnector<'static>,
    mac: [u8; 6],
    identity: [u8; 16],
    fleet: BleFleet,
    shared: &'static BluetoothAutoShared<BLE_MEMBERS>,
) {
    let controller = ExternalController::<_, HCI_COMMAND_SLOTS>::new(connector);
    /// trouble's host resources (the L2CAP packet pool + connection storage) are multiple KiB; on the
    /// stack they sit at the base of this future's frame, and the deep notify/write path plus a radio
    /// ISR (which runs on the current task's stack) can then overrun core 0's stack. Parked in a
    /// `static` so the frame stays shallow and the radio path keeps its headroom.
    static RESOURCES: StaticCell<HostResources<DefaultPacketPool, CONNECTIONS, L2CAP_CHANNELS>> =
        StaticCell::new();
    let resources = RESOURCES.init(HostResources::new());

    let mut address = mac;
    address[5] |= 0b1100_0000;
    let stack =
        trouble_host::new(controller, resources).set_random_address(Address::random(address));
    let Host {
        mut peripheral,
        mut central,
        mut runner,
        ..
    } = stack.build();

    let mut control_store = [0u8; GATT_VALUE_CAP];
    let mut data_store = [0u8; GATT_VALUE_CAP];
    let mut table: AttributeTable<NoopRawMutex, ATTRIBUTE_TABLE> = AttributeTable::new();
    if let Err(error) = GapConfig::Peripheral(PeripheralConfig {
        name: "Prns",
        appearance: &appearance::UNKNOWN,
    })
    .build(&mut table)
    {
        log::warn!("ble gap config failed: {error}");
        return;
    }
    let props = [
        CharacteristicProp::Write,
        CharacteristicProp::WriteWithoutResponse,
        CharacteristicProp::Notify,
    ];
    let (control, data) = {
        let mut service = table.add_service(Service::new(reticulum_uuid(SERVICE_UUID_LAST)));
        let control = service
            .add_characteristic(
                reticulum_uuid(CONTROL_UUID_LAST),
                &props[..],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                &mut control_store,
            )
            .build();
        let data = service
            .add_characteristic(
                reticulum_uuid(DATA_UUID_LAST),
                &props[..],
                GattVec::<u8, GATT_VALUE_CAP>::new(),
                &mut data_store,
            )
            .build();
        service.build();
        (control, data)
    };
    let server: AttributeServer<
        NoopRawMutex,
        DefaultPacketPool,
        ATTRIBUTE_TABLE,
        CCCD_TABLE,
        CONNECTIONS,
    > = AttributeServer::new(table);

    let mut adv_data = [0u8; MAX_ADVERTISEMENT_LEN];
    let adv_len = encode_advertisement(&mut adv_data).expect("advertisement fits");

    let service_uuid = reticulum_uuid(SERVICE_UUID_LAST);
    let control_uuid = reticulum_uuid(CONTROL_UUID_LAST);
    let data_uuid = reticulum_uuid(DATA_UUID_LAST);

    static BRIDGE: StaticCell<BleBridge> = StaticCell::new();
    let bridge: &'static BleBridge = BRIDGE.init(BleBridge::new());

    let backend = EmbeddedBleBackend {
        bridge,
        connected: bridge.connected.receiver(),
        dialed: bridge.dialed.receiver(),
        dial_failed: bridge.dial_failed.receiver(),
        sightings: bridge.sightings.receiver(),
        dial_request: bridge.dial_request.sender(),
        seen: heapless::Vec::new(),
    };
    let supervisor = BluetoothAuto::new(
        backend,
        BleIdentity::new(identity),
        Endpoint::Esp32(Esp32Host::Esp32),
        LinkCapabilities {
            l2cap: None,
            link_mtu: BLE_HW_MTU as u16,
        },
        shared,
    );

    let funnel = ScanFunnel {
        sightings: bridge.sightings.sender(),
    };

    let host = async {
        loop {
            if let Err(error) = runner.run_with_handler(&funnel).await {
                log::warn!("ble host runner exited: {error:?}");
            }
        }
    };

    // The radio driver: one connection at a time, so advertise (peripheral) and scan (central)
    // time-share the radio in alternating windows, both gated by the brain's `set_advertising` /
    // `set_scanning`. A scan window surfaces sightings to the funnel and serves a `Dial` the brain
    // decides; an advertise window serves an inbound central. Serving a link blocks the loop until it
    // drops (the radio carries no second connection meanwhile), then the brain reopens the radio.
    let radio_driver = async {
        let mut advertising = false;
        let mut scanning = false;
        loop {
            if let Some(state) = bridge.advertise.try_take() {
                advertising = state;
            }
            if let Some(state) = bridge.scan_enabled.try_take() {
                scanning = state;
            }
            if !advertising && !scanning {
                match select(bridge.advertise.wait(), bridge.scan_enabled.wait()).await {
                    Either::First(state) => advertising = state,
                    Either::Second(state) => scanning = state,
                }
                continue;
            }

            if scanning {
                let mut scanner = Scanner::new(central);
                let dialed = match scanner
                    .scan(&ScanConfig {
                        active: false,
                        ..Default::default()
                    })
                    .await
                {
                    Ok(_session) => {
                        match select(bridge.dial_request.receive(), Timer::after(SCAN_WINDOW)).await
                        {
                            Either::First(target) => Some(target),
                            Either::Second(()) => None,
                        }
                    }
                    Err(error) => {
                        log::warn!("ble scan failed: {error:?}");
                        Timer::after(Duration::from_millis(500)).await;
                        None
                    }
                };
                central = scanner.into_inner();
                if let Some(target) = dialed {
                    serve_central(
                        &stack,
                        &mut central,
                        target,
                        bridge,
                        &service_uuid,
                        &control_uuid,
                        &data_uuid,
                    )
                    .await;
                    continue;
                }
            }

            if advertising {
                let advertiser = match peripheral
                    .advertise(
                        &AdvertisementParameters::default(),
                        Advertisement::ConnectableScannableUndirected {
                            adv_data: &adv_data[..adv_len],
                            scan_data: &[],
                        },
                    )
                    .await
                {
                    Ok(advertiser) => advertiser,
                    Err(error) => {
                        log::warn!("ble advertise failed: {error:?}");
                        Timer::after(Duration::from_millis(500)).await;
                        continue;
                    }
                };
                match select(advertiser.accept(), Timer::after(ADV_WINDOW)).await {
                    Either::First(Ok(connection)) => {
                        match connection.with_attribute_server(&server) {
                            Ok(connection) => {
                                serve_peripheral(&connection, bridge, &control, &data).await;
                            }
                            Err(error) => log::warn!("ble attribute server bind failed: {error:?}"),
                        }
                    }
                    Either::First(Err(error)) => log::warn!("ble accept failed: {error:?}"),
                    Either::Second(()) => {}
                }
            }
        }
    };

    let radio = async {
        join(radio_driver, supervisor.run(fleet)).await;
    };

    join(host, radio).await;
}
