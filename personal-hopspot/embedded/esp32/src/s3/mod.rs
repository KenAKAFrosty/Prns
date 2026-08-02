mod board;
pub mod boards;

#[cfg(feature = "wifi-auto")]
use alloc::string::{String, ToString};
use core::fmt::Write as _;
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::{Input, Output};
use esp_hal::peripherals::USB_DEVICE;
use esp_hal::rng::Rng;
#[cfg(feature = "wifi-auto")]
use esp_hal::rom::spiflash::esp_rom_spiflash_read;
use esp_hal::spi::master::Spi;
use esp_hal::system::Stack as CpuStack;
#[cfg(feature = "wifi-auto")]
use esp_hal::time::Duration as HalDuration;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;

use embassy_executor::Spawner;
use embassy_futures::select::{select3, Either3};
#[cfg(feature = "wifi-auto")]
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{
    Config as NetConfig, ConfigV6, DhcpConfig, IpEndpoint, Ipv6Cidr, Runner, Stack, StackResources,
    StaticConfigV6,
};
#[cfg(feature = "wifi-auto")]
use embassy_net::{IpAddress, Ipv4Address, Ipv4Cidr, StaticConfigV4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
#[cfg(feature = "wifi-auto")]
use embassy_time::with_timeout;
use embassy_time::{Delay, Duration, Ticker, Timer};
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_hal_bus::spi::ExclusiveDevice;
use heapless::Vec as HVec;
#[cfg(feature = "wifi-auto")]
use portable_atomic::AtomicBool;
use portable_atomic::{AtomicU32, AtomicU64, Ordering};
use static_cell::StaticCell;

#[cfg(feature = "wifi-auto")]
use esp_radio::wifi::ap::AccessPointConfig;
#[cfg(feature = "wifi-auto")]
use esp_radio::wifi::scan::{ScanConfig, ScanTypeConfig};
#[cfg(feature = "wifi-auto")]
use esp_radio::wifi::sta::StationConfig;
#[cfg(feature = "wifi-auto")]
use esp_radio::wifi::{
    Config as WifiConfig, ControllerConfig, DisconnectReason, Interface as WifiStaDevice,
    PowerSaveMode, WifiController, WifiError,
};

#[cfg(feature = "wifi-auto")]
use esp_radio::esp_now::{
    EspNow, EspNowManager, EspNowReceiver, EspNowSender, WifiPhyRate, BROADCAST_ADDRESS,
};
#[cfg(feature = "bluetooth-auto")]
use personal_rns::bluetooth_auto::{BluetoothAutoShared, BluetoothAutoStatus};
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
#[cfg(feature = "wifi-auto")]
use personal_rns::esp_now::EspNowInterface;
#[cfg(feature = "bluetooth-auto")]
use personal_rns::interfaces::bluetooth_auto::{BleIdentity, BLE_HW_MTU};
#[cfg(feature = "wifi-auto")]
use personal_rns::interfaces::esp_now::{
    self as espnow_core, Channel as EspNowChannel, ChannelPolicy, ESP_NOW_V2_AIR_MTU,
};
use personal_rns::interfaces::lora::{AirtimePolicy, DEFAULT_915_PROFILE, LORA_MAX_PAYLOAD};
use personal_rns::interfaces::usb_auto::device_descriptor;
use personal_rns::interfaces::wifi_auto as wifi_auto_contract;
use personal_rns::interfaces::BitrateBps;
use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus, MacAddress,
    Membership,
};
use personal_rns::lora::{LoRaControl, LoRaInterface, LoRaInterfaceInput, LoRaSpectrumStatus};
use personal_rns::manifold::embassy::{
    EmbassyHost, EmbassyInterfaceSeam, EmbassyInterfaceStatus, EmbassyTimebase, InterfaceLifecycle,
};
use personal_rns::manifold::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::radios::sx126x::Sx126x;
use personal_rns::runtime::{
    minimum_interface_store_capacity, minimum_manifold_notification_capacity, CompletionPool,
    EmbassyInterfaceStore, Fleet, ManifoldLaneSet, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, StaticManifoldLane,
};
use personal_rns::storage::StorageLayout;
use personal_rns::tcp::{
    TcpClient, TcpClientInput, TcpClientTarget, TcpSocketBuffers, TCP_DNS_HOSTNAME_MAX_BYTES,
};
use personal_rns::usb_auto::{UsbAutoDevice, UsbAutoDeviceInput};
use personal_rns::wifi_auto::{
    AutoWifi, AutoWifiSegment, AutoWifiShared, AutoWifiStatus, AutoWifiTopology,
};
#[cfg(feature = "bluetooth-auto")]
use prns_interfaces_embassy::bluetooth_auto::PEER_CAPACITY as EMBEDDED_BLE_PEER_CAPACITY;

