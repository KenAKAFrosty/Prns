mod board;
pub mod boards;
mod gnss;

use alloc::string::{String, ToString};
use core::fmt::Write as _;
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::Input;
#[cfg(feature = "lora")]
use esp_hal::gpio::Output;
use esp_hal::peripherals::USB_DEVICE;
use esp_hal::rng::Rng;
use esp_hal::rom::spiflash::esp_rom_spiflash_read;
#[cfg(feature = "lora")]
use esp_hal::spi::master::Spi;
use esp_hal::system::Stack as CpuStack;
use esp_hal::time::Duration as HalDuration;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;

use embassy_executor::Spawner;
use embassy_futures::select::{select3, Either3};
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{
    Config as NetConfig, ConfigV6, DhcpConfig, IpEndpoint, Ipv6Cidr, Runner, Stack, StackResources,
    StaticConfigV6,
};
use embassy_net::{IpAddress, Ipv4Address, Ipv4Cidr, StaticConfigV4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::with_timeout;
#[cfg(feature = "lora")]
use embassy_time::Delay;
use embassy_time::{Duration, Ticker, Timer};
#[cfg(feature = "lora")]
use embedded_hal_bus::spi::ExclusiveDevice;
use heapless::Vec as HVec;
use portable_atomic::AtomicBool;
use portable_atomic::{AtomicU32, AtomicU64, Ordering};
use static_cell::StaticCell;

use esp_radio::wifi::ap::AccessPointConfig;
use esp_radio::wifi::scan::{ScanConfig, ScanTypeConfig};
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{
    AuthenticationMethod, Config as WifiConfig, ControllerConfig, DisconnectReason,
    Interface as WifiStaDevice, PowerSaveMode, WifiController, WifiError,
};

use esp_radio::esp_now::{
    EspNow, EspNowManager, EspNowReceiver, EspNowSender, WifiPhyRate, BROADCAST_ADDRESS,
};
use personal_rns::bluetooth_auto::{BluetoothAutoShared, BluetoothAutoStatus};
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::esp_now::EspNowInterface;
use personal_rns::interfaces::bluetooth_auto::BLE_HW_MTU;
use personal_rns::interfaces::esp_now::{
    self as espnow_core, Channel as EspNowChannel, ChannelPolicy, ESP_NOW_V2_AIR_MTU,
};
#[cfg(feature = "lora")]
use personal_rns::interfaces::lora::{boot_lora_profile, AirtimePolicy, LORA_MAX_PAYLOAD};
use personal_rns::interfaces::usb_auto::device_descriptor;
use personal_rns::interfaces::wifi_auto as wifi_auto_contract;
use personal_rns::interfaces::BitrateBps;
use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus, MacAddress,
    Membership,
};
#[cfg(feature = "lora")]
use personal_rns::lora::{
    LoRaApplyOutcome, LoRaControl, LoRaInterface, LoRaInterfaceInput, LoRaSpectrumStatus,
};
use personal_rns::manifold::embassy::{
    EmbassyHost, EmbassyInterfaceSeam, EmbassyInterfaceStatus, EmbassyTimebase, InterfaceLifecycle,
};
use personal_rns::manifold::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
use personal_rns::manifold::reconnect::ReconnectPolicy;
#[cfg(feature = "lora")]
use personal_rns::radios::sx126x::Sx126x;
use personal_rns::runtime::{
    minimum_interface_store_capacity, minimum_manifold_notification_capacity, CompletionPool,
    EmbassyInterfaceStore, Fleet, ManifoldLaneSet, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, SharedNorFlash, StaticManifoldLane,
};
use personal_rns::storage::StorageLayout;
use personal_rns::tcp::{
    TcpClient, TcpClientInput, TcpClientTarget, TcpSocketBuffers, TCP_DNS_HOSTNAME_MAX_BYTES,
};
use personal_rns::usb_auto::{UsbAutoDevice, UsbAutoDeviceInput};
use personal_rns::wifi_auto::{
    tcp_rendezvous, AutoWifi, AutoWifiSegment, AutoWifiShared, AutoWifiStatus, AutoWifiTopology,
    TcpRendezvousBuffers, TcpRendezvousClients, TcpRendezvousServer, TcpRendezvousStorage,
    TcpRendezvousWireSlot, UdpServiceDiscovery, UdpServiceDiscoveryStorage,
    TCP_RENDEZVOUS_CLIENT_CAPACITY, TCP_RENDEZVOUS_FRAMED_LEN, TCP_RENDEZVOUS_FRAME_CAP,
    TCP_RENDEZVOUS_READ_BUFFER_BYTES, TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES,
    UDP_SERVICE_DISCOVERY_RX_SOCKET_BYTES, UDP_SERVICE_DISCOVERY_RX_SOCKET_METADATA,
    UDP_SERVICE_DISCOVERY_SOCKET_COUNT, UDP_SERVICE_DISCOVERY_TX_SOCKET_BYTES,
    UDP_SERVICE_DISCOVERY_TX_SOCKET_METADATA,
};
use prns_interfaces_embassy::bluetooth_auto::PEER_CAPACITY as EMBEDDED_BLE_PEER_CAPACITY;

