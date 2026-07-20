pub mod boards;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::{Input, InputConfig, Output, Pull};
use esp_hal::peripherals::USB_DEVICE;
use esp_hal::rng::Rng;
#[cfg(feature = "wifi")]
use esp_hal::rom::spiflash::esp_rom_spiflash_read;
use esp_hal::spi::master::Spi;
use esp_hal::system::Stack as CpuStack;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use esp_println::println;

#[cfg(feature = "wifi")]
use alloc::string::{String, ToString};
use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{select3, Either3};
#[cfg(feature = "wifi")]
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{
    Config as NetConfig, ConfigV6, DhcpConfig, IpEndpoint, Ipv6Cidr, Runner, Stack, StackResources,
    StaticConfigV6,
};
#[cfg(feature = "wifi")]
use embassy_net::{IpAddress, Ipv4Address, Ipv4Cidr, StaticConfigV4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
#[cfg(feature = "wifi")]
use embassy_time::with_timeout;
use embassy_time::{Delay, Duration, Ticker, Timer};
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_hal_bus::spi::ExclusiveDevice;
use heapless::Vec as HVec;
#[cfg(feature = "wifi")]
use portable_atomic::AtomicBool;
use portable_atomic::{AtomicU64, Ordering};
use static_cell::{ConstStaticCell, StaticCell};

#[cfg(feature = "wifi")]
use esp_radio::wifi::ap::AccessPointConfig;
#[cfg(feature = "wifi")]
use esp_radio::wifi::scan::ScanConfig;
#[cfg(feature = "wifi")]
use esp_radio::wifi::sta::StationConfig;
#[cfg(feature = "wifi")]
use esp_radio::wifi::{
    Config as WifiConfig, ControllerConfig, Interface as WifiStaDevice, PowerSaveMode,
    WifiController,
};

#[cfg(feature = "wifi")]
use esp_radio::esp_now::{
    EspNow, EspNowManager, EspNowReceiver, EspNowSender, WifiPhyRate, BROADCAST_ADDRESS,
};
#[cfg(feature = "ble")]
use personal_rns::ble::{BluetoothAutoShared, BluetoothAutoStatus};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
#[cfg(feature = "wifi")]
use personal_rns::esp_now::EspNowInterface;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::bluetooth_auto::limits;
#[cfg(feature = "wifi")]
use personal_rns::interfaces::esp_now::core::{
    self as espnow_core, Channel as EspNowChannel, ChannelPolicy,
};
use personal_rns::interfaces::lora::core::{channel_tag, DEFAULT_915_PROFILE};
use personal_rns::interfaces::radios::sx126x::Sx126x;
use personal_rns::interfaces::usb_auto::core::device_descriptor;
use personal_rns::interfaces::wifi_auto::core as wifi_core;
use personal_rns::interfaces::BitrateBps;
use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus, MacAddress,
    Membership,
};
use personal_rns::lora::{LoRaControl, LoRaInterface};
use personal_rns::reactor::embassy::timebase::EmbassyTimebase;
use personal_rns::reactor::embassy::{
    embassy_grant_lane, EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost,
    EmbassyInterfaceSeam, EmbassyInterfaceStatus, InterfaceLifecycle, PooledEgress,
};
use personal_rns::reactor::grant::FrameSlot;
use personal_rns::reactor::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
use personal_rns::reactor::reconnect::ReconnectPolicy;
use personal_rns::runtime::{
    CompletionPool, EmbassyInterfaceStore, Fleet, FleetWire, PreConfiguredDestination, PrnsEvent,
    PrnsNode, PrnsNodeHandle, PrnsNodeRecipe, ReactorPlumbing, RequestHandlerRegistration,
};
use personal_rns::storage::StorageLayout;
use personal_rns::tcp::TcpClient;
use personal_rns::usb::UsbAutoDevice;
use personal_rns::wifi::{AutoWifi, AutoWifiShared, AutoWifiStatus};

use crate::storage::EngineStorageType;

use personal_hopspot_core as screen;

esp_app_desc!();

#[cfg(feature = "wifi")]
mod hopspot_site {
    include!(concat!(env!("OUT_DIR"), "/hopspot_site.rs"));
}

