//! The Heltec V4 / T-Beam (ESP32-S3) native-Bluetooth backend: trouble-host bridged to the engine's
//! [`BleBackend`] seam, driven by the embassy [`BluetoothAuto`] supervisor so settled BLE peers become
//! real engine interfaces (fleet members) exactly like the WiFi/USB ones. Dual-role *and* multi-peer:
//! the board both **advertises** a GATT server (a central dials us → `Inbound`) AND **scans + dials**
//! as a central (we find a peer advertising our service → `LinkReady{Dialed}`), and it carries up to
//! [`SLOTS`] concurrent peers.
//!
//! Concurrency model (mirrors the nRF T-Echo, adapted to trouble-host): the host `Stack` is parked in
//! a `static` so its `Connection`s are `'static` and can move through channels. A pool of role-agnostic
//! per-slot channel sets (the [`BleHub`]) bridges the radio to the supervisor. One *acceptor* owns the
//! peripheral (advertises, accepts), one *dialer* owns the central (scans, connects); each reserves a
//! free slot and hands its `Connection` to that slot's worker. [`SLOTS`] *slot workers* each serve one
//! connection — a peripheral GATT server (accepted) or a GATT client (dialed) — over their slot's
//! channels, all concurrently. A settled peer joins `fleet` and lights the BLE card; link death is a
//! per-slot level-triggered [`Signal`] so a rejected/failed link releases its slot back to the pool.

use core::cell::Cell;