use crate::station_recovery::{
    AccessPoint as StationAccessPoint, ConnectionFailure, ConnectionOutcome, DiscoveryScope,
    ScanFailure, ScanOutcome, StationAttempt, StationRecovery, StationYield,
};
use crate::station_security::{ObservedAuthentication, StationSecurity};
use crate::storage::EngineStorageType;

use personal_hopspot_core as screen;

pub(crate) use crate::display_runtime::{
    ImmediateBoardDisplay, RetainedBoardDisplay, RetainedDisplayDevice, S3BoardDisplay,
    S3DisplayRuntime, S3Presentation,
};
pub(crate) use crate::immediate_display::ImmediateDisplayDevice;
#[cfg(feature = "lora")]
pub(crate) use board::LoraRadio;
pub(crate) use board::{
    BoardFace, Esp32S3Board, S3BoardHardware, S3InterfaceHardware, S3ManifoldHardware,
};
pub(crate) use gnss::{GnssProvider, GnssShared, NoGnss};

esp_app_desc!();

const AP_IPV4: [u8; 4] = [192, 168, 4, 1];
const CAPTIVE_PORTAL_HOST: &str = "192.168.4.1";
const CAPTIVE_PORTAL_URL: &str = "http://192.168.4.1/";
const CAPTIVE_PORTAL_API_URL: &str = "http://192.168.4.1/captive-portal/api";
const HOPSPOT_CONFIG_OFFSET: u32 = 0xD000;
const HOPSPOT_CONFIG_MAGIC: &[u8; 8] = b"HSPCFG1\0";
const HOPSPOT_CONFIG_VERSION: u8 = 1;
const HOPSPOT_CONFIG_READ_WORDS: usize = 92;
const HOPSPOT_CONFIG_SSID_MAX: usize = 32;
const HOPSPOT_CONFIG_PASSWORD_MAX: usize = 64;
const HOPSPOT_CONFIG_TCP_KIND_OFFSET: usize = 9;
const HOPSPOT_CONFIG_TCP_HOST_LENGTH_OFFSET: usize =
    16 + HOPSPOT_CONFIG_SSID_MAX + HOPSPOT_CONFIG_PASSWORD_MAX;
const HOPSPOT_CONFIG_TCP_PORT_OFFSET: usize = HOPSPOT_CONFIG_TCP_HOST_LENGTH_OFFSET + 1;
const HOPSPOT_CONFIG_TCP_TARGET_OFFSET: usize = HOPSPOT_CONFIG_TCP_PORT_OFFSET + 2;
const HOPSPOT_CONFIG_TCP_HOSTNAME_MAX: usize = 253;
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
const TCP_BITRATE_BPS: BitrateBps = wifi_auto_contract::WIFI_EMBEDDED_BITRATE_CEILING_BPS;
const TCP_SOCKET_BUFFER_BYTES: usize = 4 * 1_024;