#[cfg(feature = "wifi")]
const AP_IPV4: [u8; 4] = [192, 168, 4, 1];
#[cfg(feature = "wifi")]
const CAPTIVE_PORTAL_HOST: &str = "192.168.4.1";
const CAPTIVE_PORTAL_URL: &str = "http://192.168.4.1/";
#[cfg(feature = "wifi")]
const CAPTIVE_PORTAL_API_URL: &str = "http://192.168.4.1/captive-portal/api";
#[cfg(feature = "wifi")]
const HOPSPOT_CONFIG_OFFSET: u32 = 0xD000;
#[cfg(feature = "wifi")]
const HOPSPOT_CONFIG_MAGIC: &[u8; 8] = b"HSPCFG1\0";
#[cfg(feature = "wifi")]
const HOPSPOT_CONFIG_VERSION: u8 = 1;
#[cfg(feature = "wifi")]
const HOPSPOT_CONFIG_READ_WORDS: usize = 32;
#[cfg(feature = "wifi")]
const HOPSPOT_CONFIG_SSID_MAX: usize = 32;
#[cfg(feature = "wifi")]
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

/// One lane per top-level driver: USB (slot 0), the TCP client (slot 1), the WiFi supervisor's one
/// shared fleet lane (slot 2), and the LoRa SX1262 (slot 3). WiFi members do NOT each take a lane —
/// they share slot 2. Under `ble` the BLE fleet takes the next slot; under `esp-now` the
/// ESP-NOW broadcast carrier (which rides the same WiFi radio) takes another.
const IFACES: usize = 4 + cfg!(feature = "ble") as usize + cfg!(feature = "esp-now") as usize;
/// The WiFi fleet's member budget: a peer costs a descriptor + status slot, never a lane buffer.
const MEMBERS: usize = 24;
/// The engine-interface (descriptor + pacer) pool: the fixed interfaces plus the WiFi members.
/// Decoupled from the lane count `IFACES` on purpose: a member costs descriptors, not buffers.
const MAX_IFACES: usize = 3 + MEMBERS + cfg!(feature = "esp-now") as usize;
/// The WiFi supervisor's fleet lane (slot 2) key: an `AutoWifi`-kind id, so every `WifiPeer` child
/// routes to this one lane by the kind byte (`lane_serves`). Also the WiFi card's aggregate id.
const WIFI_FLEET_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
const WIFI_FLEET_SLOT: usize = 2;
const LANE_DEPTH: usize = 1;
const USB_SLOT: usize = 0;
/// Slot 1: the always-on TCP client wire, so the WiFi members never claim it.
const TCP_SLOT: usize = 1;
const LORA_SLOT: usize = 3;
/// The BLE fleet's pool slot (after LoRa), present only under `ble`. Distinct from the WiFi
/// slot so both supervisors run at once when WiFi and BLE coexist.
#[cfg(feature = "ble")]
const BLE_FLEET_SLOT: usize = 4;
/// The ESP-NOW broadcast carrier's pool slot, after the BLE fleet when it is present. A 1:1 interface
/// like LoRa (not a fleet); present under `esp-now`.
#[cfg(feature = "esp-now")]
const ESPNOW_SLOT: usize = 4 + cfg!(feature = "ble") as usize;
pub const NOTIFY_CAP: usize = 16;
const COMMANDS_CAP: usize = 8;
pub const LIFECYCLE_CAP: usize = 8;
const COMPLETIONS_CAP: usize = 4;

/// Core 1's stack carries *both* the one-time engine *construction* (the big, dalek-heavy transient)
/// and the per-poll ingest crypto the reactor runs afterward. The construction transient is the higher
/// *one-shot* peak, but the live reactor's ingress path (`Ingress::classify` under real traffic) is
/// itself deep, so this is load-bearing under load, not padding: trimming it to 74 KiB to fund core 0
/// booted (construction fit) but overflowed core 1 once live RF traffic hit the reactor. 84 KiB was the
/// peripheral build's floor; dual-role BLE pushed core 0 over the internal-SRAM ceiling, so this is
/// trimmed to 80 KiB (6 KiB above the measured-overflowing 74 KiB) to fund the core-0 stack, then
/// soak-tested under live RF. Do not trim further without re-soaking — the reactor floor is near here.
const CORE1_STACK_BYTES: usize = 80 * 1024;

const RENDER_INTERVAL: Duration = Duration::from_millis(500);
const RENDER_TICKS_PER_BATTERY: u8 = 4;
const NOTICE_MS: u64 = 900;
const OLED_SLEEP_DELAY_MS: u64 = 2_500;

