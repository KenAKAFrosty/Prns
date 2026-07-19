pub mod boards;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::{Input, InputConfig, Output, Pull};
use esp_hal::peripherals::USB_DEVICE;
use esp_hal::rng::Rng;
#[cfg(feature = "radio-wifi")]
use esp_hal::rom::spiflash::esp_rom_spiflash_read;
use esp_hal::spi::master::Spi;
use esp_hal::system::Stack as CpuStack;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use esp_println::println;

#[cfg(feature = "radio-wifi")]
use alloc::string::{String, ToString};
use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{select3, Either3};
#[cfg(feature = "softap")]
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{
    Config as NetConfig, ConfigV6, DhcpConfig, IpEndpoint, Ipv6Cidr, Runner, Stack, StackResources,
    StaticConfigV6,
};
#[cfg(feature = "softap")]
use embassy_net::{IpAddress, Ipv4Address, Ipv4Cidr, StaticConfigV4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
#[cfg(feature = "softap")]
use embassy_time::with_timeout;
use embassy_time::{Delay, Duration, Ticker, Timer};
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_hal_bus::spi::ExclusiveDevice;
use heapless::Vec as HVec;
#[cfg(feature = "radio-wifi")]
use portable_atomic::AtomicBool;
use portable_atomic::{AtomicU64, Ordering};
use static_cell::{ConstStaticCell, StaticCell};

#[cfg(feature = "softap")]
use esp_radio::wifi::ap::AccessPointConfig;
#[cfg(feature = "radio-wifi")]
use esp_radio::wifi::scan::ScanConfig;
#[cfg(feature = "radio-wifi")]
use esp_radio::wifi::sta::StationConfig;
#[cfg(feature = "radio-wifi")]
use esp_radio::wifi::{
    Config as WifiConfig, ControllerConfig, Interface as WifiStaDevice, PowerSaveMode,
    WifiController,
};

#[cfg(feature = "radio-wifi")]
use esp_radio::esp_now::{
    EspNow, EspNowManager, EspNowReceiver, EspNowSender, WifiPhyRate, BROADCAST_ADDRESS,
};
#[cfg(feature = "ble-bringup")]
use personal_rns::ble::{BluetoothAutoShared, BluetoothAutoStatus};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
#[cfg(feature = "radio-wifi")]
use personal_rns::esp_now::EspNowInterface;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::bluetooth_auto::limits;
#[cfg(feature = "radio-wifi")]
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
use personal_rns::runtime::{
    CompletionPool, EmbassyInterfaceStore, Fleet, FleetWire, PreConfiguredDestination, PrnsEvent,
    PrnsNode, PrnsNodeHandle, PrnsNodeRecipe, ReactorPlumbing, RequestHandlerRegistration,
};
use personal_rns::storage::StorageLayout;
use personal_rns::tcp::client::TcpClient;
use personal_rns::usb::UsbAutoDevice;
use personal_rns::wifi::{AutoWifi, AutoWifiShared, AutoWifiStatus};

use crate::storage::EngineStorageType;

use personal_hopspot_core as screen;

esp_app_desc!();

#[cfg(feature = "softap")]
mod hopspot_site {
    include!(concat!(env!("OUT_DIR"), "/hopspot_site.rs"));
}

#[cfg(feature = "softap")]
const AP_IPV4: [u8; 4] = [192, 168, 4, 1];
#[cfg(feature = "softap")]
const CAPTIVE_PORTAL_HOST: &str = "192.168.4.1";
const CAPTIVE_PORTAL_URL: &str = "http://192.168.4.1/";
#[cfg(feature = "softap")]
const CAPTIVE_PORTAL_API_URL: &str = "http://192.168.4.1/captive-portal/api";
#[cfg(feature = "radio-wifi")]
const HOPSPOT_CONFIG_OFFSET: u32 = 0xD000;
#[cfg(feature = "radio-wifi")]
const HOPSPOT_CONFIG_MAGIC: &[u8; 8] = b"HSPCFG1\0";
#[cfg(feature = "radio-wifi")]
const HOPSPOT_CONFIG_VERSION: u8 = 1;
#[cfg(feature = "radio-wifi")]
const HOPSPOT_CONFIG_READ_WORDS: usize = 32;
#[cfg(feature = "radio-wifi")]
const HOPSPOT_CONFIG_SSID_MAX: usize = 32;
#[cfg(feature = "radio-wifi")]
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
/// they share slot 2. Under `ble-bringup` the BLE fleet takes the next slot; under `radio-wifi` the
/// ESP-NOW broadcast carrier (which rides the same WiFi radio) takes another.
const IFACES: usize =
    4 + cfg!(feature = "ble-bringup") as usize + cfg!(feature = "radio-wifi") as usize;
/// The WiFi fleet's member budget: a peer costs a descriptor + status slot, never a lane buffer.
const MEMBERS: usize = 24;
/// The engine-interface (descriptor + pacer) pool: the fixed interfaces plus the WiFi members.
/// Decoupled from the lane count `IFACES` on purpose: a member costs descriptors, not buffers.
const MAX_IFACES: usize = 3 + MEMBERS + cfg!(feature = "radio-wifi") as usize;
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
/// The BLE fleet's pool slot (after LoRa), present only under `ble-bringup`. Distinct from the WiFi
/// slot so both supervisors run at once when WiFi and BLE coexist.
#[cfg(feature = "ble-bringup")]
const BLE_FLEET_SLOT: usize = 4;
/// The ESP-NOW broadcast carrier's pool slot, after the BLE fleet when it is present. A 1:1 interface
/// like LoRa (not a fleet); present under `radio-wifi`, which brings up the WiFi radio it shares.
#[cfg(feature = "radio-wifi")]
const ESPNOW_SLOT: usize = 4 + cfg!(feature = "ble-bringup") as usize;
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

#[cfg(feature = "softap")]
use captive_portal::ap_ssid;
#[cfg(feature = "radio-wifi")]
use configuration::{hopspot_wifi_config, HopspotWifiConfig};
use connectivity::build_tcp;
#[cfg(feature = "radio-wifi")]
use connectivity::{build_wifi, espnow_channel_policy, EspNowAdapter};
#[cfg(feature = "radio-wifi")]
use display::build_interface_menu_details;
use display::{build_cards, build_snapshots, button_task};

/// The WiFi supervisor's shared aggregate + per-peer status (written + read on core 0).
static WIFI_SHARED: AutoWifiShared<MEMBERS> = AutoWifiShared::new(WIFI_FLEET_ID);

/// Under `ble-bringup` the BLE supervisor reuses the (WiFi-free) fleet slot 2, keyed by its own kind
/// so `BluetoothPeer` members route to it. The radio carries `BLE_MEMBERS` concurrent connections (the
/// pooled `ble.rs` backend sizes its slot pool + trouble-host `CONNECTIONS` to this) — 2 since the
/// reduced embedded MTU ceiling (1472) freed the internal lane RAM to carry a second peer.
#[cfg(feature = "ble-bringup")]
pub const BLE_MEMBERS: usize = limits::ESP32_S3_MAX_PEERS;
#[cfg(feature = "ble-bringup")]
const BLE_FLEET_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::BluetoothAuto as u8, 0, 0, 0, 0, 0, 0, 0]);
#[cfg(feature = "ble-bringup")]
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
#[cfg(feature = "ble-bringup")]
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
#[cfg(feature = "radio-wifi")]
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
    #[cfg(feature = "radio-wifi")]
    pub lora_radio: LoraRadio,
    #[cfg(feature = "radio-wifi")]
    pub wifi: esp_hal::peripherals::WIFI<'static>,
    pub button: esp_hal::peripherals::GPIO0<'static>,
    pub cpu_ctrl: esp_hal::peripherals::CPU_CTRL<'static>,
    pub sw_int1: esp_hal::interrupt::software::SoftwareInterrupt<'static, 1>,
    pub timebase: EmbassyTimebase,
    /// The RTC handle is kept alive for the whole run so its disabled watchdogs stay disabled.
    pub _rtc: esp_hal::rtc_cntl::Rtc<'static>,
    #[cfg(feature = "ble-bringup")]
    pub bt: esp_hal::peripherals::BT<'static>,
}

