mod board;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::peripherals::{BT, USB_DEVICE};
use esp_hal::rng::Rng;
use esp_hal::usb_serial_jtag::{UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;

use personal_rns::engine::IssuedCommand;
#[cfg(feature = "bluetooth-auto")]
use personal_rns::interfaces::bluetooth_auto::{BleIdentity, BLE_HW_MTU};
use personal_rns::interfaces::usb_auto::device_descriptor;
use personal_rns::interfaces::{BitrateBps, ConnectionState, InterfaceId};
use personal_rns::manifold::embassy::{
    EmbassyHost, EmbassyInterfaceSeam, EmbassyInterfaceStatus, InterfaceLifecycle,
};
use personal_rns::manifold::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use personal_rns::runtime::{
    minimum_interface_store_capacity, minimum_manifold_notification_capacity, CompletionPool,
    EmbassyInterfaceStore, ManifoldLaneSet, PrnsEvent, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe,
    StaticManifoldLane,
};
use personal_rns::usb_auto::{UsbAutoDevice, UsbAutoDeviceInput};

use crate::storage::{C6Storage, EngineStorageType};

use embassy_sync::signal::Signal;
#[cfg(feature = "bluetooth-auto")]
use personal_rns::bluetooth_auto::BluetoothAutoShared;
use personal_rns::interfaces::InterfaceKind;
use personal_rns::runtime::Fleet;
#[cfg(feature = "bluetooth-auto")]
use prns_interfaces_embassy::bluetooth_auto::PEER_CAPACITY as EMBEDDED_BLE_PEER_CAPACITY;

#[cfg(feature = "esp-now")]
use esp_radio::esp_now::{
    EspNow, EspNowManager, EspNowReceiver, EspNowSender, WifiPhyRate, BROADCAST_ADDRESS,
};
#[cfg(feature = "esp-now")]
use esp_radio::wifi::ControllerConfig;
#[cfg(feature = "esp-now")]
use personal_rns::esp_now::EspNowInterface;
#[cfg(feature = "esp-now")]
use personal_rns::interfaces::esp_now::{
    self as espnow_core, Channel as EspNowChannel, ChannelPolicy, ESP_NOW_V2_AIR_MTU,
};
use personal_rns::manifold::interface_seam::Interface;

esp_app_desc!();

use board::{C6Hardware, XiaoEsp32C6, ANNOUNCE_APP_DATA, NODE_ANNOUNCE_APP_DATA, USB_INTERFACE_ID};

const USB_LANE: usize = 1;
const ESPNOW_LANE: usize = cfg!(feature = "esp-now") as usize;
const BLE_LANE: usize = cfg!(feature = "bluetooth-auto") as usize;
const LANE_COUNT: usize = USB_LANE + ESPNOW_LANE + BLE_LANE;
const LANE_DEPTH: usize = 1;
const OUTBOUND_BURST_DEPTH: usize = C6Storage::MAX_OUTGOING_RESOURCE_REACTION_FRAMES;
#[cfg(feature = "bluetooth-auto")]
pub const BLE_PEER_CAPACITY: usize = EMBEDDED_BLE_PEER_CAPACITY;
#[cfg(not(feature = "bluetooth-auto"))]
pub const BLE_PEER_CAPACITY: usize = 0;
const INTERFACE_CAPACITY: usize = LANE_COUNT + BLE_LANE * BLE_PEER_CAPACITY + 1;
pub const NOTIFY_CAP: usize = minimum_manifold_notification_capacity(LANE_COUNT, LANE_DEPTH);
const COMMANDS_CAP: usize = 8;
pub const LIFECYCLE_CAP: usize = 32;
const COMPLETIONS_CAP: usize = 4;
const INTERFACE_STORE_CAP: usize = minimum_interface_store_capacity(INTERFACE_CAPACITY);
const PACKET_PHY_RETENTION_CAPACITY: usize = 32;
const PACKET_PHY_INDEX_BUCKETS: usize =
    personal_rns::routing::dedup::dedup_index_buckets(PACKET_PHY_RETENTION_CAPACITY);
const _: () = assert!(C6Storage::LINK_SESSIONS > BLE_PEER_CAPACITY);
#[cfg(feature = "bluetooth-auto")]
const BLE_START_DELAY: Duration = Duration::from_secs(3);
#[cfg(feature = "bluetooth-auto")]
fn c6_ble_config() -> esp_radio::ble::Config {
    esp_radio::ble::Config::default()
        .with_task_priority(0)
        .with_task_stack_size(4096)
        .with_max_connections(BLE_PEER_CAPACITY as u16)
        .with_default_tx_power(esp_radio::ble::TxPower::P20)
}

#[cfg(feature = "bluetooth-auto")]
const BLE_SUPERVISOR_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);

type Mtx = CriticalSectionRawMutex;
type UsbSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, EMBEDDED_MAX_WIRE_FRAME_LEN>;
type InterfaceStore = EmbassyInterfaceStore<
    Mtx,
    INTERFACE_STORE_CAP,
    PACKET_PHY_RETENTION_CAPACITY,
    PACKET_PHY_INDEX_BUCKETS,
>;
#[cfg(feature = "bluetooth-auto")]
type C6BleFleet = Fleet<Mtx, BLE_HW_MTU, NOTIFY_CAP, LIFECYCLE_CAP>;
type Node = PrnsNode<
    (),
    personal_hopspot_core::node_pages::NodePageRoutes,
    for<'a> fn(PrnsEvent<'a>, &()),
    EngineStorageType,
    EmbassyHost<fn(&mut [u8])>,
    Mtx,
    LANE_COUNT,
    INTERFACE_CAPACITY,
    NOTIFY_CAP,
    COMMANDS_CAP,
    LIFECYCLE_CAP,
    COMPLETIONS_CAP,