const BUTTON_LONG_PRESS: Duration = Duration::from_millis(500);
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);

type Mtx = CriticalSectionRawMutex;
type LaneBuf = [FrameSlot<EMBEDDED_MAX_WIRE_FRAME_LEN>; LANE_DEPTH];
type LaneChannel = zerocopy_channel::Channel<'static, Mtx, FrameSlot<EMBEDDED_MAX_WIRE_FRAME_LEN>>;
type ReactorInbound = HVec<
    (
        InterfaceId,
        EmbassyGrantConsumer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
    ),
    IFACES,
>;
type ReactorEgressLanes = HVec<
    (
        InterfaceId,
        EmbassyGrantProducer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
    ),
    IFACES,
>;
type Handle = PrnsNodeHandle<'static, Mtx, COMMANDS_CAP, COMPLETIONS_CAP>;
type UsbSeam = EmbassyInterfaceSeam<'static, Mtx, NOTIFY_CAP, EMBEDDED_MAX_WIRE_FRAME_LEN>;
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
    EMBEDDED_MAX_WIRE_FRAME_LEN,
    IFACES,
    MAX_IFACES,
    NOTIFY_CAP,
    COMMANDS_CAP,
    LIFECYCLE_CAP,
    COMPLETIONS_CAP,
>;
const EMPTY_SLOT: FrameSlot<EMBEDDED_MAX_WIRE_FRAME_LEN> = FrameSlot::empty();
/// The free-slot id a pool slot carries until an interface occupies it (never a real medium id).
const FREE_SLOT: InterfaceId = InterfaceId::new([0xff; 8]);

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

#[cfg(feature = "wifi")]
use captive_portal::ap_ssid;
#[cfg(feature = "wifi")]
use configuration::{hopspot_wifi_config, HopspotWifiConfig};
use connectivity::build_tcp;
#[cfg(feature = "wifi")]
use connectivity::{build_wifi, espnow_channel_policy, EspNowAdapter};
#[cfg(feature = "wifi")]
use display::build_interface_menu_details;
use display::{build_cards, build_snapshots, button_task};

/// The WiFi supervisor's shared aggregate + per-peer status (written + read on core 0).
static WIFI_SHARED: AutoWifiShared<MEMBERS> = AutoWifiShared::new(WIFI_FLEET_ID);

/// Under `ble` the BLE supervisor reuses the (WiFi-free) fleet slot 2, keyed by its own kind
/// so `BluetoothPeer` members route to it. The radio carries `BLE_MEMBERS` concurrent connections (the
/// pooled `ble.rs` backend sizes its slot pool + trouble-host `CONNECTIONS` to this) — 2 since the
/// reduced embedded MTU ceiling (1472) freed the internal lane RAM to carry a second peer.
#[cfg(feature = "ble")]
pub const BLE_MEMBERS: usize = limits::ESP32_S3_MAX_PEERS;
#[cfg(feature = "ble")]
const BLE_FLEET_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);
#[cfg(feature = "ble")]
static BLE_SHARED: BluetoothAutoShared<BLE_MEMBERS> = BluetoothAutoShared::new(BLE_FLEET_ID);
static LORA_CONTROL: LoRaControl = LoRaControl::new();

/// The reactor's pool: one inbound + one outbound grant ring per slot, split at boot into the
/// reactor side (core 1's plumbing) and the interface side (core 0's USB seam / fleet wires).
static IN_BUF: [ConstStaticCell<LaneBuf>; IFACES] =
    [const { ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]) }; IFACES];
static IN_CH: [StaticCell<LaneChannel>; IFACES] = [const { StaticCell::new() }; IFACES];
static OUT_BUF: [ConstStaticCell<LaneBuf>; IFACES] =
    [const { ConstStaticCell::new([EMPTY_SLOT; LANE_DEPTH]) }; IFACES];
static OUT_CH: [StaticCell<LaneChannel>; IFACES] = [const { StaticCell::new() }; IFACES];

