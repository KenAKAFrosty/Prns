mod board;
mod entropy;
mod firmware;

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Delay, Duration, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::gpio::{Input, Output};
use esp_hal::peripherals::BT;
use esp_hal::rng::TrngSource;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::spi::master::Spi;
use esp_hal::uart::{UartRx, UartTx};
use esp_hal::Async;
use personal_rns::bluetooth_auto::BluetoothAutoShared;
use personal_rns::engine::IssuedCommand;
use personal_rns::interfaces::bluetooth_auto::{BleIdentity, BLE_HW_MTU};
use personal_rns::interfaces::lora::{LORA_MAX_PAYLOAD, US915_AUTO_LORA_PROFILE};
use personal_rns::interfaces::usb_auto::device_descriptor;
use personal_rns::interfaces::{BitrateBps, ConnectionState, InterfaceId, InterfaceKind};
use personal_rns::lora::{LoRaControl, LoRaInterface};
use personal_rns::manifold::embassy::{
    EmbassyHost, EmbassyInterfaceSeam, EmbassyInterfaceStatus, InterfaceLifecycle,
};
use personal_rns::manifold::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
use personal_rns::radios::sx126x::Sx126x;
use personal_rns::runtime::{
    minimum_interface_store_capacity, minimum_manifold_notification_capacity, CompletionPool,
    EmbassyInterfaceStore, Fleet, ManifoldLaneSet, PrnsEvent, PrnsNode, StaticManifoldLane,
};
use personal_rns::usb_auto::{ProtocolHostPresence, UsbAutoDevice, UsbAutoDeviceInput};
use prns_interfaces_embassy::bluetooth_auto::PEER_CAPACITY as EMBEDDED_BLE_PEER_CAPACITY;
use static_cell::StaticCell;

use crate::storage::InternalStorage;
use entropy::S3Fn8EntropySource;

esp_app_desc!();

const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"wslv3usb");
const USB_UART_BAUD: u32 = 115_200;
const USB_UART_DATA_BITS_PER_FRAME: u64 = 8;
const USB_UART_WIRE_BITS_PER_FRAME: u64 = 10;
const USB_UART_PAYLOAD_BITRATE_BPS: BitrateBps = BitrateBps::guess(
    USB_UART_BAUD as u64 * USB_UART_DATA_BITS_PER_FRAME / USB_UART_WIRE_BITS_PER_FRAME,
);
const USB_LANE: usize = 1;
const LORA_LANE: usize = 1;
const BLE_LANE: usize = 1;
const LANE_COUNT: usize = USB_LANE + LORA_LANE + BLE_LANE;
const LANE_DEPTH: usize = 1;
const OUTBOUND_BURST_DEPTH: usize = InternalStorage::MAX_OUTGOING_RESOURCE_REACTION_FRAMES;
pub const BLE_PEER_CAPACITY: usize = EMBEDDED_BLE_PEER_CAPACITY;
const INTERFACE_CAPACITY: usize = LANE_COUNT + BLE_PEER_CAPACITY + 1;
pub const NOTIFY_CAP: usize = minimum_manifold_notification_capacity(LANE_COUNT, LANE_DEPTH);
const COMMANDS_CAP: usize = 8;
pub const LIFECYCLE_CAP: usize = 8;
const COMPLETIONS_CAP: usize = 4;
const INTERFACE_STORE_CAP: usize = minimum_interface_store_capacity(INTERFACE_CAPACITY);
const PACKET_PHY_RETENTION_CAPACITY: usize = 32;
const PACKET_PHY_INDEX_BUCKETS: usize =
    personal_rns::routing::dedup::dedup_index_buckets(PACKET_PHY_RETENTION_CAPACITY);
const BLE_START_DELAY: Duration = Duration::from_secs(3);
const BLE_SUPERVISOR_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);
const _: () = assert!(InternalStorage::LINK_SESSIONS > BLE_PEER_CAPACITY);

type Mtx = CriticalSectionRawMutex;
type LoraRadio = Sx126x<
    ExclusiveDevice<Spi<'static, Async>, Output<'static>, Delay>,
    Input<'static>,
    Input<'static>,
    Output<'static>,
    Delay,
>;
type UsbSeam =
    EmbassyInterfaceSeam<'static, Mtx, S3Fn8EntropySource, NOTIFY_CAP, EMBEDDED_MAX_WIRE_FRAME_LEN>;