#[cfg(feature = "wifi-auto")]
use crate::station_recovery::{
    AccessPoint as StationAccessPoint, ConnectionFailure, ConnectionOutcome, ScanFailure,
    ScanOutcome, StationAttempt, StationRecovery, StationYield,
};
use crate::storage::EngineStorageType;

use personal_hopspot_core as screen;

pub(crate) use board::{
    BoardDisplay, BoardFace, Esp32S3Board, S3BoardHardware, S3InterfaceHardware, S3ManifoldHardware,
};

esp_app_desc!();

#[cfg(feature = "wifi-auto")]
mod hopspot_site {
    include!(concat!(env!("OUT_DIR"), "/hopspot_site.rs"));
}

#[cfg(feature = "wifi-auto")]
const AP_IPV4: [u8; 4] = [192, 168, 4, 1];
#[cfg(feature = "wifi-auto")]
const CAPTIVE_PORTAL_HOST: &str = "192.168.4.1";
const CAPTIVE_PORTAL_URL: &str = "http://192.168.4.1/";
#[cfg(feature = "wifi-auto")]
const CAPTIVE_PORTAL_API_URL: &str = "http://192.168.4.1/captive-portal/api";
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_OFFSET: u32 = 0xD000;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_MAGIC: &[u8; 8] = b"HSPCFG1\0";
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_VERSION: u8 = 1;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_READ_WORDS: usize = 92;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_SSID_MAX: usize = 32;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_PASSWORD_MAX: usize = 64;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_TCP_KIND_OFFSET: usize = 9;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_TCP_HOST_LENGTH_OFFSET: usize =
    16 + HOPSPOT_CONFIG_SSID_MAX + HOPSPOT_CONFIG_PASSWORD_MAX;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_TCP_PORT_OFFSET: usize = HOPSPOT_CONFIG_TCP_HOST_LENGTH_OFFSET + 1;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_TCP_TARGET_OFFSET: usize = HOPSPOT_CONFIG_TCP_PORT_OFFSET + 2;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_TCP_HOSTNAME_MAX: usize = 253;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_TCP_DEFAULT_PORT: u16 = 4242;

/// Fallback Wi-Fi network the board joins as a station, read at build time. Normal flashing writes the
/// same values into the reserved `hopcfg` flash slot so the published firmware artifact can stay
/// generic.
const WIFI_SSID: &str = match option_env!("HOPSPOT_WIFI_SSID") {
    Some(ssid) => ssid,
    None => "",
};
const WIFI_PASSWORD: &str = match option_env!("HOPSPOT_WIFI_PASSWORD") {
    Some(password) => password,
    None => "",
};

const HOPSPOT_TCP_TARGET: &str = match option_env!("HOPSPOT_TCP_TARGET") {
    Some(target) => target,
    None => "",
};
/// The board's claim about its pipe to the LAN node: it sets the declared MTU tier, which the
/// manifold then clamps to the embedded ceiling. A 2.4 GHz station's honest order of magnitude.
const TCP_BITRATE_BPS: BitrateBps = BitrateBps::guess(65_000_000);
/// One TCP socket's smoltcp rx/tx buffer — sized for the board's frames, DRAM-frugal over throughput.
const TCP_SOCKET_BUF: usize = 1_024;

const LANE_COUNT: usize =
    4 + cfg!(feature = "bluetooth-auto") as usize + cfg!(feature = "esp-now") as usize;