/// The reactor↔interface channels (cross-core via `CriticalSectionRawMutex`).
static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, personal_rns::engine::IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
/// The reactor's outbound-commit wake for the fleet lane: the egress (core 1) signals it on every
/// commit so the supervisor's drain is roused across the core boundary (the outbound mirror of `NOTIFY`).
static OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();
/// The BLE fleet's own outbound-commit wake (slot 4), so the BLE supervisor is roused only by its own
/// egress and not spuriously by WiFi commits when the two fleets coexist.
#[cfg(feature = "ble")]
static BLE_OUTBOUND_WAKE: Signal<Mtx, ()> = Signal::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
static BUTTON_EVENTS: Channel<Mtx, screen::InputEvent, 4> = Channel::new();
/// Per-interface engine counts the reactor (core 1) pushes into and the render task (core 0) reads —
/// a `CriticalSectionRawMutex` store so the `&'static` shared across cores stays `Sync`. Capacity is a
/// power of two above the interface ceiling, so a live interface's counts never get dropped.
static INTERFACE_STORE: InterfaceStore = EmbassyInterfaceStore::new();
const INTERFACE_STORE_CAP: usize = 32;
const PACKET_PHY_RETENTION_CAPACITY: usize = 32;
const PACKET_PHY_INDEX_BUCKETS: usize =
    personal_rns::routing::dedup::dedup_index_buckets(PACKET_PHY_RETENTION_CAPACITY);

/// The engine's entropy: the hardware TRNG blocks until WiFi RF is live (wifi::new enables it, but
/// the radio is not associated when the engine starts), so entropy is a board-unique software PRNG
/// over this `static` state. Acceptable ONLY because this whole identity is a NEVER-ship bring-up
/// fixture; the long-term fix is to gate the TRNG on RF-up. A fn (not a closure) so the host type
/// stays nameable for the cross-core move.
static ENTROPY_STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
#[cfg(feature = "wifi")]
static WIFI_STATION_JOINED: AtomicBool = AtomicBool::new(false);

fn seeded_entropy(bytes: &mut [u8]) {
    let mut state = ENTROPY_STATE.load(Ordering::Relaxed);
    for byte in bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state >> 24) as u8;
    }
    ENTROPY_STATE.store(state, Ordering::Relaxed);
}

/// The recipe's event sink — a fn (not a closure) so the node type stays nameable.
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
    #[cfg(feature = "wifi")]
    pub wifi: esp_hal::peripherals::WIFI<'static>,
    pub button: esp_hal::peripherals::GPIO0<'static>,
    pub cpu_ctrl: esp_hal::peripherals::CPU_CTRL<'static>,
    pub sw_int1: esp_hal::interrupt::software::SoftwareInterrupt<'static, 1>,
    pub timebase: EmbassyTimebase,
    /// The RTC handle is kept alive for the whole run so its disabled watchdogs stay disabled.
    pub _rtc: esp_hal::rtc_cntl::Rtc<'static>,
    #[cfg(feature = "ble")]
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
        let timebase = ::personal_rns::reactor::embassy::timebase::EmbassyTimebase::start_at(
            ::personal_rns::engine::InstantMillis(rtc.current_time_us() / 1000),
        );
        ::esp_println::println!("{} boot — recipe runtime, engine core 1 + I/O core 0", $banner);
        (sw_int.software_interrupt1, timebase, rtc)
    }};
}
pub(crate) use boot_common;

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "wifi"), allow(dead_code))]
pub enum RadioMode {
    Ble,
    AccessPoint,
}

#[cfg(feature = "wifi")]
const RADIO_MODE_AP: u32 = 0x4150_0001;
#[cfg(feature = "wifi")]
const RADIO_MODE_BLE: u32 = 0x424C_4501;
#[cfg(feature = "wifi")]
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RADIO_MODE_FLAG: u32 = 0;

fn boot_radio_mode(station_configured: bool) -> RadioMode {
    let _ = station_configured;
    #[cfg(feature = "wifi")]
    {
        let flag = unsafe { core::ptr::addr_of!(RADIO_MODE_FLAG).read() };
        if flag == RADIO_MODE_AP {
            return RadioMode::AccessPoint;
        }
        RadioMode::Ble
    }
    #[cfg(not(feature = "wifi"))]
    {
        RadioMode::Ble
    }
}

#[cfg(feature = "wifi")]
fn request_radio_mode(mode: RadioMode) -> ! {
    let flag = match mode {
        RadioMode::AccessPoint => RADIO_MODE_AP,
        RadioMode::Ble => RADIO_MODE_BLE,
    };
    unsafe { core::ptr::addr_of_mut!(RADIO_MODE_FLAG).write(flag) };
    esp_hal::system::software_reset();
}

mod firmware;
pub(super) use firmware::run;
