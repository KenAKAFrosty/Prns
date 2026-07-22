use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::peripherals::{BT, USB_DEVICE};
use esp_hal::rng::Rng;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;

use personal_rns::engine::{InstantMillis, IssuedCommand, RatchetPolicy};
#[cfg(feature = "bluetooth-auto")]
use personal_rns::interfaces::bluetooth_auto::BleIdentity;
use personal_rns::interfaces::usb_auto::device_descriptor;
use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::reactor::embassy::{
    EmbassyHost, EmbassyInterfaceSeam, EmbassyInterfaceStatus, EmbassyTimebase, InterfaceLifecycle,
};
use personal_rns::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use personal_rns::runtime::{
    CompletionPool, EmbassyInterfaceStore, PreConfiguredDestination, PrnsEvent, PrnsNode,
    PrnsNodeHandle, PrnsNodeRecipe, RequestHandlerRegistration, StaticReactorPool,
};
use personal_rns::usb_auto::UsbAutoDevice;

use crate::storage::{C6Storage, EngineStorageType};

use embassy_sync::signal::Signal;
#[cfg(feature = "bluetooth-auto")]
use personal_rns::bluetooth_auto::BluetoothAutoShared;
use personal_rns::interfaces::InterfaceKind;
use personal_rns::runtime::Fleet;
#[cfg(feature = "bluetooth-auto")]
use prns_interfaces_embassy::bluetooth_auto::EmbeddedBleBackend;

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
    self as espnow_core, Channel as EspNowChannel, ChannelPolicy,
};
use personal_rns::reactor::interface_seam::Interface;

esp_app_desc!();

const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x13Personal Hopspot C6\xc0";

const USB_LANE: usize = 1;
const ESPNOW_LANE: usize = cfg!(feature = "esp-now") as usize;
const BLE_LANE: usize = cfg!(feature = "bluetooth-auto") as usize;
const LANE_COUNT: usize = USB_LANE + ESPNOW_LANE + BLE_LANE;
#[cfg(feature = "bluetooth-auto")]
pub const BLE_MEMBERS: usize = EmbeddedBleBackend::MAX_PEERS;
#[cfg(not(feature = "bluetooth-auto"))]
pub const BLE_MEMBERS: usize = 0;
pub const BLE_CONTROLLER_CONNECTIONS: usize = 8;
const INTERFACE_CAPACITY: usize = LANE_COUNT + BLE_LANE * BLE_MEMBERS + 1;
pub const NOTIFY_CAP: usize = 32;
const COMMANDS_CAP: usize = 8;
pub const LIFECYCLE_CAP: usize = 32;
const COMPLETIONS_CAP: usize = 4;
const INTERFACE_STORE_CAP: usize = 32;
const PACKET_PHY_RETENTION_CAPACITY: usize = 32;
const PACKET_PHY_INDEX_BUCKETS: usize =
    personal_rns::routing::dedup::dedup_index_buckets(PACKET_PHY_RETENTION_CAPACITY);
#[cfg(feature = "bluetooth-auto")]
const BLE_START_DELAY: Duration = Duration::from_secs(3);
// BLE needs heap for esp-radio's controller + trouble-host's boxed GATT clients/reassemblers; 64 KB
// covers it with margin. Kept off the larger end so the leftover linker `.stack` region stays big
// enough for the BLE construction transient (the single-core main task runs on `.stack` — esp-rtos
// gives it no separate task stack, so RAM spent on the heap is RAM taken from that one stack).
#[cfg(not(any(feature = "bluetooth-auto", feature = "esp-now")))]
const HEAP_BYTES: usize = 32 * 1024;
#[cfg(all(feature = "bluetooth-auto", not(feature = "esp-now")))]
const HEAP_BYTES: usize = 64 * 1024;
#[cfg(all(feature = "esp-now", not(feature = "bluetooth-auto")))]
const HEAP_BYTES: usize = 72 * 1024;
#[cfg(all(feature = "esp-now", feature = "bluetooth-auto"))]
const HEAP_BYTES: usize = 88 * 1024;
#[cfg(feature = "bluetooth-auto")]
fn c6_ble_config() -> esp_radio::ble::Config {
    esp_radio::ble::Config::default()
        .with_task_priority(0)
        .with_task_stack_size(4096)
        .with_max_connections(BLE_CONTROLLER_CONNECTIONS as u16)
        .with_default_tx_power(esp_radio::ble::TxPower::P20)
}

const LANE_DEPTH: usize = 1;
const USB_SLOT: usize = 0;
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"hopsp-c6");
#[cfg(feature = "esp-now")]
const ESPNOW_SLOT: usize = USB_LANE;
#[cfg(feature = "bluetooth-auto")]
const BLE_SUPERVISOR_SLOT: usize = USB_LANE + ESPNOW_LANE;
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
type C6BleFleet = Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP>;
type Node = PrnsNode<
    (),
    (),
    for<'a> fn(PrnsEvent<'a>, &()),
    EngineStorageType,
    EmbassyHost<fn(&mut [u8])>,
    Mtx,
    EMBEDDED_MAX_WIRE_FRAME_LEN,
    LANE_COUNT,
    INTERFACE_CAPACITY,
    NOTIFY_CAP,
    COMMANDS_CAP,
    LIFECYCLE_CAP,
    COMPLETIONS_CAP,
>;

static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
static INTERFACE_STORE: InterfaceStore = EmbassyInterfaceStore::new();
static REACTOR_POOL: StaticReactorPool<
    Mtx,
    EMBEDDED_MAX_WIRE_FRAME_LEN,
    LANE_DEPTH,
    LANE_COUNT,
> = StaticReactorPool::new();
static USB_STATUS: EmbassyInterfaceStatus =
    EmbassyInterfaceStatus::new(USB_INTERFACE_ID, ConnectionState::Initializing);
#[cfg(feature = "bluetooth-auto")]
static BLE_SHARED: BluetoothAutoShared<BLE_MEMBERS> = BluetoothAutoShared::new(BLE_SUPERVISOR_ID);
#[cfg(feature = "bluetooth-auto")]
static BLE_OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();

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
    let device = UsbAutoDevice::new(USB_INTERFACE_ID, rx, tx, &USB_STATUS, host_present);
    device.run(seam).await
}

#[cfg(feature = "esp-now")]
const ESPNOW_SEND_RETRIES: u8 = 8;
#[cfg(feature = "esp-now")]
const ESPNOW_SEND_RETRY_DELAY: Duration = Duration::from_millis(5);

#[cfg(feature = "esp-now")]
const fn espnow_phy_rate() -> WifiPhyRate {
    WifiPhyRate::Rate6m
}

#[cfg(feature = "bluetooth-auto")]
#[embassy_executor::task]
async fn ble_task(
    spawner: Spawner,
    bt: BT<'static>,
    mac: [u8; 6],
    identity: BleIdentity,
    fleet: C6BleFleet,
    shared: &'static BluetoothAutoShared<BLE_MEMBERS>,
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
            let _ = self.manager.set_rate(espnow_phy_rate());
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