const MEMBERS: usize = 24;
#[cfg(feature = "bluetooth-auto")]
pub const BLE_PEER_CAPACITY: usize = EMBEDDED_BLE_PEER_CAPACITY;
#[cfg(not(feature = "bluetooth-auto"))]
pub const BLE_PEER_CAPACITY: usize = 0;
const INTERFACE_CAPACITY: usize =
    3 + MEMBERS + BLE_PEER_CAPACITY + cfg!(feature = "esp-now") as usize;
const WIFI_SUPERVISOR_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
const LANE_DEPTH: usize = 1;
const LORA_OUTBOUND_DEPTH: usize = 1;
pub const NOTIFY_CAP: usize = minimum_manifold_notification_capacity(LANE_COUNT, LANE_DEPTH);
const COMMANDS_CAP: usize = 8;
pub const LIFECYCLE_CAP: usize = 8;
const COMPLETIONS_CAP: usize = 4;

const CORE1_STACK_BYTES: usize = 72 * 1024;

const RENDER_INTERVAL: Duration = Duration::from_millis(500);
const RENDER_TICKS_PER_BATTERY: u8 = 4;
const NOTICE_MS: u64 = 900;
const OLED_SLEEP_DELAY_MS: u64 = 2_500;

const BUTTON_LONG_PRESS: Duration = Duration::from_millis(500);
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);

type Mtx = CriticalSectionRawMutex;
type Handle = PrnsNodeHandle<'static, Mtx, COMMANDS_CAP, COMPLETIONS_CAP>;
type UsbSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, EMBEDDED_MAX_WIRE_FRAME_LEN>;
#[cfg(feature = "lora")]
type S3LoraInterface = LoRaInterface<
    'static,
    ExclusiveDevice<Spi<'static, esp_hal::Async>, Output<'static>, Delay>,
    Input<'static>,
    Input<'static>,
    Output<'static>,
    Delay,
>;
#[cfg(feature = "lora")]
type S3LoraSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, LORA_MAX_PAYLOAD>;
#[cfg(feature = "esp-now")]
type S3EspNowInterface = EspNowInterface<'static, EspNowAdapter>;
#[cfg(feature = "esp-now")]
type S3EspNowSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, ESP_NOW_V2_AIR_MTU>;
type S3TcpSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, EMBEDDED_MAX_WIRE_FRAME_LEN>;
#[cfg(feature = "wifi-auto")]
type S3WifiFleet = Fleet<Mtx, { wifi_auto_contract::HARDWARE_MTU }, NOTIFY_CAP, LIFECYCLE_CAP>;
#[cfg(feature = "bluetooth-auto")]
type S3BleFleet = Fleet<Mtx, BLE_HW_MTU, NOTIFY_CAP, LIFECYCLE_CAP>;
type InterfaceStore = EmbassyInterfaceStore<
    Mtx,
    INTERFACE_STORE_CAP,
    PACKET_PHY_RETENTION_CAPACITY,
    PACKET_PHY_INDEX_BUCKETS,
>;
/// The fully-spelled node type, so it can ride to core 1 as a concrete `#[task]` argument — which
/// is why `on_event` is a fn pointer and the host's entropy is a fn pointer, not closures.
type S3Node = PrnsNode<
    (),
    screen::node_pages::NodePageRoutes,
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
macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static CELL: StaticCell<$t> = StaticCell::new();
        CELL.init($val)
    }};
}

mod captive_portal;
mod configuration;
mod connectivity;
mod display;

#[cfg(feature = "wifi-auto")]
use captive_portal::ap_ssid;
#[cfg(feature = "wifi-auto")]
use configuration::{hopspot_wifi_config, HopspotWifiConfig};
use configuration::{HopspotTcpClientConfig, HopspotTcpClientHost};
use connectivity::build_tcp;
#[cfg(feature = "wifi-auto")]
use connectivity::{build_wifi, espnow_channel_policy, EspNowAdapter};
#[cfg(feature = "wifi-auto")]
use display::build_interface_menu_details;
#[cfg(not(feature = "wifi-auto"))]
use display::{add_lora_spectrum, add_manifold_pressure};
use display::{build_cards, build_snapshots, button_task};

