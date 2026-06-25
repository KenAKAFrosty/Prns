//! The Heltec V4 (ESP32-S3) native-Bluetooth backend: trouble-host's GATT peripheral bridged to the
//! engine's [`BleBackend`] seam, driven by the embassy [`BluetoothAuto`] supervisor so a settled BLE
//! peer becomes a real engine interface (a fleet member) exactly like the WiFi/USB ones.
//!
//! trouble's `GattConnection` is lifetime-bound to the stack, so it cannot move to a `'static` task.
//! Instead the trouble loop runs as a joined *driver* future that demultiplexes the connection: it
//! routes control-characteristic writes to a control channel and reassembles data-characteristic
//! writes onto a data channel, and drains the seam's outbound channels back onto the two
//! characteristics (control as one PDU, data fragmented to the same scheme the Android backend
//! speaks). The seam ([`EmbeddedBleBackend`]/`Link`/`Source`/`Sink`) reads those stack-local
//! channels, decoupled from the connection's lifetime; link death is a level-triggered [`Signal`]
//! the driver raises on disconnect and clears per connection, so the channels carry only real
//! payloads and reset cleanly. The radio carries one connection at a time, so there is one of each.

use embassy_futures::join::join3;
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as BridgeMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_sync_07::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{with_timeout, Duration, Timer};
use esp_radio::ble::controller::BleConnector;
use heapless_09::Vec as GattVec;
use personal_rns::interfaces::bluetooth_auto::core::{
    encode_advertisement, fragments_of, BleAddress, BleIdentity, Control, Dialect, Endpoint,
    Esp32Host, Fragment, L2capPlan, LinkCapabilities, Reassembler, BLE_HW_MTU,
    BLE_SERVICE_UUID_BYTES, CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN, MAX_ADVERTISEMENT_LEN,
};
use personal_rns::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource,
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

const CONTROL_UUID_LAST: u8 = 0xe7;
const DATA_UUID_LAST: u8 = 0xe8;

/// The GATT data lane's fragmentation, byte-identical to the Android backend so the two interoperate:
/// reassemble inbound writes up to [`GATT_REASSEMBLY_CAP`], fragment outbound frames to
/// [`GATT_FRAGMENT_PAYLOAD`]-byte chunks under the 5-byte fragment header.
const GATT_REASSEMBLY_CAP: usize = 600;
const GATT_FRAGMENT_PAYLOAD: usize = 180;

/// Pace the GATT data fragments so a multi-fragment frame does not blast the controller's TX queue
/// back-to-back: the controller gets a moment to put each fragment on air before the next is queued,
/// keeping the radio stable under sustained announce traffic instead of overrunning it.
const NOTIFY_PACING: Duration = Duration::from_millis(15);
/// A single notify that never resolves must not wedge the driver — and through the shared controller,
/// the whole radio — so each is bounded; on timeout the frame is dropped and the link left to recover
/// rather than blocking forever.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);

/// The bridge channels' depths and frame buffer. Control is lockstep (handshake), so a shallow lane
/// suffices; data buffers a few frames so a slow reactor never stalls the GATT read.
const CTRL_DEPTH: usize = 4;
const DATA_DEPTH: usize = 4;
const FRAME_CAP: usize = BLE_HW_MTU;

const CAPACITY_GATED_ADVERTISING: bool = true;

type FrameBytes = heapless::Vec<u8, FRAME_CAP>;

fn reticulum_uuid(last: u8) -> Uuid {
    let mut bytes = BLE_SERVICE_UUID_BYTES;
    bytes[15] = last;
    Uuid::from(u128::from_be_bytes(bytes))
}

/// The seam's error: the link is gone (the central disconnected, or the bridge frame would not fit).
#[derive(Debug)]
struct Closed;

/// The `'static` bridge between the main-executor driver and the interrupt-executor supervisor. A
/// `CriticalSectionRawMutex` guards each lane because the two halves run on different executors.
struct BleBridge {
    connected: Channel<BridgeMutex, (), 2>,
    control_in: Channel<BridgeMutex, Control, CTRL_DEPTH>,
    control_out: Channel<BridgeMutex, Control, CTRL_DEPTH>,
    data_in: Channel<BridgeMutex, FrameBytes, DATA_DEPTH>,
    data_out: Channel<BridgeMutex, FrameBytes, DATA_DEPTH>,
    link_dead: Signal<BridgeMutex, ()>,
    advertise: Signal<BridgeMutex, ()>,
}