/// The per-board seam: the ~6% of an ESP32-S3 Hopspot that differs between boards (identity
/// strings, display driver + flush, battery source, power/pin bring-up). Everything else lives in
/// [`run_core`], so a shared-path change can never again rot one board while the other compiles.
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
#[cfg_attr(not(feature = "softap"), allow(dead_code))]
pub enum RadioMode {
    Ble,
    AccessPoint,
}

#[cfg(feature = "softap")]
const RADIO_MODE_AP: u32 = 0x4150_0001;
#[cfg(feature = "softap")]
const RADIO_MODE_BLE: u32 = 0x424C_4501;
#[cfg(feature = "softap")]
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RADIO_MODE_FLAG: u32 = 0;

fn boot_radio_mode(station_configured: bool) -> RadioMode {
    let _ = station_configured;
    #[cfg(feature = "softap")]
    {
        let flag = unsafe { core::ptr::addr_of!(RADIO_MODE_FLAG).read() };
        if flag == RADIO_MODE_AP {
            return RadioMode::AccessPoint;
        }
        RadioMode::Ble
    }
    #[cfg(not(feature = "softap"))]
    {
        RadioMode::Ble
    }
}

#[cfg(feature = "softap")]
fn request_radio_mode(mode: RadioMode) -> ! {
    let flag = match mode {
        RadioMode::AccessPoint => RADIO_MODE_AP,
        RadioMode::Ble => RADIO_MODE_BLE,
    };
    unsafe { core::ptr::addr_of_mut!(RADIO_MODE_FLAG).write(flag) };
    esp_hal::system::software_reset();
}