static WIFI_SHARED: AutoWifiShared<MEMBERS> = AutoWifiShared::new(WIFI_SUPERVISOR_ID);

#[cfg(feature = "bluetooth-auto")]
const BLE_SUPERVISOR_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);
#[cfg(feature = "bluetooth-auto")]
static BLE_SHARED: BluetoothAutoShared<BLE_PEER_CAPACITY> =
    BluetoothAutoShared::new(BLE_SUPERVISOR_ID);
static LORA_CONTROL: LoRaControl = LoRaControl::new();
static USB_MANIFOLD_LANE: StaticManifoldLane<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, LANE_DEPTH> =
    StaticManifoldLane::new();
static TCP_MANIFOLD_LANE: StaticManifoldLane<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, LANE_DEPTH> =
    StaticManifoldLane::new();
static WIFI_MANIFOLD_LANE: StaticManifoldLane<
    Mtx,
    { wifi_auto_contract::HARDWARE_MTU },
    LANE_DEPTH,
> = StaticManifoldLane::new();
static LORA_MANIFOLD_LANE: StaticManifoldLane<
    Mtx,
    LORA_MAX_PAYLOAD,
    LANE_DEPTH,
    LORA_OUTBOUND_DEPTH,
> = StaticManifoldLane::new();
#[cfg(feature = "bluetooth-auto")]
static BLE_MANIFOLD_LANE: StaticManifoldLane<Mtx, BLE_HW_MTU, LANE_DEPTH> =
    StaticManifoldLane::new();
#[cfg(feature = "esp-now")]
static ESPNOW_MANIFOLD_LANE: StaticManifoldLane<Mtx, ESP_NOW_V2_AIR_MTU, LANE_DEPTH> =
    StaticManifoldLane::new();

static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, personal_rns::engine::IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
static OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();
#[cfg(feature = "bluetooth-auto")]
static BLE_OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
static BUTTON_EVENTS: Channel<Mtx, screen::InputEvent, 4> = Channel::new();
/// Per-interface engine counts the manifold (core 1) pushes into and the render task (core 0) reads —
/// a `CriticalSectionRawMutex` store so the `&'static` shared across cores stays `Sync`. Capacity is a
/// power of two above the interface ceiling, so a live interface's counts never get dropped.
static INTERFACE_STORE: InterfaceStore = EmbassyInterfaceStore::new();
const INTERFACE_STORE_CAP: usize = minimum_interface_store_capacity(INTERFACE_CAPACITY);
const PACKET_PHY_RETENTION_CAPACITY: usize = 32;
const PACKET_PHY_INDEX_BUCKETS: usize =
    personal_rns::routing::dedup::dedup_index_buckets(PACKET_PHY_RETENTION_CAPACITY);

#[cfg(feature = "wifi-auto")]
static WIFI_STATION_JOINED: AtomicBool = AtomicBool::new(false);
static CORE_ONE_HEARTBEAT: AtomicU64 = AtomicU64::new(0);

fn hardware_entropy(bytes: &mut [u8]) {
    Rng::new().read(bytes);
}

fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}

const BOOT_PHASE_MAGIC: u32 = 0x5052_0000;

#[derive(Clone, Copy)]
pub(crate) enum BootPhase {
    OledBegin = 1,
    OledReady = 2,
    OledFailed = 3,
    WifiBegin = 4,
    WifiReady = 5,
    TcpBegin = 6,
    TcpReady = 7,
    CoreOneStartBegin = 8,
    CoreOneStartReady = 9,
    CoreOneExecutorReady = 10,
    PersistenceRestoreBegin = 11,
    PersistenceRestoreComplete = 12,
    DisplayRuntimeBegin = 13,
    DisplayFirstRenderBegin = 14,
    DisplayFirstRenderComplete = 15,
    DisplayFirstRenderUnavailable = 16,
    WifiConnectionBegin = 17,
    WifiAssociated = 18,
    WifiDiscoveryBegin = 19,
    WifiDiscoveryComplete = 20,
    NetworkReady = 21,
    BluetoothBegin = 22,
    BluetoothReady = 23,
    WatchdogReady = 24,
    AnnounceBegin = 25,
    AnnounceDeliveryIssueReturned = 26,
    AnnounceNodeIssueReturned = 27,
}