impl BleBridge {
    const fn new() -> Self {
        Self {
            connected: Channel::new(),
            control_in: Channel::new(),
            control_out: Channel::new(),
            data_in: Channel::new(),
            data_out: Channel::new(),
            link_dead: Signal::new(),
            advertise: Signal::new(),
        }
    }
}

/// The trouble→seam bridge as a [`BleBackend`]: it surfaces one `Inbound` link per central the driver
/// accepts, the link reading/writing the `'static` bridge channels. One connection at a time.
struct EmbeddedBleBackend<'a> {
    connected: Receiver<'a, BridgeMutex, (), 2>,
    control_in: Receiver<'a, BridgeMutex, Control, CTRL_DEPTH>,
    control_out: Sender<'a, BridgeMutex, Control, CTRL_DEPTH>,
    data_in: Receiver<'a, BridgeMutex, FrameBytes, DATA_DEPTH>,
    data_out: Sender<'a, BridgeMutex, FrameBytes, DATA_DEPTH>,
    link_dead: &'a Signal<BridgeMutex, ()>,
    advertise: &'a Signal<BridgeMutex, ()>,
}

impl<'a> BleBackend for EmbeddedBleBackend<'a> {
    const MAX_PEERS: usize = 1;
    type Error = Closed;
    type Link = EmbeddedBleLink<'a>;

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), Closed> {
        if enabled {
            self.advertise.signal(());
        } else {
            self.advertise.reset();
        }
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<EmbeddedBleLink<'a>> {
        self.connected.receive().await;
        BleEvent::Inbound(EmbeddedBleLink {
            control_in: self.control_in,
            control_out: self.control_out,
            data_in: self.data_in,
            data_out: self.data_out,
            link_dead: self.link_dead,
        })
    }

    async fn dial(&mut self, _address: BleAddress) {}
}

/// One accepted central's link over the bridge channels: the control lane carries the handshake, and
/// [`into_data`](BleLink::into_data) splits the data lane into its source/sink halves.
struct EmbeddedBleLink<'a> {
    control_in: Receiver<'a, BridgeMutex, Control, CTRL_DEPTH>,
    control_out: Sender<'a, BridgeMutex, Control, CTRL_DEPTH>,
    data_in: Receiver<'a, BridgeMutex, FrameBytes, DATA_DEPTH>,
    data_out: Sender<'a, BridgeMutex, FrameBytes, DATA_DEPTH>,
    link_dead: &'a Signal<BridgeMutex, ()>,
}

impl<'a> BleLink for EmbeddedBleLink<'a> {
    type Error = Closed;
    type Source = EmbeddedBleSource<'a>;
    type Sink = EmbeddedBleSink<'a>;

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

    fn into_data(self) -> (EmbeddedBleSource<'a>, EmbeddedBleSink<'a>) {
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

struct EmbeddedBleSource<'a> {
    data_in: Receiver<'a, BridgeMutex, FrameBytes, DATA_DEPTH>,
    link_dead: &'a Signal<BridgeMutex, ()>,
}

impl<'a> BleSource for EmbeddedBleSource<'a> {
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

struct EmbeddedBleSink<'a> {
    data_out: Sender<'a, BridgeMutex, FrameBytes, DATA_DEPTH>,
    link_dead: &'a Signal<BridgeMutex, ()>,
}

impl<'a> BleSink for EmbeddedBleSink<'a> {
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

/// Stand the native-Bluetooth interface up on the board's BLE controller: build trouble's GATT
/// peripheral, bridge it to the [`BluetoothAuto`] supervisor over a `'static` bridge, and run the
/// HCI host, the connection driver, and the supervisor all joined on the main executor (core 0's
/// large thread-mode stack — the supervisor's handshake crypto and frame buffers need it). The
/// reactor (core 1) commits a frame to `fleet` and signals the cross-core outbound wake; that wake
/// is caught by a light relay on core 0's interrupt executor, which kicks this supervisor with an
/// ordinary same-core wake (see `heltec_v4`). A settled peer joins `fleet` and lights `shared`'s BLE
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
    /// stack they sit at the base of this future's frame, and the deep `data.notify` path plus a radio
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
        let mut service = table.add_service(Service::new(reticulum_uuid(0xe3)));
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

    static BRIDGE: StaticCell<BleBridge> = StaticCell::new();
    let bridge: &'static BleBridge = BRIDGE.init(BleBridge::new());