pub async fn run<B: Esp32S3Board>(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);
    let bringup = B::bringup(p, &spawner).await;
    run_core::<B>(spawner, bringup).await;
}

/// Platform run on core 0: the self-identity crypto, the radios + WiFi/TCP, and the I/O
/// run-loops + screen. The engine is built *and* owned by core 1 (the construction transient,
/// then the reactor, on its own stack), so core 0 never touches the node. Never returns.
#[allow(clippy::too_many_lines)]
pub async fn run_core<B: Esp32S3Board>(spawner: Spawner, b: Bringup<B::Display, B::Battery>) {
    log_heap_footprint("run_core entry (post-bringup, core 0)");
    let mut display = b.display;
    let oled_ok = b.oled_ok;
    let mut battery_source = b.battery;
    #[cfg(feature = "radio-wifi")]
    let wifi_config = hopspot_wifi_config();
    #[cfg(feature = "radio-wifi")]
    let station_configured = wifi_config.has_station();
    #[cfg(not(feature = "radio-wifi"))]
    let station_configured = false;
    let radio_mode = boot_radio_mode(station_configured);

    let usb_status = B::usb_status();
    let usb_id = usb_status.id();
    let (usb_rx, usb_tx) = UsbSerialJtag::new(b.usb_device).into_async().split();

    let mac = base_mac_address();
    let mut mac_octets = [0u8; 6];
    mac_octets.copy_from_slice(&mac.as_bytes()[..6]);
    let secret_key = fixture_identity_secret_key(&mac);

    let transport_secret = secret_key.clone();
    let self_destination = {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = personal_rns::routing::announce::expand_name("lxmf", &["delivery"])
            .expect("valid name");
        personal_rns::routing::announce::derive_destination_hash(&signer.identity_hash(), &name)
    };
    let seed = self_destination.as_bytes();
    ENTROPY_STATE.store(
        u64::from_le_bytes([
            seed[0], seed[1], seed[2], seed[3], seed[4], seed[5], seed[6], seed[7],
        ]) | 1,
        Ordering::Relaxed,
    );

    let mut inbound: ReactorInbound = HVec::new();
    let mut egress_lanes: ReactorEgressLanes = HVec::new();
    let mut iface_halves: [Option<(
        EmbassyGrantProducer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
        EmbassyGrantConsumer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
    )>; IFACES] = [const { None }; IFACES];
    for slot in 0..IFACES {
        let in_ch = IN_CH[slot].init(zerocopy_channel::Channel::new(IN_BUF[slot].take()));
        let (in_producer, in_consumer) = embassy_grant_lane(in_ch);
        let out_ch = OUT_CH[slot].init(zerocopy_channel::Channel::new(OUT_BUF[slot].take()));
        let (mut out_producer, out_consumer) = embassy_grant_lane(out_ch);
        if slot == WIFI_FLEET_SLOT {
            out_producer.set_outbound_wake(&OUTBOUND_WAKE);
        }
        #[cfg(feature = "ble-bringup")]
        if slot == BLE_FLEET_SLOT {
            out_producer.set_outbound_wake(&BLE_OUTBOUND_WAKE);
        }
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        iface_halves[slot] = Some((in_producer, out_consumer));
    }

    #[cfg(feature = "radio-wifi")]
    let lora_radio = b.lora_radio;
    let lora_profile = DEFAULT_915_PROFILE;
    let lora_id = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&lora_profile));
    let lora_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new(lora_id, ConnectionState::Initializing)
    );
    #[cfg(feature = "radio-wifi")]
    let lora = LoRaInterface::new(
        lora_radio,
        lora_profile,
        &LORA_CONTROL,
        lora_status,
        LIFECYCLE.dyn_sender(),
    );

    // The WiFi stack carries both the WiFi-auto UDP and the TCP client, so it stands up before the
    // node moves to core 1 — activating the TCP slot is a core-0-only act.
    #[cfg(feature = "radio-wifi")]
    let (wifi, tcp_stack, esp_now) = build_wifi(
        &spawner,
        b.wifi,
        mac_octets,
        &wifi_config,
        radio_mode == RadioMode::AccessPoint,
    );
    #[cfg(not(feature = "radio-wifi"))]
    let wifi: Option<AutoWifi<'static, MEMBERS>> = None;
    #[cfg(not(feature = "radio-wifi"))]
    let tcp_stack: Option<Stack<'static>> = None;

    #[cfg(feature = "radio-wifi")]
    let espnow_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new(espnow_core::interface_id(), ConnectionState::Initializing)
    );
    #[cfg(feature = "radio-wifi")]
    let espnow = esp_now.map(|radio| {
        EspNowInterface::new(
            EspNowAdapter::new(radio),
            espnow_channel_policy(station_configured),
            espnow_status,
        )
    });

    let tcp_built = tcp_stack.and_then(build_tcp);
    let tcp_status = tcp_built.as_ref().map(|(_, status, _)| *status);
    let tcp_id = tcp_built.as_ref().map(|(_, _, id)| *id);

    let handle: Handle = PrnsNodeHandle::new(COMMANDS.sender(), &COMPLETION);
    let plumbing = ReactorPlumbing::new(
        inbound,
        PooledEgress::new(egress_lanes),
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new_with_timebase(b.timebase, seeded_entropy as fn(&mut [u8]));

    let recipe = PrnsNodeRecipe {
        transport_identity: Some(transport_secret),
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "lxmf",
            aspects: &["delivery"],
            identity: secret_key,
            announce_app_data: B::ANNOUNCE_APP_DATA,
            proof: personal_rns::routing::ProofStrategy::ProveAll,
            link_requests: personal_rns::routing::LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::Ratcheted,
            request_handlers: RequestHandlerRegistration::None,
        }],
        app_state: (),
        storage: EngineStorageType::default(),
        routes: personal_rns::routes![],
        interfaces: personal_rns::runtime::Manual,
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };

    #[cfg(feature = "radio-wifi")]
    let lora_cfg = lora.descriptor();
    #[cfg(feature = "radio-wifi")]
    let espnow_cfg = espnow.as_ref().map(|e| e.descriptor());
    let tcp_cfg = tcp_built.as_ref().map(|(t, _, _)| t.descriptor());
    let has_wifi = wifi.is_some();

    // The engine is built and run on core 1: its stack carries the dalek-heavy construction
    // transient, then the reactor reuses that space (see `CORE1_STACK_BYTES`).
    let core1_stack = mk_static!(CpuStack<CORE1_STACK_BYTES>, CpuStack::new());
    esp_rtos::start_second_core(b.cpu_ctrl, b.sw_int1, core1_stack, move || {
        static NODE: StaticCell<S3Node> = StaticCell::new();
        let node: &'static mut S3Node =
            NODE.init_with(|| PrnsNode::new(recipe, plumbing, host, HVec::new()));
        node.activate(USB_SLOT, device_descriptor(usb_id));
        if let Some(cfg) = tcp_cfg {
            node.activate(TCP_SLOT, cfg);
        }
        #[cfg(all(feature = "radio-wifi", not(feature = "ap-test")))]
        node.activate(LORA_SLOT, lora_cfg);
        #[cfg(all(feature = "radio-wifi", not(feature = "ap-test")))]
        if let Some(cfg) = espnow_cfg {
            node.activate(ESPNOW_SLOT, cfg);
        }
        #[cfg(feature = "radio-wifi")]
        if has_wifi {
            node.activate_fleet(WIFI_FLEET_SLOT, WIFI_FLEET_ID);
        }
        #[cfg(feature = "ble-bringup")]
        if radio_mode == RadioMode::Ble {
            node.activate_fleet(BLE_FLEET_SLOT, BLE_FLEET_ID);
        }
        log_heap_footprint("post-construction (engine columns boxed into PSRAM)");

        static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
        EXECUTOR
            .init(esp_rtos::embassy::Executor::new())
            .run(|spawner| {
                spawner.spawn(reactor_core(node).expect("reactor task fits"));
            })
    });

    let usb_seam = {
        let (in_producer, out_consumer) = iface_halves[USB_SLOT].take().expect("usb slot half");
        EmbassyInterfaceSeam::new(usb_id, in_producer, NOTIFY.sender(), out_consumer)
    };
    spawner.spawn(
        usb_device_task(usb_rx, usb_tx, usb_seam, usb_id, usb_status).expect("usb task fits"),
    );

    #[cfg(feature = "radio-wifi")]
    let lora_seam = {
        let (lora_in_producer, lora_out_consumer) =
            iface_halves[LORA_SLOT].take().expect("lora slot half");
        EmbassyInterfaceSeam::new(
            lora_id,
            lora_in_producer,
            NOTIFY.sender(),
            lora_out_consumer,
        )
    };

    #[cfg(feature = "radio-wifi")]
    let espnow = espnow.map(|interface| {
        let (in_producer, out_consumer) =
            iface_halves[ESPNOW_SLOT].take().expect("espnow slot half");
        let seam =
            EmbassyInterfaceSeam::new(interface.id(), in_producer, NOTIFY.sender(), out_consumer);
        (interface, seam)
    });

    let tcp = tcp_built.map(|(tcp, _, _)| {
        let (in_producer, out_consumer) = iface_halves[TCP_SLOT].take().expect("tcp slot half");
        let seam = EmbassyInterfaceSeam::new(tcp.id(), in_producer, NOTIFY.sender(), out_consumer);
        (tcp, seam)
    });

    #[cfg(feature = "radio-wifi")]
    let wifi_fleet: Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP> = {
        let (in_producer, out_consumer) = iface_halves[WIFI_FLEET_SLOT]
            .take()
            .expect("wifi fleet half");
        Fleet::new(
            FleetWire {
                inbound: in_producer,
                outbound: out_consumer,
                notify: NOTIFY.sender(),
                outbound_wake: &OUTBOUND_WAKE,
            },
            LIFECYCLE.sender(),
        )
    };
    // The WiFi-auto run loop's two MTU receive buffers live on the heap (the D-cache donation),
    // not on the bounded `#[esp_rtos::main]` stack that run()'s future rides; the alloc-free
    // embassy AutoWifi just borrows them. Leaked: they live for the program's whole life anyway.
    #[cfg(feature = "radio-wifi")]
    let wifi_data_buf: &'static mut [u8] = alloc::vec![0u8; wifi_core::HARDWARE_MTU].leak();
    #[cfg(feature = "radio-wifi")]
    let wifi_sec_data_buf: &'static mut [u8] = alloc::vec![0u8; wifi_core::HARDWARE_MTU].leak();
    #[cfg(feature = "ble-bringup")]
    let ble_fleet: Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP> = {
        let (in_producer, out_consumer) =
            iface_halves[BLE_FLEET_SLOT].take().expect("ble fleet half");
        Fleet::new(
            FleetWire {
                inbound: in_producer,
                outbound: out_consumer,
                notify: NOTIFY.sender(),
                outbound_wake: &BLE_OUTBOUND_WAKE,
            },
            LIFECYCLE.sender(),
        )
    };

    let button = Input::new(b.button, InputConfig::default().with_pull(Pull::Up));
    spawner.spawn(button_task(button).expect("button task fits"));

    let wifi_status = wifi.as_ref().map(AutoWifi::status);
    let wifi_id = wifi_status.as_ref().map(|status| {
        use personal_rns::interfaces::InterfaceStatus;
        status.id()
    });

    #[cfg(feature = "radio-wifi")]
    let espnow_card_id = espnow.as_ref().map(|(interface, _)| interface.id());
    #[cfg(feature = "radio-wifi")]
    let espnow_card_status = espnow_card_id.map(|_| espnow_status);
    #[cfg(not(feature = "radio-wifi"))]
    let (espnow_card_id, espnow_card_status): (
        Option<InterfaceId>,
        Option<&'static EmbassyInterfaceStatus>,
    ) = (None, None);

    let render = async move {
        let mut ui_state = screen::UiState::new();
        ui_state.set_storage_limits(<EngineStorageType as StorageLayout>::LIMITS);
        ui_state.set_display_power_capable(oled_ok);
        ui_state.set_radio_state(
            cfg!(feature = "softap"),
            radio_mode == RadioMode::AccessPoint,
        );
        let mut working_lora_profile = DEFAULT_915_PROFILE;
        let mut battery_state = screen::BatteryState::Unknown;
        let mut battery_gauge = screen::BatteryGauge::lipo();
        #[cfg(feature = "softap")]
        let ap_footer_ssid = (radio_mode == RadioMode::AccessPoint).then(ap_ssid);
        #[cfg(feature = "softap")]
        let site_footer = ap_footer_ssid.as_deref().map(|ssid| {
            screen::UiFooter::with_lines(
                "WifiAP",
                Some(ssid),
                Some("docs @"),
                Some(CAPTIVE_PORTAL_HOST),
            )
        });
        #[cfg(not(feature = "softap"))]
        let site_footer = None;
        let has_site_footer = site_footer.is_some();
        let mut ticks_to_battery: u8 = 0;
        let mut activity = screen::CardActivityTracker::<8>::new();
        let mut notice_until_ms: Option<u64> = None;
        let mut oled_awake = true;
        let mut oled_off_at_ms: Option<u64> = None;
        let mut oled_sleep_at_ms: Option<u64> = None;
        let mut render_tick = Ticker::every(RENDER_INTERVAL);
        let mut settle_after_draw = false;
        loop {
            if ticks_to_battery == 0 {
                battery_state = battery_gauge.sample(&mut battery_source);
                ticks_to_battery = RENDER_TICKS_PER_BATTERY;
            }

            let snapshots = build_snapshots(
                usb_status,
                wifi_status.as_ref(),
                tcp_status,
                lora_status,
                espnow_card_status,
            );
            let mut cards = build_cards(
                &snapshots,
                usb_status.id(),
                wifi_id,
                tcp_id,
                lora_status.id(),
                espnow_card_id,
            );
            let now_ms = embassy_time::Instant::now().as_millis();
            let activity_secs = (now_ms / 1000).min(u64::from(u32::MAX)) as u32;
            activity.update(&mut cards, activity_secs);
            let card_count = cards.len();
            #[cfg(all(feature = "radio-wifi", feature = "softap"))]
            let menu_ap_ssid = ap_footer_ssid.as_deref();
            #[cfg(all(feature = "radio-wifi", not(feature = "softap")))]
            let menu_ap_ssid = None;
            #[cfg(feature = "radio-wifi")]
            let interface_menu_details = build_interface_menu_details(
                ui_state
                    .selected_card(card_count)
                    .and_then(|index| cards.get(index)),
                &snapshots,
                usb_status,
                &wifi_config,
                menu_ap_ssid,
            );
            #[cfg(not(feature = "radio-wifi"))]
            let interface_menu_details = screen::InterfaceMenuDetailRows::new();
            ui_state.sync_card_count_with_footer(card_count, has_site_footer);
            if notice_until_ms.is_some_and(|until| now_ms >= until) {
                ui_state.clear_notice();
                notice_until_ms = None;
            }
            if let Some(off_at) = oled_off_at_ms {
                if oled_awake && now_ms >= off_at {
                    B::set_display_awake(&mut display, false);
                    oled_awake = false;
                    oled_off_at_ms = None;
                    ui_state.clear_notice();
                    notice_until_ms = None;
                }
            }
            if let Some(sleep_at) = oled_sleep_at_ms {
                if oled_awake && now_ms >= sleep_at {
                    B::set_display_awake(&mut display, false);
                    oled_awake = false;
                }
            }
            if oled_ok && oled_awake {
                screen::draw_with_state_footer_details_at(
                    &mut display,
                    &cards,
                    battery_state,
                    &ui_state,
                    site_footer,
                    &interface_menu_details,
                    now_ms,
                );
                B::flush(&mut display);
            }
            if settle_after_draw {
                Timer::after(Duration::from_millis(screen::COALESCE_MS)).await;
                settle_after_draw = false;
            }

            match select3(
                INTERFACE_STORE.changed(),
                BUTTON_EVENTS.receive(),
                render_tick.next(),
            )
            .await
            {
                Either3::First(()) => {
                    settle_after_draw = true;
                }
                Either3::Third(()) => {
                    ticks_to_battery = ticks_to_battery.saturating_sub(1);
                }
                Either3::Second(event) => {
                    let now_ms = embassy_time::Instant::now().as_millis();
                    if !oled_awake && oled_sleep_at_ms.is_none() {
                        if oled_ok {
                            B::set_display_awake(&mut display, true);
                            oled_awake = true;
                        }
                        oled_off_at_ms = None;
                        ui_state.show_notice(screen::UiNotice::Awake);
                        notice_until_ms = Some(now_ms + NOTICE_MS);
                        continue;
                    }
                    oled_off_at_ms = None;
                    let selected_kind = ui_state
                        .selected_card(card_count)
                        .and_then(|index| cards.get(index))
                        .map(|card| card.kind);
                    match ui_state.handle_input_with_footer(
                        event,
                        card_count,
                        has_site_footer,
                        selected_kind,
                    ) {
                        screen::UiAction::OledOff => {
                            ui_state.show_notice(screen::UiNotice::OledOff);
                            notice_until_ms = Some(now_ms + NOTICE_MS);
                            oled_off_at_ms = Some(now_ms + NOTICE_MS);
                        }
                        screen::UiAction::Sleep => {
                            ui_state.show_notice(screen::UiNotice::Sleeping);
                            notice_until_ms = Some(now_ms + NOTICE_MS);
                            oled_sleep_at_ms = Some(now_ms + OLED_SLEEP_DELAY_MS);
                            usb_status.set_enabled(false);
                            lora_status.set_enabled(false);
                            if let Some(status) = wifi_status.as_ref() {
                                status.set_enabled(false);
                            }
                            if let Some(status) = espnow_card_status {
                                status.set_enabled(false);
                            }
                            if let Some(tcp) = tcp_status {
                                tcp.set_enabled(false);
                            }
                            #[cfg(feature = "ble-bringup")]
                            {
                                let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                status.set_enabled(false);
                            }
                        }
                        screen::UiAction::Wake => {
                            oled_off_at_ms = None;
                            oled_sleep_at_ms = None;
                            if oled_ok && !oled_awake {
                                B::set_display_awake(&mut display, true);
                                oled_awake = true;
                            }
                            ui_state.show_notice(screen::UiNotice::Awake);
                            notice_until_ms = Some(now_ms + NOTICE_MS);
                            usb_status.set_enabled(true);
                            lora_status.set_enabled(true);
                            if let Some(status) = wifi_status.as_ref() {
                                status.set_enabled(true);
                            }
                            if let Some(status) = espnow_card_status {
                                status.set_enabled(true);
                            }
                            if let Some(tcp) = tcp_status {
                                tcp.set_enabled(true);
                            }
                            #[cfg(feature = "ble-bringup")]
                            {
                                let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                status.set_enabled(true);
                            }
                        }
                        screen::UiAction::Announce => {
                            ui_state.show_notice(screen::UiNotice::Announcing);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            let _ = handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
                                destination: self_destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Registered,
                            }));
                        }
                        screen::UiAction::ToggleSelectedInterface => {
                            if let Some(card) = ui_state
                                .selected_card(card_count)
                                .and_then(|index| cards.get(index))
                            {
                                let mut handled = false;
                                let mut show_toggle_notice = |enabled: bool| {
                                    ui_state.show_notice(if enabled {
                                        screen::UiNotice::TurningOff
                                    } else {
                                        screen::UiNotice::TurningOn
                                    });
                                    notice_until_ms =
                                        Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                                };
                                if card.id == usb_status.id() {
                                    show_toggle_notice(usb_status.is_enabled());
                                    usb_status.set_enabled(!usb_status.is_enabled());
                                    handled = true;
                                }
                                if !handled && card.id == lora_status.id() {
                                    show_toggle_notice(lora_status.is_enabled());
                                    lora_status.set_enabled(!lora_status.is_enabled());
                                    handled = true;
                                }
                                if !handled {
                                    if let Some(status) = wifi_status.as_ref() {
                                        if card.id == status.id() {
                                            show_toggle_notice(status.is_enabled());
                                            status.set_enabled(!status.is_enabled());
                                            handled = true;
                                        }
                                    }
                                }
                                if !handled && Some(card.id) == espnow_card_id {
                                    if let Some(status) = espnow_card_status {
                                        show_toggle_notice(status.is_enabled());
                                        status.set_enabled(!status.is_enabled());
                                        handled = true;
                                    }
                                }
                                if !handled {
                                    if let (Some(tcp), Some(tcp_id)) = (tcp_status, tcp_id) {
                                        if card.id == tcp_id {
                                            show_toggle_notice(tcp.is_enabled());
                                            tcp.set_enabled(!tcp.is_enabled());
                                            #[cfg(feature = "ble-bringup")]
                                            {
                                                handled = true;
                                            }
                                        }
                                    }
                                }
                                #[cfg(feature = "ble-bringup")]
                                if !handled && card.id == BLE_FLEET_ID {
                                    let status = BluetoothAutoStatus::new(&BLE_SHARED);
                                    show_toggle_notice(status.is_enabled());
                                    status.set_enabled(!status.is_enabled());
                                }
                            }
                        }
                        screen::UiAction::OpenLoRaEditor => {
                            ui_state.open_lora_editor(working_lora_profile);
                        }
                        screen::UiAction::SetLoRaProfile(profile) => {
                            ui_state.show_notice(screen::UiNotice::Saved);
                            notice_until_ms =
                                Some(embassy_time::Instant::now().as_millis() + NOTICE_MS);
                            working_lora_profile = profile;
                            LORA_CONTROL.signal(profile);
                        }
                        screen::UiAction::SwapRadioMode => {
                            #[cfg(feature = "softap")]
                            {
                                let next = match radio_mode {
                                    RadioMode::Ble => RadioMode::AccessPoint,
                                    RadioMode::AccessPoint => RadioMode::Ble,
                                };
                                request_radio_mode(next);
                            }
                        }
                        screen::UiAction::OpenDocs => {}
                        screen::UiAction::None => {}
                    }
                }
            }
        }
    };

    #[cfg(all(feature = "ble-bringup", not(feature = "radio-wifi")))]
    // Halve the BLE controller task stack (8192 -> 4096; esp-radio's own default hints "4096?") to
    // reclaim ~4 KiB internal SRAM toward the full radio stack + SoftAP fit.
    let ble_connector = esp_radio::ble::controller::BleConnector::new(
        b.bt,
        esp_radio::ble::Config::default().with_task_stack_size(4096),
    )
    .expect("ble connector");

    #[cfg(all(feature = "ble-bringup", not(feature = "radio-wifi")))]
    {
        let _ = (wifi, tcp, has_wifi);
        join(
            crate::ble::run(ble_connector, mac_octets, ble_fleet, &BLE_SHARED),
            render,
        )
        .await;
    }
    #[cfg(all(feature = "radio-wifi", not(feature = "ble-bringup")))]
    {
        #[cfg(not(feature = "ap-test"))]
        let lora_run = lora.run(lora_seam);
        #[cfg(feature = "ap-test")]
        let lora_run = async {
            let _ = (lora, lora_seam);
        };
        #[cfg(not(feature = "ap-test"))]
        let espnow_run = async {
            if let Some((interface, seam)) = espnow {
                interface.run(seam).await;
            }
        };
        #[cfg(feature = "ap-test")]
        let espnow_run = async {
            let _ = espnow;
        };
        match (wifi, tcp) {
            (Some(wifi), Some((tcp, tcp_seam))) => {
                join(
                    join(
                        join(lora_run, espnow_run),
                        join(
                            wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                            tcp.run(tcp_seam),
                        ),
                    ),
                    render,
                )
                .await;
            }
            (Some(wifi), None) => {
                join(
                    join(
                        join(lora_run, espnow_run),
                        wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                    ),
                    render,
                )
                .await;
            }
            (None, _) => {
                join(join(lora_run, espnow_run), render).await;
            }
        }
    }
    #[cfg(all(feature = "ble-bringup", feature = "radio-wifi"))]
    {
        #[cfg(not(feature = "ap-test"))]
        let lora_run = lora.run(lora_seam);
        #[cfg(feature = "ap-test")]
        let lora_run = async {
            let _ = (lora, lora_seam);
        };
        #[cfg(not(feature = "ap-test"))]
        let espnow_run = async {
            if let Some((interface, seam)) = espnow {
                interface.run(seam).await;
            }
        };
        #[cfg(feature = "ap-test")]
        let espnow_run = async {
            let _ = espnow;
        };
        match radio_mode {
            RadioMode::Ble => {
                log_heap_footprint("pre-ble-connector (core 0)");
                let ble_connector = esp_radio::ble::controller::BleConnector::new(
                    b.bt,
                    esp_radio::ble::Config::default().with_task_stack_size(4096),
                )
                .expect("ble connector");
                log_heap_footprint("post-ble-connector (core 0)");
                let ble_run = crate::ble::run(ble_connector, mac_octets, ble_fleet, &BLE_SHARED);
                match (wifi, tcp) {
                    (Some(wifi), Some((tcp, tcp_seam))) => {
                        join(
                            join(join(join(ble_run, lora_run), espnow_run), tcp.run(tcp_seam)),
                            join(
                                wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                                render,
                            ),
                        )
                        .await;
                    }
                    (Some(wifi), None) => {
                        join(
                            join(join(ble_run, lora_run), espnow_run),
                            join(
                                wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                                render,
                            ),
                        )
                        .await;
                    }
                    (None, _) => {
                        join(join(join(ble_run, lora_run), espnow_run), render).await;
                    }
                }
            }
            RadioMode::AccessPoint => {
                let _ = (b.bt, ble_fleet);
                match (wifi, tcp) {
                    (Some(wifi), Some((tcp, tcp_seam))) => {
                        join(
                            join(
                                join(lora_run, espnow_run),
                                join(
                                    wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                                    tcp.run(tcp_seam),
                                ),
                            ),
                            render,
                        )
                        .await;
                    }
                    (Some(wifi), None) => {
                        join(
                            join(
                                join(lora_run, espnow_run),
                                wifi.run(wifi_fleet, wifi_data_buf, wifi_sec_data_buf),
                            ),
                            render,
                        )
                        .await;
                    }
                    (None, _) => {
                        join(join(lora_run, espnow_run), render).await;
                    }
                }
            }
        }
    }
}

/// Core 1: run only the engine reactor over the slot pool. The node was built on core 0 and lives in
/// a `static`; core 1 borrows it by `&'static mut`, so only a pointer crosses the core boundary (the
/// engine never moves) and this core needs just a small per-poll stack for the ingest crypto.
#[embassy_executor::task]
async fn reactor_core(node: &'static mut S3Node) {
    node.run_reactor_with_interface_store(&INTERFACE_STORE)
        .await
}

/// A bring-up fixture identity (the oracle X25519 0x22 ‖ Ed25519 0x11 keypair with the board MAC
/// mixed in so every flashed board is distinct). NEVER ship: predictable from the MAC.
fn fixture_identity_secret_key(
    mac: &esp_hal::efuse::MacAddress,
) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);
    for (i, byte) in mac.as_bytes().iter().enumerate() {
        secret_key[i] ^= byte;
        secret_key[32 + i] ^= byte;
    }
    secret_key
}