impl BootPhase {
    fn label(self) -> &'static str {
        match self {
            Self::OledBegin => "oled.begin",
            Self::OledReady => "oled.ready",
            Self::OledFailed => "oled.failed",
            Self::WifiBegin => "wifi.begin",
            Self::WifiReady => "wifi.ready",
            Self::TcpBegin => "tcp.begin",
            Self::TcpReady => "tcp.ready",
            Self::CoreOneStartBegin => "core1.start.begin",
            Self::CoreOneStartReady => "core1.start.ready",
            Self::CoreOneExecutorReady => "core1.executor.ready",
            Self::PersistenceRestoreBegin => "persistence.restore.begin",
            Self::PersistenceRestoreComplete => "persistence.restore.complete",
            Self::DisplayRuntimeBegin => "display.runtime.begin",
            Self::DisplayFirstRenderBegin => "display.first-render.begin",
            Self::DisplayFirstRenderComplete => "display.first-render.complete",
            Self::DisplayFirstRenderUnavailable => "display.first-render.unavailable",
            Self::WifiConnectionBegin => "wifi.connection.begin",
            Self::WifiAssociated => "wifi.associated",
            Self::WifiDiscoveryBegin => "wifi.discovery.begin",
            Self::WifiDiscoveryComplete => "wifi.discovery.complete",
            Self::NetworkReady => "network.ready",
            Self::BluetoothBegin => "bluetooth.begin",
            Self::BluetoothReady => "bluetooth.ready",
            Self::WatchdogReady => "watchdog.ready",
            Self::AnnounceBegin => "announce.begin",
            Self::AnnounceDeliveryIssueReturned => "announce.delivery.issue.return",
            Self::AnnounceNodeIssueReturned => "announce.node.issue.return",
        }
    }
}

#[esp_hal::ram(unstable(rtc_fast, persistent))]
static LAST_BOOT_PHASE: AtomicU32 = AtomicU32::new(0);

pub(crate) fn previous_boot_phase() -> u32 {
    let encoded = LAST_BOOT_PHASE.load(Ordering::Relaxed);
    if encoded & 0xFFFF_0000 == BOOT_PHASE_MAGIC {
        encoded & 0x0000_FFFF
    } else {
        0
    }
}

pub(crate) fn boot_stage(phase: BootPhase) {
    let number = phase as u32;
    let stage = phase.label();
    LAST_BOOT_PHASE.store(BOOT_PHASE_MAGIC | number, Ordering::Relaxed);
    log::info!(
        "boot-stage t_ms={} phase={number} stage={stage}",
        embassy_time::Instant::now().as_millis()
    );
}

const DCACHE_FREE_BASE: usize = 0x3FCF_0000;
const DCACHE_FREE_LEN: usize = 32 * 1024;

pub(crate) fn reclaim_dcache_region() {
    // SAFETY: On this PSRAM-enabled ESP32-S3 layout, 0x3FCF0000..0x3FCF8000 is the documented
    // unused DCache address window. This boot-only function runs once before any allocations.
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            DCACHE_FREE_BASE as *mut u8,
            DCACHE_FREE_LEN,
            esp_alloc::MemoryCapability::Internal.into(),
        ));
    }
}

#[embassy_executor::task]
async fn usb_device_task(
    rx: UsbSerialJtagRx<'static, Async>,
    tx: UsbSerialJtagTx<'static, Async>,
    seam: UsbSeam,
    status: &'static EmbassyInterfaceStatus,
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
        status,
        host_present,
    });
    device.run(seam).await
}