    let backend = EmbeddedBleBackend {
        connected: bridge.connected.receiver(),
        control_in: bridge.control_in.receiver(),
        control_out: bridge.control_out.sender(),
        data_in: bridge.data_in.receiver(),
        data_out: bridge.data_out.sender(),
        link_dead: &bridge.link_dead,
        advertise: &bridge.advertise,
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

    let connected_tx = bridge.connected.sender();
    let control_in_tx = bridge.control_in.sender();
    let control_out_rx = bridge.control_out.receiver();
    let data_in_tx = bridge.data_in.sender();
    let data_out_rx = bridge.data_out.receiver();

    let host = async {
        loop {
            if let Err(error) = runner.run().await {
                log::warn!("ble host runner exited: {error:?}");
            }
        }
    };

    let driver = async {
        loop {
            if CAPACITY_GATED_ADVERTISING {
                bridge.advertise.wait().await;
            }
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
                    continue;
                }
            };
            log::info!("ble advertising connectable, awaiting a central");
            let connection = match advertiser.accept().await {
                Ok(connection) => connection,
                Err(error) => {
                    log::warn!("ble accept failed: {error:?}");
                    continue;
                }
            };
            let connection = match connection.with_attribute_server(&server) {
                Ok(connection) => connection,
                Err(error) => {
                    log::warn!("ble attribute server bind failed: {error:?}");
                    continue;
                }
            };
            log::info!("ble central connected");
            bridge.link_dead.reset();
            bridge.control_in.clear();
            bridge.data_in.clear();
            bridge.control_out.clear();
            bridge.data_out.clear();
            let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
            connected_tx.send(()).await;

            loop {
                match select3(
                    connection.next(),
                    control_out_rx.receive(),
                    data_out_rx.receive(),
                )
                .await
                {
                    Either3::First(GattConnectionEvent::Disconnected { reason }) => {
                        log::info!("ble central disconnected: {reason:?}");
                        bridge.link_dead.signal(());
                        break;
                    }
                    Either3::First(GattConnectionEvent::Gatt { event }) => {
                        if let GattEvent::Write(write) = &event {
                            if write.handle() == control.handle {
                                match Control::decode(write.data()) {
                                    Some(message) => {
                                        let _ = control_in_tx.try_send(message);
                                    }
                                    None => log::warn!(
                                        "ble control write undecodable ({} bytes)",
                                        write.data().len()
                                    ),
                                }
                            } else if write.handle() == data.handle {
                                if let Some(fragment) = Fragment::decode(write.data()) {
                                    if let Some(frame) = reassembler.absorb(&fragment) {
                                        log::info!("ble data in ({} bytes)", frame.len());
                                        let mut bytes = FrameBytes::new();
                                        if bytes.extend_from_slice(frame).is_ok() {
                                            let _ = data_in_tx.try_send(bytes);
                                        }
                                    }
                                }
                            }
                        }
                        match event.accept() {
                            Ok(reply) => reply.send().await,
                            Err(error) => log::warn!("ble gatt reply failed: {error:?}"),
                        }
                    }
                    Either3::First(_) => {}
                    Either3::Second(message) => {
                        let mut buf = [0u8; CONTROL_MAX_LEN];
                        if let Some(len) = message.encode(&mut buf) {
                            let mut value = GattVec::<u8, GATT_VALUE_CAP>::new();
                            let _ = value.extend_from_slice(&buf[..len]);
                            match with_timeout(NOTIFY_TIMEOUT, control.notify(&connection, &value))
                                .await
                            {
                                Ok(Ok(())) => log::info!("ble control notified ({len} bytes)"),
                                Ok(Err(error)) => {
                                    log::warn!("ble control notify failed: {error:?}")
                                }
                                Err(_) => log::warn!("ble control notify timed out"),
                            }
                        }
                    }
                    Either3::Third(frame) => {
                        log::info!("ble data out ({} bytes)", frame.len());
                        let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                        for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                            let Some(len) = fragment.encode(&mut buf) else {
                                continue;
                            };
                            let mut value = GattVec::<u8, GATT_VALUE_CAP>::new();
                            let _ = value.extend_from_slice(&buf[..len]);
                            match with_timeout(NOTIFY_TIMEOUT, data.notify(&connection, &value))
                                .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(error)) => {
                                    log::warn!("ble data notify failed: {error:?}");
                                    break;
                                }
                                Err(_) => {
                                    log::warn!("ble data notify timed out");
                                    break;
                                }
                            }
                            Timer::after(NOTIFY_PACING).await;
                        }
                    }
                }
            }
        }
    };

    join3(host, driver, supervisor.run(fleet)).await;
}
