pub mod boards;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::{Input, InputConfig, Output, Pull};
use esp_hal::peripherals::USB_DEVICE;
use esp_hal::rng::Rng;
#[cfg(feature = "wifi-auto")]
use esp_hal::rom::spiflash::esp_rom_spiflash_read;
use esp_hal::spi::master::Spi;
use esp_hal::system::Stack as CpuStack;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use esp_println::println;

#[cfg(feature = "wifi-auto")]
use alloc::string::{String, ToString};
use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_futures::join::join;
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
use portable_atomic::{AtomicU64, Ordering};
use static_cell::StaticCell;

#[cfg(feature = "wifi-auto")]
use esp_radio::wifi::ap::AccessPointConfig;
#[cfg(feature = "wifi-auto")]
use esp_radio::wifi::scan::ScanConfig;
#[cfg(feature = "wifi-auto")]
use esp_radio::wifi::sta::StationConfig;
#[cfg(feature = "wifi-auto")]
use esp_radio::wifi::{
    Config as WifiConfig, ControllerConfig, Interface as WifiStaDevice, PowerSaveMode,
    WifiController, WifiError,
};

#[cfg(feature = "wifi-auto")]
use esp_radio::esp_now::{
    EspNow, EspNowManager, EspNowReceiver, EspNowSender, WifiPhyRate, BROADCAST_ADDRESS,
};
#[cfg(feature = "bluetooth-auto")]
use personal_rns::bluetooth_auto::{BluetoothAutoShared, BluetoothAutoStatus};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
#[cfg(feature = "wifi-auto")]
use personal_rns::esp_now::EspNowInterface;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
#[cfg(feature = "bluetooth-auto")]
use personal_rns::interfaces::bluetooth_auto::{BleIdentity, BLE_HW_MTU};
#[cfg(feature = "wifi-auto")]
use personal_rns::interfaces::esp_now::{
    self as espnow_core, Channel as EspNowChannel, ChannelPolicy, ESP_NOW_V2_AIR_MTU,
};
use personal_rns::interfaces::lora::{channel_tag, DEFAULT_915_PROFILE, LORA_MAX_PAYLOAD};
use personal_rns::interfaces::usb_auto::device_descriptor;
use personal_rns::interfaces::wifi_auto as wifi_auto_contract;
use personal_rns::interfaces::BitrateBps;
use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus, MacAddress,
    Membership,
};
use personal_rns::lora::{LoRaControl, LoRaInterface};
use personal_rns::radios::sx126x::Sx126x;
use personal_rns::reactor::embassy::{
    EmbassyHost, EmbassyInterfaceSeam, EmbassyInterfaceStatus, EmbassyTimebase, InterfaceLifecycle,
};
use personal_rns::reactor::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
use personal_rns::reactor::reconnect::ReconnectPolicy;
use personal_rns::runtime::{
    minimum_interface_store_capacity, minimum_reactor_notification_capacity, CompletionPool,
    EmbassyInterfaceStore, Fleet, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, ReactorLaneSet, RequestHandlerRegistration, StaticReactorLane,
};
use personal_rns::storage::StorageLayout;
use personal_rns::tcp::TcpClient;
use personal_rns::usb_auto::UsbAutoDevice;
use personal_rns::wifi_auto::{AutoWifi, AutoWifiShared, AutoWifiStatus};
#[cfg(feature = "bluetooth-auto")]
use prns_interfaces_embassy::bluetooth_auto::PEER_CAPACITY as EMBEDDED_BLE_PEER_CAPACITY;

use crate::storage::EngineStorageType;

use personal_hopspot_core as screen;

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
const HOPSPOT_CONFIG_READ_WORDS: usize = 32;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_SSID_MAX: usize = 32;
#[cfg(feature = "wifi-auto")]
const HOPSPOT_CONFIG_PASSWORD_MAX: usize = 64;

/// Fallback WiFi network the board joins as a station, read at build time. Normal flashing writes the
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

/// The LAN Reticulum TCP node the board dials (`ip:port`, e.g. `192.168.1.50:4242`), read at build
/// time like the WiFi creds. Empty (or unparseable) leaves the TCP interface down. No DNS — a
/// resolved address only. Rides the WiFi stack, so it needs WiFi up.
const HOPSPOT_TCP_TARGET: &str = match option_env!("HOPSPOT_TCP_TARGET") {
    Some(target) => target,
    None => "",
};
/// The board's claim about its pipe to the LAN node: it sets the declared MTU tier, which the
/// reactor then clamps to the embedded ceiling. A 2.4 GHz station's honest order of magnitude.
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
pub const NOTIFY_CAP: usize = minimum_reactor_notification_capacity(LANE_COUNT, LANE_DEPTH);
const COMMANDS_CAP: usize = 8;
pub const LIFECYCLE_CAP: usize = 8;
const COMPLETIONS_CAP: usize = 4;

const CORE1_STACK_BYTES: usize = 80 * 1024;