/// The identical ESP32-S3 early boot every board's `bringup` runs first: allocators (PSRAM +
/// internal + the reclaimed D-cache region), the RTOS timer, and the RTC with its watchdogs disabled
/// for the slow PSRAM-backed engine construction. A block expression (so its bindings escape
/// macro hygiene) owning `$p`'s early peripherals, yielding `(software_interrupt1, timebase, rtc)`.
///
/// Heap region order is load-bearing for Wi-Fi boards: PSRAM must register first so capability-free
/// boot allocations land externally and leave the small internal regions for the radio. Heltec V4-R8
/// (Octal) passes a custom `PsramConfig` and uses `split_psram_heap`, which splits the window so
/// engine construction gets a freelist of its own without starving the rest of the system.
macro_rules! boot_common {
    ($p:ident, $banner:expr) => {
        $crate::s3::boot_common!(
            $p,
            $banner,
            ::esp_hal::psram::PsramConfig::default(),
            global_psram_heap
        )
    };
    ($p:ident, $banner:expr, $psram_config:expr) => {
        $crate::s3::boot_common!($p, $banner, $psram_config, split_psram_heap)
    };
    ($p:ident, $banner:expr, $psram_config:expr, global_psram_heap) => {{
        ::esp_println::logger::init_logger_from_env();
        $crate::s3::boot_add_psram_global!($p, $psram_config);
        ::esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 43 * 1024);
        $crate::s3::reclaim_dcache_region();
        $crate::s3::boot_psram_probe!();
        $crate::s3::boot_rtos_tail!($p, $banner)
    }};
    ($p:ident, $banner:expr, $psram_config:expr, split_psram_heap) => {{
        ::esp_println::logger::init_logger_from_env();
        $crate::s3::boot_add_psram_split!($p, $psram_config);
        ::esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 43 * 1024);
        $crate::s3::reclaim_dcache_region();
        $crate::s3::boot_psram_probe!();
        $crate::s3::boot_rtos_tail!($p, $banner)
    }};
}
pub(crate) use boot_common;

macro_rules! boot_add_psram_global {
    ($p:ident, $psram_config:expr) => {{
        let psram = ::esp_hal::psram::Psram::new($p.PSRAM, $psram_config);
        let (start, size) = psram.raw_parts();
        ::esp_println::println!("PSRAM mapped start={start:?} size={size} (global)");
        // SAFETY: region comes from esp-hal's PSRAM mapper; added once at boot.
        unsafe {
            ::esp_alloc::HEAP.add_region(::esp_alloc::HeapRegion::new(
                start,
                size,
                ::esp_alloc::MemoryCapability::External.into(),
            ));
        }
    }};
}
pub(crate) use boot_add_psram_global;

/// Split a board's PSRAM into two disjoint windows: a private low half owned solely by [`PsramAlloc`]
/// for engine construction, and a high half handed to `esp_alloc`. The global half is registered
/// before the internal region so capability-free allocations land externally and leave the small
/// internal regions for the radios, which is the same ordering `global_psram_heap` relies on.
///
/// Reserving the whole window privately instead leaves the system on internal RAM alone, roughly
/// 75 KiB across the reclaimed region and the D-cache window. The Bluetooth controller's receive
/// buffers cannot be allocated from that, and neither can the captive portal, whose four HTTP tasks
/// want 24 KiB each. Both fail on a board carrying 8 MiB of PSRAM.
macro_rules! boot_add_psram_split {
    ($p:ident, $psram_config:expr) => {{
        let psram = ::esp_hal::psram::Psram::new($p.PSRAM, $psram_config);
        let (start, size) = psram.raw_parts();
        let private_size = size / 2;
        let global_size = size - private_size;
        ::esp_println::println!(
            "PSRAM mapped start={start:?} size={size} (private={private_size} global={global_size})"
        );
        // SAFETY: the low window is given to PsramAlloc alone and is never registered with
        // esp_alloc; the high window is disjoint from it and is added to the heap exactly once.
        unsafe {
            $crate::storage::init_private_psram_heap(start, private_size);
            ::esp_alloc::HEAP.add_region(::esp_alloc::HeapRegion::new(
                start.add(private_size),
                global_size,
                ::esp_alloc::MemoryCapability::External.into(),
            ));
        }
    }};
}
pub(crate) use boot_add_psram_split;