const LANE_COUNT: usize = 5 + cfg!(feature = "lora") as usize;
const MEMBERS: usize = 24;
pub const BLE_PEER_CAPACITY: usize = EMBEDDED_BLE_PEER_CAPACITY;
pub const BLE_CONTROLLER_ACTIVITY_CAPACITY: u8 = (BLE_PEER_CAPACITY + 1) as u8;
// The S3 Wi-Fi blob creates its driver task at priority 29. Keep the BLE controller immediately
// below it so an active GATT link cannot monopolize core 0 ahead of Wi-Fi receive processing.
const BLE_CONTROLLER_TASK_PRIORITY: u8 = 28;
const INTERFACE_CAPACITY: usize = LANE_COUNT + MEMBERS + BLE_PEER_CAPACITY;
const WIFI_SUPERVISOR_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
const LANE_DEPTH: usize = 1;
/// A resource request is one inbound frame that synchronously emits its parts and, at most, one
/// hashmap update. Derive every S3 lane's PSRAM backlog from the engine storage recipe: the compact
/// build can hold only eighteen parts, rather than the protocol-wide seventy-five-part window.
const OUTBOUND_BURST_DEPTH: usize = EngineStorageType::MAX_OUTGOING_RESOURCE_REACTION_FRAMES;
pub const NOTIFY_CAP: usize = minimum_manifold_notification_capacity(LANE_COUNT, LANE_DEPTH);
const _: () = assert!(EngineStorageType::LINK_SESSIONS > MEMBERS + BLE_PEER_CAPACITY);
const _: () = assert!(BLE_CONTROLLER_ACTIVITY_CAPACITY <= 10);
const COMMANDS_CAP: usize = 8;
pub const LIFECYCLE_CAP: usize = 8;
const COMPLETIONS_CAP: usize = 4;

const CORE1_STACK_BYTES: usize = 72 * 1024;
const RECLAIMED_HEAP_BYTES: usize = 72 * 1024;
// Large Wi-Fi, BLE, and manifold async state machines plus both embassy-net socket tables park
// ordinary software state in PSRAM. Reinvest the released `.bss` in the internal-only radio heap:
// the S3 Wi-Fi blob can retain its complete RX profile during BLE coexistence without exhausting
// the allocator. Execution and both CPU stacks remain in internal RAM; only suspended future state
// lives in PSRAM (Heltec V4-R8 additionally polls `run_core` on a dedicated PSRAM stack).
const RADIO_INTERNAL_HEAP_BYTES: usize = 52 * 1024;

const RENDER_INTERVAL_MS: u64 = 500;
const RENDER_INTERVAL: Duration = Duration::from_millis(RENDER_INTERVAL_MS);
/// Keep the existing two-second probe cadence so the Heltec's voltage-trend charging inference
/// stays responsive. The exact level is human-facing and moves much more slowly, so publish it only
/// every ten seconds; fresh external-power state still reaches the plug on every probe.
const BATTERY_SAMPLE_INTERVAL_MS: u64 = 2_000;
const BATTERY_DISPLAY_INTERVAL_MS: u64 = 10_000;
const RENDER_TICKS_PER_BATTERY_SAMPLE: u8 = (BATTERY_SAMPLE_INTERVAL_MS / RENDER_INTERVAL_MS) as u8;
const RENDER_TICKS_PER_BATTERY_DISPLAY: u8 =
    (BATTERY_DISPLAY_INTERVAL_MS / RENDER_INTERVAL_MS) as u8;
const NOTICE_MS: u64 = 900;
const DISPLAY_SLEEP_DELAY_MS: u64 = 2_500;

const BUTTON_LONG_PRESS: Duration = Duration::from_millis(500);
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);

type Mtx = CriticalSectionRawMutex;
type Handle = PrnsNodeHandle<'static, Mtx, COMMANDS_CAP, COMPLETIONS_CAP>;
type UsbSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, EMBEDDED_MAX_WIRE_FRAME_LEN>;
#[cfg(feature = "lora")]
type S3LoraInterface = LoRaInterface<'static, LoraRadio>;
#[cfg(feature = "lora")]
type S3LoraSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, LORA_MAX_PAYLOAD>;
type S3EspNowInterface = EspNowInterface<'static, EspNowAdapter>;
type S3EspNowSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, ESP_NOW_V2_AIR_MTU>;
type S3TcpSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, EMBEDDED_MAX_WIRE_FRAME_LEN>;
type S3WifiFleet = Fleet<Mtx, { wifi_auto_contract::HARDWARE_MTU }, NOTIFY_CAP, LIFECYCLE_CAP>;
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

use captive_portal::ap_ssid;
use configuration::{hopspot_wifi_config, HopspotWifiConfig};
use configuration::{HopspotTcpClientConfig, HopspotTcpClientHost};
use connectivity::{build_tcp, build_wifi, espnow_channel_policy, EspNowAdapter, ESPNOW_PHY};
use display::build_interface_menu_details;
use display::{build_cards, build_snapshots, button_task};