const RENDER_INTERVAL: Duration = Duration::from_millis(500);
const RENDER_TICKS_PER_BATTERY: u8 = 4;
const NOTICE_MS: u64 = 900;
const OLED_SLEEP_DELAY_MS: u64 = 2_500;

const BUTTON_LONG_PRESS: Duration = Duration::from_millis(500);
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);

type Mtx = CriticalSectionRawMutex;
type Handle = PrnsNodeHandle<'static, Mtx, COMMANDS_CAP, COMPLETIONS_CAP>;
type UsbSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, EMBEDDED_MAX_WIRE_FRAME_LEN>;
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
    (),
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
type ReactorLanes = ReactorLaneSet<Mtx, LANE_COUNT, NOTIFY_CAP>;
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
use connectivity::build_tcp;
#[cfg(feature = "wifi-auto")]
use connectivity::{build_wifi, espnow_channel_policy, EspNowAdapter};
#[cfg(feature = "wifi-auto")]
use display::build_interface_menu_details;
use display::{build_cards, build_snapshots, button_task};

static WIFI_SHARED: AutoWifiShared<MEMBERS> = AutoWifiShared::new(WIFI_SUPERVISOR_ID);

#[cfg(feature = "bluetooth-auto")]
const BLE_SUPERVISOR_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);
#[cfg(feature = "bluetooth-auto")]
static BLE_SHARED: BluetoothAutoShared<BLE_PEER_CAPACITY> =
    BluetoothAutoShared::new(BLE_SUPERVISOR_ID);
static LORA_CONTROL: LoRaControl = LoRaControl::new();
static USB_REACTOR_LANE: StaticReactorLane<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, LANE_DEPTH> =
    StaticReactorLane::new();
static TCP_REACTOR_LANE: StaticReactorLane<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, LANE_DEPTH> =
    StaticReactorLane::new();
static WIFI_REACTOR_LANE: StaticReactorLane<Mtx, { wifi_auto_contract::HARDWARE_MTU }, LANE_DEPTH> =
    StaticReactorLane::new();
static LORA_REACTOR_LANE: StaticReactorLane<Mtx, LORA_MAX_PAYLOAD, LANE_DEPTH> =
    StaticReactorLane::new();
#[cfg(feature = "bluetooth-auto")]
static BLE_REACTOR_LANE: StaticReactorLane<Mtx, BLE_HW_MTU, LANE_DEPTH> = StaticReactorLane::new();
#[cfg(feature = "esp-now")]
static ESPNOW_REACTOR_LANE: StaticReactorLane<Mtx, ESP_NOW_V2_AIR_MTU, LANE_DEPTH> =
    StaticReactorLane::new();

static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, personal_rns::engine::IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
static OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();
#[cfg(feature = "bluetooth-auto")]
static BLE_OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
static BUTTON_EVENTS: Channel<Mtx, screen::InputEvent, 4> = Channel::new();
/// Per-interface engine counts the reactor (core 1) pushes into and the render task (core 0) reads —
/// a `CriticalSectionRawMutex` store so the `&'static` shared across cores stays `Sync`. Capacity is a
/// power of two above the interface ceiling, so a live interface's counts never get dropped.
static INTERFACE_STORE: InterfaceStore = EmbassyInterfaceStore::new();
const INTERFACE_STORE_CAP: usize = minimum_interface_store_capacity(INTERFACE_CAPACITY);
const PACKET_PHY_RETENTION_CAPACITY: usize = 32;
const PACKET_PHY_INDEX_BUCKETS: usize =
    personal_rns::routing::dedup::dedup_index_buckets(PACKET_PHY_RETENTION_CAPACITY);

#[cfg(feature = "wifi-auto")]
static WIFI_STATION_JOINED: AtomicBool = AtomicBool::new(false);

fn hardware_entropy(bytes: &mut [u8]) {
    Rng::new().read(bytes);
}

fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}