macro_rules! boot_psram_probe {
    () => {{
        // Keep the smoke test tiny: Heltec's private PSRAM path is a bump allocator
        // (deallocate is a no-op), and large probes would permanently consume the window
        // until the pre-engine reinit.
        let layout = ::core::alloc::Layout::from_size_align(4, 4).expect("u32 layout");
        let ptr =
            ::allocator_api2::alloc::Allocator::allocate(&$crate::storage::PsramAlloc, layout)
                .expect("PSRAM probe allocation failed");
        // SAFETY: exclusive PsramAlloc allocation; write/read (leak is fine — bump).
        unsafe {
            const PATTERN: u32 = 0xA5A5_5A5A;
            let p = ptr.cast::<u32>().as_ptr();
            p.write_volatile(PATTERN);
            let read = p.read_volatile();
            assert_eq!(read, PATTERN, "PSRAM probe readback mismatch");
            ::esp_println::println!("PSRAM probe ok read=0x{read:08X}");
        }
    }};
}
pub(crate) use boot_psram_probe;

macro_rules! boot_rtos_tail {
    ($p:ident, $banner:expr) => {{
        let timg0 = ::esp_hal::timer::timg::TimerGroup::new($p.TIMG0);
        let sw_int =
            ::esp_hal::interrupt::software::SoftwareInterruptControl::new($p.SW_INTERRUPT);
        ::esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
        let mut rtc = ::esp_hal::rtc_cntl::Rtc::new($p.LPWR);
        // The engine construction allocates + zeroes PSRAM-backed columns synchronously; PSRAM is
        // slow, so it can overrun the RTC watchdog's ~2s timeout. Disable RWDT/SWD over the boot.
        rtc.rwdt.disable();
        rtc.swd.disable();
        let timebase = ::personal_rns::manifold::embassy::EmbassyTimebase::start_at(
            ::personal_rns::engine::InstantMillis(rtc.current_time_us() / 1000),
        );
        ::esp_println::println!(
            "{} boot {} commit={} reset={:?} previous_phase={} — recipe runtime, engine core 1 + I/O core 0",
            $banner,
            env!("HOPSPOT_BUILD_IDENTITY"),
            env!("HOPSPOT_BUILD_COMMIT_SHORT"),
            ::esp_hal::system::reset_reason(),
            $crate::s3::previous_boot_phase()
        );
        (sw_int.software_interrupt1, timebase, rtc)
    }};
}
pub(crate) use boot_rtos_tail;

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "wifi-auto"), allow(dead_code))]
pub enum RadioMode {
    Ble,
    AccessPoint,
}

#[cfg(feature = "wifi-auto")]
const RADIO_MODE_AP: u32 = 0x4150_0001;
#[cfg(feature = "wifi-auto")]
const RADIO_MODE_BLE: u32 = 0x424C_4501;
#[cfg(feature = "wifi-auto")]
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RADIO_MODE_FLAG: u32 = 0;

fn boot_radio_mode(station_configured: bool) -> RadioMode {
    let _ = station_configured;
    #[cfg(feature = "wifi-auto")]
    {
        // SAFETY: Boot reads the aligned RTC-fast persistent word before concurrent tasks start;
        // volatile semantics are not required because reset is the only cross-execution boundary.
        let flag = unsafe { core::ptr::addr_of!(RADIO_MODE_FLAG).read() };
        if flag == RADIO_MODE_AP {
            return RadioMode::AccessPoint;
        }
        RadioMode::Ble
    }
    #[cfg(not(feature = "wifi-auto"))]
    {
        RadioMode::Ble
    }
}

#[cfg(feature = "wifi-auto")]
fn request_radio_mode(mode: RadioMode) -> ! {
    let flag = match mode {
        RadioMode::AccessPoint => RADIO_MODE_AP,
        RadioMode::Ble => RADIO_MODE_BLE,
    };
    // SAFETY: This is the sole write to the aligned RTC-fast word, immediately before a software
    // reset; no other task can observe or concurrently access the mutable static.
    unsafe { core::ptr::addr_of_mut!(RADIO_MODE_FLAG).write(flag) };
    esp_hal::system::software_reset();
}

mod firmware;
pub(super) use firmware::run;