static WIFI_SHARED: AutoWifiShared<MEMBERS> = AutoWifiShared::new(WIFI_SUPERVISOR_ID);

const BLE_SUPERVISOR_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);
static BLE_SHARED: BluetoothAutoShared<BLE_PEER_CAPACITY> =
    BluetoothAutoShared::new(BLE_SUPERVISOR_ID);
#[cfg(feature = "lora")]
static LORA_CONTROL: LoRaControl = LoRaControl::new();
static USB_MANIFOLD_LANE: StaticManifoldLane<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, LANE_DEPTH, 0> =
    StaticManifoldLane::new();
static TCP_MANIFOLD_LANE: StaticManifoldLane<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, LANE_DEPTH, 0> =
    StaticManifoldLane::new();
static WIFI_MANIFOLD_LANE: StaticManifoldLane<
    Mtx,
    { wifi_auto_contract::HARDWARE_MTU },
    LANE_DEPTH,
    0,
> = StaticManifoldLane::new();
#[cfg(feature = "lora")]
static LORA_MANIFOLD_LANE: StaticManifoldLane<Mtx, LORA_MAX_PAYLOAD, LANE_DEPTH, 0> =
    StaticManifoldLane::new();
static BLE_MANIFOLD_LANE: StaticManifoldLane<Mtx, BLE_HW_MTU, LANE_DEPTH, 0> =
    StaticManifoldLane::new();
static ESPNOW_MANIFOLD_LANE: StaticManifoldLane<Mtx, ESP_NOW_V2_AIR_MTU, LANE_DEPTH, 0> =
    StaticManifoldLane::new();

static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, personal_rns::engine::IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
static OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();
static BLE_OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
const BUTTON_EVENT_CAPACITY: usize = 4;
static BUTTON_EVENTS: Channel<Mtx, screen::InputEvent, BUTTON_EVENT_CAPACITY> = Channel::new();
/// Per-interface engine counts the manifold (core 1) pushes into and the render task (core 0) reads —
/// a `CriticalSectionRawMutex` store so the `&'static` shared across cores stays `Sync`. Capacity is a
/// power of two above the interface ceiling, so a live interface's counts never get dropped.
static INTERFACE_STORE: InterfaceStore = EmbassyInterfaceStore::new();
const INTERFACE_STORE_CAP: usize = minimum_interface_store_capacity(INTERFACE_CAPACITY);
const PACKET_PHY_RETENTION_CAPACITY: usize = 32;
const PACKET_PHY_INDEX_BUCKETS: usize =
    personal_rns::routing::dedup::dedup_index_buckets(PACKET_PHY_RETENTION_CAPACITY);

static WIFI_STATION_JOINED: AtomicBool = AtomicBool::new(false);
static WIFI_STATION_DATA_PATH_DEGRADED: AtomicBool = AtomicBool::new(false);
static WIFI_DRIVER_RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);
static CORE_ONE_HEARTBEAT: AtomicU64 = AtomicU64::new(0);

fn hardware_entropy(bytes: &mut [u8]) {
    Rng::new().read(bytes);
}

fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}

const BOOT_PHASE_MAGIC: u32 = 0x5052_0000;

#[derive(Clone, Copy)]
pub(crate) enum BootPhase {
    #[allow(dead_code)]
    DisplayHardwareBegin = 1,
    #[allow(dead_code)]
    DisplayHardwareReady = 2,
    #[allow(dead_code)]
    DisplayHardwareFailed = 3,
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
    AnnounceNodeIssueReturned = 27,
    /// Heltec V4-R8: about to hand `run_core` to the PSRAM-backed executor.
    #[cfg(feature = "heltec-v4-r8-boot")]
    RunCorePinBegin = 32,
    /// Heltec V4-R8: first instruction inside `run_core`.
    #[cfg(feature = "heltec-v4-r8-boot")]
    RunCoreEntered = 33,
    /// Heltec V4-R8: before first `run_core` await (LoRa profile / flash).
    #[cfg(feature = "heltec-v4-r8-boot")]
    RunCorePreLoraProfile = 34,
}