/// Print the allocator's per-region high-water footprint over the boot log: `External` is the
/// mapped PSRAM (and the engine's boxed columns), `Internal` the 56 KiB SRAM heap. Safe only
/// before the USB interface claims the USB-serial-JTAG: a construction-time probe, never run-loop.
fn log_heap_footprint(label: &str) {
    println!("[mem] {label}");
    println!("{}", esp_alloc::HEAP.stats());
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

/// The SX1262 radio handle, identical on every ESP32-S3 board: the pin *identity* (which GPIO is
/// SCK/CS/BUSY/…) is erased into `Output`/`Input` by the time the driver holds it, so only the wiring
/// in [`Esp32S3Board::bringup`] differs — the type the shared core threads does not.
pub type LoraRadio = Sx126x<
    ExclusiveDevice<Spi<'static, esp_hal::Async>, Output<'static>, Delay>,
    Input<'static>,
    Input<'static>,
    Output<'static>,
    Delay,
>;

/// Everything [`Esp32S3Board::bringup`] hands the shared core: the board-built peripherals
/// (display, battery, radio) plus the leftover singletons. esp-hal singletons can't be partially
/// moved through a borrow, so the board takes the whole `Peripherals` and returns what is left.
pub struct Bringup<D, B> {
    pub display: D,
    pub oled_ok: bool,
    pub battery: B,
    pub usb_device: USB_DEVICE<'static>,
    #[cfg(feature = "lora")]
    pub lora_radio: LoraRadio,
    #[cfg(feature = "wifi-auto")]
    pub wifi: esp_hal::peripherals::WIFI<'static>,
    pub button: esp_hal::peripherals::GPIO0<'static>,
    pub cpu_ctrl: esp_hal::peripherals::CPU_CTRL<'static>,
    pub sw_int1: esp_hal::interrupt::software::SoftwareInterrupt<'static, 1>,
    pub timebase: EmbassyTimebase,
    /// The RTC handle is kept alive for the whole run so its disabled watchdogs stay disabled.
    pub _rtc: esp_hal::rtc_cntl::Rtc<'static>,
    #[cfg(feature = "bluetooth-auto")]
    pub bt: esp_hal::peripherals::BT<'static>,
}

/// The per-board seam: the ~6% of an ESP32-S3 Hopspot that differs between boards (identity
/// strings, display driver + flush, battery source, power/pin bring-up). Everything else lives in
/// [`firmware::run_core`], so a shared-path change can never again rot one board while the other compiles.
#[allow(async_fn_in_trait)]
pub trait Esp32S3Board {
    const ANNOUNCE_APP_DATA: &'static [u8];
    const BOOT_BANNER: &'static str;
    type Display: DrawTarget<Color = BinaryColor>;
    type Battery: screen::BatterySource;

    fn usb_status() -> &'static EmbassyInterfaceStatus;
    /// Push the framebuffer to the panel — the one display op that is not `embedded-graphics`.
    fn flush(display: &mut Self::Display);
    /// Turn the panel driver on/off without changing the retained framebuffer.
    fn set_display_awake(display: &mut Self::Display, awake: bool);
    /// Own `Peripherals` (esp-hal singletons can't be partial-moved through a borrow): bring up
    /// power/display/battery/SX1262, run [`boot_common`], and hand the rest back in [`Bringup`].
    async fn bringup(
        p: esp_hal::peripherals::Peripherals,
        spawner: &Spawner,
    ) -> Bringup<Self::Display, Self::Battery>;
}

#[embassy_executor::task]
async fn usb_device_task(
    rx: UsbSerialJtagRx<'static, Async>,
    tx: UsbSerialJtagTx<'static, Async>,
    seam: UsbSeam,
    id: InterfaceId,
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
    let device = UsbAutoDevice::new(id, rx, tx, status, host_present);
    device.run(seam).await
}

/// The identical ESP32-S3 early boot every board's `bringup` runs first: allocators (PSRAM +
/// internal + the reclaimed D-cache region), the RTOS timer, and the RTC with its watchdogs disabled
/// for the slow PSRAM-backed engine construction. A block expression (so its bindings escape
/// macro hygiene) owning `$p`'s early peripherals, yielding `(software_interrupt1, timebase, rtc)`.
/// PSRAM registers first and that order is load-bearing: the allocator serves a capability-free
/// allocation from the first region with space, so external must lead or ordinary boot
/// allocations bleed the two small internal regions dry and the radio bring-up — whose
/// allocations genuinely require internal SRAM — finds crumbs and dies on core 0
/// (measured on the Heltec V4: wifi init 37.9K + ble connector 31.6K of 75.8K internal, 60 bytes left).
macro_rules! boot_common {
    ($p:ident, $banner:expr) => {{
        ::esp_println::logger::init_logger_from_env();
        ::esp_alloc::psram_allocator!($p.PSRAM, ::esp_hal::psram);
        ::esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 38 * 1024);
        $crate::s3::reclaim_dcache_region();
        let timg0 = ::esp_hal::timer::timg::TimerGroup::new($p.TIMG0);
        let sw_int =
            ::esp_hal::interrupt::software::SoftwareInterruptControl::new($p.SW_INTERRUPT);
        ::esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
        let mut rtc = ::esp_hal::rtc_cntl::Rtc::new($p.LPWR);
        // The engine construction allocates + zeroes PSRAM-backed columns synchronously; PSRAM is
        // slow, so it can overrun the RTC watchdog's ~2s timeout. Disable RWDT/SWD over the boot.
        rtc.rwdt.disable();
        rtc.swd.disable();
        let timebase = ::personal_rns::reactor::embassy::EmbassyTimebase::start_at(
            ::personal_rns::engine::InstantMillis(rtc.current_time_us() / 1000),
        );
        ::esp_println::println!("{} boot — recipe runtime, engine core 1 + I/O core 0", $banner);
        (sw_int.software_interrupt1, timebase, rtc)
    }};
}
pub(crate) use boot_common;

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