use embassy_futures::join::join;
use embassy_futures::select::{select, select3, select4, Either, Either3, Either4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as BridgeMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_sync_07::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{with_timeout, Duration, Timer};
use esp_radio::ble::controller::BleConnector;
use heapless_09::Vec as GattVec;

use personal_rns::interfaces::bluetooth_auto::core::{
    contains_service, encode_advertisement, fragments_of, BleAddress, BleIdentity, Control, Dialect,
    Endpoint, Esp32Host, Fragment, L2capPlan, LinkCapabilities, Reassembler, BLE_HW_MTU,
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

/// The concurrent-peer pool size: one connection-slot per peer the radio carries at once. Equal to the
/// supervisor's member ceiling. `CONNECTIONS`/`L2CAP_CHANNELS` (the trouble-host host resources) and
/// the slot-worker `join` below are sized to this.
const SLOTS: usize = BLE_MEMBERS;
const _: () = assert!(SLOTS == 2, "the slot-worker join in `run` is hand-unrolled for SLOTS == 2");

const HCI_COMMAND_SLOTS: usize = 20;
const CONNECTIONS: usize = SLOTS;
const L2CAP_CHANNELS: usize = 2 * SLOTS;
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
/// back-to-back: the controller gets a moment to put each fragment on air before the next is queued.
const NOTIFY_PACING: Duration = Duration::from_millis(15);
/// A single notify/write that never resolves must not wedge a slot's serve loop, so each is bounded.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);
/// A dial scans for its whitelisted peer before connecting; `connect` with a zero scan timeout scans
/// forever, so bound it — on timeout the connect errors, the slot frees, and the brain backs off.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
/// A dialed peer that connects but stalls the GATT bring-up (MTU exchange / discovery / subscribe) must
/// not hold its slot forever, so the whole bring-up is bounded.
const GATT_SETUP_TIMEOUT: Duration = Duration::from_secs(6);

/// The radio time-shares advertising (peripheral) and scanning (central) in alternating windows rather
/// than running both at once — keeping one serve frame per active role off the deepest path and
/// sidestepping any controller limit on simultaneous advertise+scan. Two boards alternating overlap
/// within a cycle, so discovery converges; a `Dial` decided during an off-window is buffered.
const ADV_WINDOW: Duration = Duration::from_millis(600);
const SCAN_WINDOW: Duration = Duration::from_millis(600);

/// Per-slot bridge channel depths. Control is lockstep (handshake), so a shallow lane suffices; data
/// buffers a few frames so a slow reactor never stalls the GATT read.
const CTRL_DEPTH: usize = 4;
const DATA_DEPTH: usize = 4;
const SIGHTING_DEPTH: usize = 4;
/// Recently-scanned peers, kept so [`dial`](EmbeddedBleBackend::dial) (which the brain calls with only
/// the 6 address bytes) can recover the full `(AddrKind, BdAddr)` the central must whitelist to connect.
const SEEN_CAP: usize = 8;
const FRAME_CAP: usize = BLE_HW_MTU;

type FrameBytes = heapless::Vec<u8, FRAME_CAP>;
type Controller = ExternalController<BleConnector<'static>, HCI_COMMAND_SLOTS>;
type HostStack = Stack<'static, Controller, DefaultPacketPool>;

fn reticulum_uuid(last: u8) -> Uuid {
    let mut bytes = BLE_SERVICE_UUID_BYTES;
    bytes[15] = last;
    Uuid::from(u128::from_be_bytes(bytes))
}

/// The seam's error: the link is gone (the peer disconnected, or the bridge frame would not fit).
#[derive(Debug)]
struct Closed;

/// A peer the scanner saw advertising our service: the full `(AddrKind, BdAddr)` (so the dialer
/// whitelists it exactly) and the report RSSI.
#[derive(Clone, Copy)]
struct SeenPeer {
    kind: AddrKind,
    addr: BdAddr,
    rssi: i8,
}

/// The full radio address the central must whitelist to dial a peer, carried from a sighting through
/// the brain's `Dial` back to the dialer.
#[derive(Clone, Copy)]
struct DialTarget {
    kind: AddrKind,
    addr: BdAddr,
}

/// The work handed to a free slot's worker: a connection the acceptor accepted (we are its GATT
/// server) or one the dialer opened (we are its GATT client). Both are `'static` because the host
/// `Stack` is parked in a `static`, so they ride a channel to the worker.
enum SlotJob {
    Accept(Connection<'static, DefaultPacketPool>),
    Dial(Connection<'static, DefaultPacketPool>),
}

/// One slot's `'static` bridge between its worker (the trouble-host GATT side) and the supervisor's
/// [`EmbeddedBleLink`]. Role-agnostic: a peripheral serve loop or a central serve loop pumps the same
/// four lanes; `link_dead` tears the supervisor's halves down when the connection drops; `peer_addr`
/// is the connected address so the brain keys this peer correctly.
struct SlotChannels {
    control_in: Channel<BridgeMutex, Control, CTRL_DEPTH>,
    control_out: Channel<BridgeMutex, Control, CTRL_DEPTH>,
    data_in: Channel<BridgeMutex, FrameBytes, DATA_DEPTH>,
    data_out: Channel<BridgeMutex, FrameBytes, DATA_DEPTH>,
    link_dead: Signal<BridgeMutex, ()>,
    peer_addr: BlockingMutex<BridgeMutex, Cell<[u8; 6]>>,
}

impl SlotChannels {
    const fn new() -> Self {
        Self {
            control_in: Channel::new(),
            control_out: Channel::new(),
            data_in: Channel::new(),
            data_out: Channel::new(),
            link_dead: Signal::new(),
            peer_addr: BlockingMutex::new(Cell::new([0u8; 6])),
        }
    }

    fn set_peer_addr(&self, bytes: [u8; 6]) {
        self.peer_addr.lock(|cell| cell.set(bytes));
    }

    fn addr(&self) -> [u8; 6] {
        self.peer_addr.lock(|cell| cell.get())
    }

    fn clear_lanes(&self) {
        self.link_dead.reset();
        self.control_in.clear();
        self.control_out.clear();
        self.data_in.clear();
        self.data_out.clear();
    }
}

/// The shared hub the whole BLE plane coordinates through: a pool of role-agnostic [`SlotChannels`],
/// the `assign`/`free`/`connected`/`dialed` plumbing that hands each new connection to an idle slot and
/// tells the supervisor which slot lit up (and how), the scanner's sighting funnel + dial requests, and
/// the brain's advertise/scan gates. One `static`, so the slot workers, the acceptor, the dialer, the
/// scan event handler, and the supervisor all reference the same channels.
struct BleHub {
    slots: [SlotChannels; SLOTS],
    assign: [Channel<BridgeMutex, SlotJob, 1>; SLOTS],
    free: Channel<BridgeMutex, usize, SLOTS>,
    connected: Channel<BridgeMutex, usize, SLOTS>,
    dialed: Channel<BridgeMutex, usize, SLOTS>,
    dial_failed: Channel<BridgeMutex, [u8; 6], SLOTS>,
    sightings: Channel<BridgeMutex, SeenPeer, SIGHTING_DEPTH>,
    dial_request: Channel<BridgeMutex, DialTarget, SLOTS>,
    advertise: Signal<BridgeMutex, bool>,
    scan_enabled: Signal<BridgeMutex, bool>,
}

impl BleHub {
    const fn new() -> Self {
        Self {
            slots: [const { SlotChannels::new() }; SLOTS],
            assign: [const { Channel::new() }; SLOTS],
            free: Channel::new(),
            connected: Channel::new(),
            dialed: Channel::new(),
            dial_failed: Channel::new(),
            sightings: Channel::new(),
            dial_request: Channel::new(),
            advertise: Signal::new(),
            scan_enabled: Signal::new(),
        }
    }
}

/// The trouble→seam bridge as a [`BleBackend`]: it surfaces each slot's live link (whichever role won
/// it) reading/writing that slot's `'static` channels, the scanner's sightings, and dial failures.
struct EmbeddedBleBackend {
    hub: &'static BleHub,
    connected: Receiver<'static, BridgeMutex, usize, SLOTS>,
    dialed: Receiver<'static, BridgeMutex, usize, SLOTS>,
    dial_failed: Receiver<'static, BridgeMutex, [u8; 6], SLOTS>,
    sightings: Receiver<'static, BridgeMutex, SeenPeer, SIGHTING_DEPTH>,
    dial_request: Sender<'static, BridgeMutex, DialTarget, SLOTS>,
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

    fn link(&self, slot: usize) -> EmbeddedBleLink {
        let s = &self.hub.slots[slot];
        EmbeddedBleLink {
            control_in: s.control_in.receiver(),
            control_out: s.control_out.sender(),
            data_in: s.data_in.receiver(),
            data_out: s.data_out.sender(),
            link_dead: &s.link_dead,
            address: s.addr(),
        }
    }
}

impl BleBackend for EmbeddedBleBackend {
    const MAX_PEERS: usize = SLOTS;
    type Error = Closed;
    type Link = EmbeddedBleLink;

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), Closed> {
        self.hub.advertise.signal(enabled);
        Ok(())
    }

    async fn set_scanning(&mut self, enabled: bool) -> Result<(), Closed> {
        self.hub.scan_enabled.signal(enabled);
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
            Either4::First(slot) => BleEvent::Inbound(self.link(slot)),
            Either4::Second(slot) => BleEvent::LinkReady {
                link: self.link(slot),
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

    async fn on_link_closed(&mut self, address: BleAddress) {
        // The supervisor rejected/closed this peer (handshake timeout/abort, keeper-duel loss, or a
        // settled member dropping). Raise the matching slot's link_dead so its serve loop returns and
        // the slot rejoins the free pool, instead of pumping a dead link.
        for slot in &self.hub.slots {
            if slot.addr() == *address.octets() {
                slot.link_dead.signal(());
            }
        }
    }
}

/// One slot's live link over its bridge channels: the control lane carries the handshake, and
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
/// advertising report. This funnel filters reports to ones carrying our service UUID and pushes each as
/// a [`SeenPeer`] to the hub for the backend to turn into a brain `Sighting`. `&self`/sync, so it holds
/// a `'static` sender and `try_send`s (drops on a full funnel — the next report re-surfaces the peer).
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

/// Serve one accepted peripheral connection over its slot until it drops: the GATT server routes the
/// peer's control/data writes inbound (reassembling data fragments), and the outbound loop fans the
/// supervisor's control/data back as GATT notifications. Honors `link_dead` so a supervisor-side close
/// (handshake reject) returns even if the peer stays connected.
async fn serve_peripheral(
    slot: &'static SlotChannels,
    connection: &GattConnection<'_, '_, DefaultPacketPool>,
    control: &Characteristic<GattVec<u8, GATT_VALUE_CAP>>,
    data: &Characteristic<GattVec<u8, GATT_VALUE_CAP>>,
) {
    let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
    let control_out_rx = slot.control_out.receiver();
    let data_out_rx = slot.data_out.receiver();
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();

    loop {
        match select4(
            connection.next(),
            control_out_rx.receive(),
            data_out_rx.receive(),
            slot.link_dead.wait(),
        )
        .await
        {
            Either4::First(GattConnectionEvent::Disconnected { .. }) => break,
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
                    let Some(len) = fragment.encode(&mut buf) else { continue };
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

/// Serve one dialed central connection over its slot (the central twin of [`serve_peripheral`]):
/// discover the peer's [`ReticulumService`] control/data characteristics, subscribe to their
/// notifications, signal `dialed` so the supervisor settles the link as `Dialed`, then pump it until it
/// drops. The GATT client carries trouble-host's `Notification<512>` pubsub (~1.3 KiB); the peripheral
/// side's equivalent (`AttributeServer`) is a `static`, but the client is per-dial, so it is boxed onto
/// the heap (esp-alloc falls through to PSRAM) to keep this frame shallow. On a discovery failure the
/// peer is reported `dial_failed` so the brain backs off.
async fn serve_central(
    idx: usize,
    hub: &'static BleHub,
    stack: &'static HostStack,
    connection: Connection<'static, DefaultPacketPool>,
    service_uuid: &Uuid,
    control_uuid: &Uuid,
    data_uuid: &Uuid,
) {
    let slot = &hub.slots[idx];
    let addr = connection.peer_address().into_inner();
    let fail = || {
        let _ = hub.dial_failed.try_send(addr);
    };

    let client = match with_timeout(
        GATT_SETUP_TIMEOUT,
        GattClient::<Controller, DefaultPacketPool, MAX_SERVICES>::new(stack, &connection),
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

    slot.set_peer_addr(addr);
    let mut reassembler = alloc::boxed::Box::new(Reassembler::<GATT_REASSEMBLY_CAP>::new());
    let control_out_rx = slot.control_out.receiver();
    let data_out_rx = slot.data_out.receiver();
    let control_in_tx = slot.control_in.sender();
    let data_in_tx = slot.data_in.sender();
    hub.dialed.send(idx).await;

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
                        let Some(len) = fragment.encode(&mut buf) else { continue };
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

    let _ = select4(
        client.task(),
        inbound,
        outbound,
        slot.link_dead.wait(),
    )
    .await;
}

/// One pool slot's worker: park until the acceptor or the dialer hands it a connection, serve it in
/// whichever role the job names over this slot's channels, then signal `link_dead` and return the slot
/// to the free list. [`SLOTS`] of these run concurrently — the inline twin of the desktop supervisor's
/// per-connection tasks (inline because trouble-host's `GattConnection`/`GattClient` are stack-bound).
async fn serve_slot(
    idx: usize,
    hub: &'static BleHub,
    stack: &'static HostStack,
    server: &AttributeServer<
        '_,
        NoopRawMutex,
        DefaultPacketPool,
        ATTRIBUTE_TABLE,
        CCCD_TABLE,
        CONNECTIONS,
    >,
    control: &Characteristic<GattVec<u8, GATT_VALUE_CAP>>,
    data: &Characteristic<GattVec<u8, GATT_VALUE_CAP>>,
    service_uuid: &Uuid,
    control_uuid: &Uuid,
    data_uuid: &Uuid,
) {
    let slot = &hub.slots[idx];
    loop {
        let job = hub.assign[idx].receive().await;
        slot.clear_lanes();
        match job {
            SlotJob::Accept(connection) => {
                slot.set_peer_addr(connection.peer_address().into_inner());
                match connection.with_attribute_server(server) {
                    Ok(connection) => {
                        hub.connected.send(idx).await;
                        serve_peripheral(slot, &connection, control, data).await;
                    }
                    Err(error) => log::warn!("ble attribute server bind failed: {error:?}"),
                }
            }
            SlotJob::Dial(connection) => {
                serve_central(
                    idx,
                    hub,
                    stack,
                    connection,
                    service_uuid,
                    control_uuid,
                    data_uuid,
                )
                .await;
            }
        }
        slot.link_dead.signal(());
        let _ = hub.free.try_send(idx);
    }
}

/// Advertise (gated by the brain's `set_advertising`) and hand each accepted central to a free slot —
/// the one place that drives the single advertising set. Reserves a free slot, advertises into it,
/// hands the connection to that slot's worker, loops to fill the next. Time-shared with the scanner via
/// alternating windows; a mid-advertise disable drops the pending advertise and releases the slot.
async fn acceptor(
    hub: &'static BleHub,
    peripheral: &mut Peripheral<'static, Controller, DefaultPacketPool>,
    adv_data: &[u8],
) {
    let mut enabled = false;
    loop {
        if !enabled {
            enabled = hub.advertise.wait().await;
            continue;
        }
        let idx = match select(hub.free.receive(), hub.advertise.wait()).await {
            Either::First(idx) => idx,
            Either::Second(state) => {
                enabled = state;
                continue;
            }
        };
        let advertiser = match peripheral
            .advertise(
                &AdvertisementParameters::default(),
                Advertisement::ConnectableScannableUndirected {
                    adv_data,
                    scan_data: &[],
                },
            )
            .await
        {
            Ok(advertiser) => advertiser,
            Err(error) => {
                log::warn!("ble advertise failed: {error:?}");
                let _ = hub.free.try_send(idx);
                Timer::after(Duration::from_millis(500)).await;
                continue;
            }
        };
        match select3(
            advertiser.accept(),
            Timer::after(ADV_WINDOW),
            hub.advertise.wait(),
        )
        .await
        {
            Either3::First(Ok(connection)) => {
                if hub.assign[idx].try_send(SlotJob::Accept(connection)).is_err() {
                    let _ = hub.free.try_send(idx);
                }
            }
            Either3::First(Err(error)) => {
                log::warn!("ble accept failed: {error:?}");
                let _ = hub.free.try_send(idx);
            }
            Either3::Second(()) => {
                let _ = hub.free.try_send(idx);
            }
            Either3::Third(state) => {
                enabled = state;
                let _ = hub.free.try_send(idx);
            }
        }
    }
}

/// Scan (gated by the brain's `set_scanning`) so sightings flow to the funnel, and on a brain `Dial`
/// stop scanning, connect, and hand the dialed connection to a free slot. The `Scanner` owns the
/// `Central` while scanning; `into_inner` reclaims it to connect — one scan-or-connect at a time.
async fn dialer(
    hub: &'static BleHub,
    mut central: Central<'static, Controller, DefaultPacketPool>,
) {
    let mut enabled = false;
    loop {
        if !enabled {
            enabled = hub.scan_enabled.wait().await;
            continue;
        }
        let mut scanner = Scanner::new(central);
        let target = {
            match scanner
                .scan(&ScanConfig {
                    active: false,
                    ..Default::default()
                })
                .await
            {
                Ok(_session) => {
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
                            None
                        }
                    }
                }
                Err(error) => {
                    log::warn!("ble scan failed: {error:?}");
                    Timer::after(Duration::from_millis(500)).await;
                    None
                }
            }
        };
        central = scanner.into_inner();
        if let Some(target) = target {
            let Ok(idx) = hub.free.try_receive() else {
                continue;
            };
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
            match central.connect(&config).await {
                Ok(connection) => {
                    if hub.assign[idx].try_send(SlotJob::Dial(connection)).is_err() {
                        let _ = hub.free.try_send(idx);
                    }
                }
                Err(_) => {
                    let _ = hub.free.try_send(idx);
                    let _ = hub.dial_failed.try_send(bd.into_inner());
                }
            }
        }
    }
}

/// Stand the native-Bluetooth interface up on the board's BLE controller. Builds trouble's dual-role
/// host (peripheral GATT server + central), parks it in a `static` so connections are `'static`, and
/// joins the HCI host (carrying the scan handler), the acceptor, the dialer, [`SLOTS`] slot workers,
/// and the supervisor on the main executor (core 0's large thread-mode stack). A settled peer joins
/// `fleet` and lights the BLE card. Never returns.
pub async fn run(
    connector: BleConnector<'static>,
    mac: [u8; 6],
    identity: [u8; 16],
    fleet: BleFleet,
    shared: &'static BluetoothAutoShared<BLE_MEMBERS>,
) {
    let controller = ExternalController::<_, HCI_COMMAND_SLOTS>::new(connector);
    static RESOURCES: StaticCell<HostResources<DefaultPacketPool, CONNECTIONS, L2CAP_CHANNELS>> =
        StaticCell::new();
    let resources = RESOURCES.init(HostResources::new());

    let mut address = mac;
    address[5] |= 0b1100_0000;
    // The host stack is parked in a `static` so its `Connection`s are `'static` and can ride the hub's
    // assign channels from the acceptor/dialer to a slot worker (trouble-host's own objects are
    // otherwise lifetime-bound to the stack).
    static STACK: StaticCell<HostStack> = StaticCell::new();
    let stack: &'static HostStack =
        STACK.init(trouble_host::new(controller, resources).set_random_address(Address::random(address)));
    let Host {
        mut peripheral,
        central,
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

    static HUB: StaticCell<BleHub> = StaticCell::new();
    let hub: &'static BleHub = HUB.init(BleHub::new());
    for idx in 0..SLOTS {
        let _ = hub.free.try_send(idx);
    }

    let backend = EmbeddedBleBackend {
        hub,
        connected: hub.connected.receiver(),
        dialed: hub.dialed.receiver(),
        dial_failed: hub.dial_failed.receiver(),
        sightings: hub.sightings.receiver(),
        dial_request: hub.dial_request.sender(),
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
        sightings: hub.sightings.sender(),
    };

    let host = async {
        loop {
            if let Err(error) = runner.run_with_handler(&funnel).await {
                log::warn!("ble host runner exited: {error:?}");
            }
        }
    };

    let workers = join(
        serve_slot(
            0,
            hub,
            stack,
            &server,
            &control,
            &data,
            &service_uuid,
            &control_uuid,
            &data_uuid,
        ),
        serve_slot(
            1,
            hub,
            stack,
            &server,
            &control,
            &data,
            &service_uuid,
            &control_uuid,
            &data_uuid,
        ),
    );
    let radio = join(acceptor(hub, &mut peripheral, &adv_data[..adv_len]), dialer(hub, central));
    let plane = join(radio, join(workers, supervisor.run(fleet)));
    join(host, plane).await;
}