impl BootPhase {
    fn label(self) -> &'static str {
        match self {
            Self::DisplayHardwareBegin => "display.hardware.begin",
            Self::DisplayHardwareReady => "display.hardware.ready",
            Self::DisplayHardwareFailed => "display.hardware.failed",
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
            Self::AnnounceNodeIssueReturned => "announce.node.issue.return",
            #[cfg(feature = "heltec-v4-r8-boot")]
            Self::RunCorePinBegin => "run-core.pin.begin",
            #[cfg(feature = "heltec-v4-r8-boot")]
            Self::RunCoreEntered => "run-core.entered",
            #[cfg(feature = "heltec-v4-r8-boot")]
            Self::RunCorePreLoraProfile => "run-core.pre-lora-profile",
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
/// Heap region order is load-bearing for Wi-Fi boards: PSRAM must register first so capability-free boot allocations land externally and leave the small internal regions for the radio.
/// Heltec V4-R8 (Octal) passes a custom `PsramConfig` and uses `split_psram_heap`, which gives engine construction a private freelist without starving the rest of the system.
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
        ::esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: $crate::s3::RECLAIMED_HEAP_BYTES);
        ::esp_alloc::heap_allocator!(size: $crate::s3::RADIO_INTERNAL_HEAP_BYTES);
        $crate::s3::reclaim_dcache_region();
        $crate::s3::boot_psram_probe!();
        $crate::s3::boot_rtos_tail!($p, $banner)
    }};
    ($p:ident, $banner:expr, $psram_config:expr, split_psram_heap) => {{
        ::esp_println::logger::init_logger_from_env();
        $crate::s3::boot_add_psram_split!($p, $psram_config);
        ::esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: $crate::s3::RECLAIMED_HEAP_BYTES);
        ::esp_alloc::heap_allocator!(size: $crate::s3::RADIO_INTERNAL_HEAP_BYTES);
        $crate::s3::reclaim_dcache_region();
        $crate::s3::boot_psram_probe!();
        $crate::s3::boot_rtos_tail!($p, $banner)
    }};
}
pub(crate) use boot_common;

// Only the global-heap boards expand this arm; a build whose board set is all split-heap
// leaves it unexpanded.
#[allow(unused_macros)]
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
#[allow(unused_imports)]
pub(crate) use boot_add_psram_global;

/// Split a board's PSRAM into two disjoint windows: a private low half owned solely by [`crate::storage::PsramAlloc`] for engine construction, and a high half handed to `esp_alloc`.
/// Register the global half before the internal regions so capability-free allocations land externally and leave the small internal regions for the radios, matching the ordering `global_psram_heap` relies on.
///
/// Reserving the whole window privately instead leaves the system on roughly 75 KiB of internal RAM across the reclaimed region and the D-cache window.
macro_rules! boot_add_psram_split {
    ($p:ident, $psram_config:expr) => {{
        let psram = ::esp_hal::psram::Psram::new($p.PSRAM, $psram_config);
        let (start, size) = psram.raw_parts();
        let private_size = size / 2;
        let global_size = size - private_size;
        ::esp_println::println!(
            "PSRAM mapped start={start:?} size={size} (private={private_size} global={global_size})"
        );
        // SAFETY: The low window is given to PsramAlloc alone and is never registered with esp_alloc.
        // The high window is disjoint from it and is added to the heap exactly once.
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
pub enum RadioMode {
    Ble,
    AccessPoint,
}

const RADIO_MODE_AP: u32 = 0x4150_0001;
const RADIO_MODE_BLE: u32 = 0x424C_4501;
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RADIO_MODE_FLAG: u32 = 0;

fn boot_radio_mode(_station_configured: bool) -> RadioMode {
    #[cfg(feature = "wifi-security-probe")]
    return RadioMode::AccessPoint;

    // SAFETY: Boot reads the aligned RTC-fast persistent word before concurrent tasks start;
    // volatile semantics are not required because reset is the only cross-execution boundary.
    #[cfg(not(feature = "wifi-security-probe"))]
    let flag = unsafe { core::ptr::addr_of!(RADIO_MODE_FLAG).read() };
    #[cfg(not(feature = "wifi-security-probe"))]
    if flag == RADIO_MODE_AP {
        RadioMode::AccessPoint
    } else {
        RadioMode::Ble
    }
}

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