type BleFleet = Fleet<Mtx, BLE_HW_MTU, NOTIFY_CAP, LIFECYCLE_CAP>;
type InterfaceStore = EmbassyInterfaceStore<
    Mtx,
    INTERFACE_STORE_CAP,
    PACKET_PHY_RETENTION_CAPACITY,
    PACKET_PHY_INDEX_BUCKETS,
>;
type Node = PrnsNode<
    (),
    personal_hopspot_core::node_pages::NodePageRoutes,
    for<'a> fn(PrnsEvent<'a>, &()),
    InternalStorage,
    EmbassyHost<Mtx, S3Fn8EntropySource>,
    Mtx,
    LANE_COUNT,
    INTERFACE_CAPACITY,
    NOTIFY_CAP,
    COMMANDS_CAP,
    LIFECYCLE_CAP,
    COMPLETIONS_CAP,
>;
type ManifoldLanes = ManifoldLaneSet<Mtx, LANE_COUNT, NOTIFY_CAP>;

struct S3Fn8Hardware {
    usb_rx: UartRx<'static, Async>,
    usb_tx: UartTx<'static, Async>,
    lora_radio: LoraRadio,
    bluetooth: BT<'static>,
    identity_entropy: TrngSource<'static>,
    mac: [u8; 6],
    timebase: personal_rns::manifold::embassy::EmbassyTimebase,
    _rtc: Rtc<'static>,
    _vext: Output<'static>,
    _adc_control: Output<'static>,
}

static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
static INTERFACE_STORE: InterfaceStore = EmbassyInterfaceStore::new();
static USB_MANIFOLD_LANE: StaticManifoldLane<
    Mtx,
    EMBEDDED_MAX_WIRE_FRAME_LEN,
    LANE_DEPTH,
    OUTBOUND_BURST_DEPTH,
> = StaticManifoldLane::new();
static LORA_MANIFOLD_LANE: StaticManifoldLane<
    Mtx,
    LORA_MAX_PAYLOAD,
    LANE_DEPTH,
    OUTBOUND_BURST_DEPTH,
> = StaticManifoldLane::new();
static BLE_MANIFOLD_LANE: StaticManifoldLane<Mtx, BLE_HW_MTU, LANE_DEPTH, OUTBOUND_BURST_DEPTH> =
    StaticManifoldLane::new();
static USB_STATUS: EmbassyInterfaceStatus =
    EmbassyInterfaceStatus::new_accounted(USB_INTERFACE_ID, ConnectionState::Initializing);
static BLE_SHARED: BluetoothAutoShared<BLE_PEER_CAPACITY> =
    BluetoothAutoShared::new(BLE_SUPERVISOR_ID);
static BLE_OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();
static LORA_CONTROL: LoRaControl = LoRaControl::new();

#[embassy_executor::task]
async fn manifold_task(
    node: &'static mut Node,
    persistence: &'static mut crate::persistence::S3Fn8Persistence,
) {
    let _ = node.restore_embedded_persistence(persistence).await;
    node.run_manifold_with_persistence_and_interface_store(&INTERFACE_STORE, persistence)
        .await
}

#[embassy_executor::task]
async fn usb_device_task(rx: UartRx<'static, Async>, tx: UartTx<'static, Async>, seam: UsbSeam) {
    let device = UsbAutoDevice::new(UsbAutoDeviceInput {
        rx,
        tx,
        status: &USB_STATUS,
        bitrate: USB_UART_PAYLOAD_BITRATE_BPS,
        host_presence: ProtocolHostPresence::new(),
    });
    device.run(seam).await
}

fn ble_config() -> esp_radio::ble::Config {
    esp_radio::ble::Config::default()
        .with_task_priority(0)
        .with_task_stack_size(4096)
        .with_max_activities((BLE_PEER_CAPACITY + 1) as u8)
        .with_default_tx_power(esp_radio::ble::TxPower::P20)
}

#[embassy_executor::task]
async fn ble_task(
    spawner: Spawner,
    bt: BT<'static>,
    mac: [u8; 6],
    identity: BleIdentity,
    fleet: BleFleet,
) {
    Timer::after(BLE_START_DELAY).await;
    let connector =
        esp_radio::ble::controller::BleConnector::new(bt, ble_config()).expect("BLE connector");
    entropy::reseed_after_radio_start();
    crate::bluetooth_auto::run(connector, mac, identity, fleet, &BLE_SHARED, spawner).await;
}

fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}

pub use firmware::run;