>;
type ManifoldLanes = ManifoldLaneSet<Mtx, LANE_COUNT, NOTIFY_CAP>;

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
#[cfg(feature = "esp-now")]
static ESPNOW_MANIFOLD_LANE: StaticManifoldLane<
    Mtx,
    ESP_NOW_V2_AIR_MTU,
    LANE_DEPTH,
    OUTBOUND_BURST_DEPTH,
> = StaticManifoldLane::new();
#[cfg(feature = "bluetooth-auto")]
static BLE_MANIFOLD_LANE: StaticManifoldLane<Mtx, BLE_HW_MTU, LANE_DEPTH, OUTBOUND_BURST_DEPTH> =
    StaticManifoldLane::new();
static USB_STATUS: EmbassyInterfaceStatus =
    EmbassyInterfaceStatus::new_accounted(USB_INTERFACE_ID, ConnectionState::Initializing);
#[cfg(feature = "bluetooth-auto")]
static BLE_SHARED: BluetoothAutoShared<BLE_PEER_CAPACITY> =
    BluetoothAutoShared::new(BLE_SUPERVISOR_ID);
#[cfg(feature = "bluetooth-auto")]
static BLE_OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();

#[embassy_executor::task]
async fn manifold_task(
    node: &'static mut Node,
    persistence: &'static mut crate::persistence::C6Persistence,
) {
    let _ = node.restore_embedded_persistence(persistence).await;
    node.run_manifold_with_persistence_and_interface_store(&INTERFACE_STORE, persistence)
        .await
}

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static CELL: StaticCell<$t> = StaticCell::new();
        CELL.init($val)
    }};
}

fn hardware_entropy(bytes: &mut [u8]) {
    Rng::new().read(bytes);
}

fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}

#[embassy_executor::task]
async fn usb_device_task(
    rx: UsbSerialJtagRx<'static, Async>,
    tx: UsbSerialJtagTx<'static, Async>,
    seam: UsbSeam,
) {
    let mut last_sof = 0u16;
    let host_present = move || {
        let frame = USB_DEVICE::regs()
            .fram_num()
            .read()
            .sof_frame_index()
            .bits();
        let advanced = frame != last_sof;
        last_sof = frame;
        advanced
    };
    let device = UsbAutoDevice::new(UsbAutoDeviceInput {
        rx,
        tx,
        status: &USB_STATUS,
        host_present,
    });
    device.run(seam).await
}

#[cfg(feature = "esp-now")]
const ESPNOW_SEND_RETRIES: u8 = 8;
#[cfg(feature = "esp-now")]
const ESPNOW_SEND_RETRY_DELAY: Duration = Duration::from_millis(5);

#[cfg(feature = "esp-now")]
struct EspNowPhySettings {
    driver_rate: WifiPhyRate,
    bitrate: BitrateBps,
}

#[cfg(feature = "esp-now")]
const ESPNOW_PHY: EspNowPhySettings = EspNowPhySettings {
    driver_rate: WifiPhyRate::Rate6m,
    bitrate: BitrateBps::guess(12_000_000),
};

#[cfg(feature = "bluetooth-auto")]
#[embassy_executor::task]
async fn ble_task(
    spawner: Spawner,
    bt: BT<'static>,
    mac: [u8; 6],
    identity: BleIdentity,
    fleet: C6BleFleet,
    shared: &'static BluetoothAutoShared<BLE_PEER_CAPACITY>,
) {
    Timer::after(BLE_START_DELAY).await;
    let connector =
        esp_radio::ble::controller::BleConnector::new(bt, c6_ble_config()).expect("ble connector");
    crate::bluetooth_auto::run(connector, mac, identity, fleet, shared, spawner).await;
}

#[cfg(feature = "esp-now")]
fn espnow_channel_policy() -> ChannelPolicy {
    ChannelPolicy::Fixed(EspNowChannel::DEFAULT)
}

#[cfg(feature = "esp-now")]
struct EspNowAdapter {
    manager: EspNowManager<'static>,
    sender: EspNowSender<'static>,
    receiver: EspNowReceiver<'static>,
    rate_applied: bool,
}

#[cfg(feature = "esp-now")]
impl EspNowAdapter {
    fn new(esp_now: EspNow<'static>) -> Self {
        let (manager, sender, receiver) = esp_now.split();
        Self {
            manager,
            sender,
            receiver,
            rate_applied: false,
        }
    }

    fn ensure_rate(&mut self) {
        if !self.rate_applied {
            let _ = self.manager.set_rate(ESPNOW_PHY.driver_rate);
            self.rate_applied = true;
        }
    }
}

#[cfg(feature = "esp-now")]
impl espnow_core::EspNowRadio for EspNowAdapter {
    fn set_channel(&mut self, channel: EspNowChannel) {
        let _ = self.manager.set_channel(channel.as_u8());
    }

    async fn broadcast(&mut self, frame: &[u8]) -> bool {
        self.ensure_rate();
        for _ in 0..ESPNOW_SEND_RETRIES {
            if self
                .sender
                .send_async(&BROADCAST_ADDRESS, frame)
                .await
                .is_ok()
            {
                return true;
            }
            Timer::after(ESPNOW_SEND_RETRY_DELAY).await;
        }
        false
    }

    async fn receive(&mut self, buf: &mut [u8]) -> usize {
        let frame = self.receiver.receive_async().await;
        let data = frame.data();
        let len = data.len().min(buf.len());
        buf[..len].copy_from_slice(&data[..len]);
        len
    }
}

mod firmware;
pub use firmware::run;
