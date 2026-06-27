use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::{Input, InputConfig, Output, Pull};
use esp_hal::rng::Rng;
use esp_hal::spi::master::Spi;
use esp_hal::system::Stack as CpuStack;
use esp_println::println;

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
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
#[cfg(feature = "ble-bringup")]
use personal_rns::interfaces::bluetooth_auto::{BluetoothAutoShared, BluetoothAutoStatus};
#[cfg(feature = "radio-wifi")]
use personal_rns::interfaces::esp_now::{
    core as espnow_core, Channel as EspNowChannel, ChannelPolicy, EspNowInterface,
};
use personal_rns::interfaces::rns_parity::lora::core::{channel_tag, DEFAULT_915_PROFILE};
use personal_rns::interfaces::rns_parity::lora::impls::embassy::{LoRaControl, LoRaInterface};
use personal_rns::interfaces::rns_parity::tcp::client::embassy::TcpClient;
use personal_rns::interfaces::rns_parity::wifi_auto::core as wifi_core;
use personal_rns::interfaces::rns_parity::wifi_auto::{AutoWifi, AutoWifiShared, AutoWifiStatus};
use personal_rns::interfaces::substrate::EmbassyTimebase;
use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceSnapshot, InterfaceStatus, MacAddress,
    Membership,
};
use personal_rns::reactor::grant::FrameSlot;
use personal_rns::reactor::impls::embassy_reactor::{
    embassy_grant_lane, EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost,
    EmbassyInterfaceSeam, EmbassyInterfaceStatus, InterfaceLifecycle, PooledEgress,
};
use personal_rns::reactor::interface_seam::{Interface, EMBEDDED_MAX_WIRE_FRAME_LEN};
use personal_rns::runtime::{
    CompletionPool, EmbassyInterfaceStore, EmbassyPrnsHandle, Fleet, MemberWire,
    PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe, ReactorPlumbing,
};
use personal_rns::storage::StorageLayout;
use personal_rns::subghz_rf::Sx126x;
use personal_rns::wire::TransportId;

use crate::engine_storage::EngineStorageType;

use personal_hopspot_ui as screen;

esp_app_desc!();

#[cfg(feature = "softap")]
mod hopspot_site {
    include!(concat!(env!("OUT_DIR"), "/hopspot_site.rs"));
}

#[cfg(feature = "softap")]
const AP_IPV4: [u8; 4] = [192, 168, 4, 1];
#[cfg(feature = "softap")]
const CAPTIVE_PORTAL_URL: &str = "http://192.168.4.1/";
#[cfg(feature = "softap")]
const CAPTIVE_PORTAL_API_URL: &str = "http://192.168.4.1/captive-portal/api";

/// The WiFi network the board joins (station mode), read at build time. Export them (e.g.
/// `source .wifi-env`) before `cargo heltec-v4`; an unset SSID leaves WiFi down, board runs USB-only.
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
const TCP_BITRATE_BPS: u32 = 65_000_000;
/// One TCP socket's smoltcp rx/tx buffer — sized for the board's frames, DRAM-frugal over throughput.
const TCP_SOCKET_BUF: usize = 1_024;

/// One lane per top-level driver: USB (slot 0), the TCP client (slot 1), the WiFi supervisor's one
/// shared fleet lane (slot 2), and the LoRa SX1262 (slot 3). WiFi members do NOT each take a lane —
/// they share slot 2. Under `ble-bringup` the BLE fleet takes the next slot; under `radio-wifi` the
/// ESP-NOW broadcast carrier (which rides the same WiFi radio) takes another.
const IFACES: usize =
    4 + cfg!(feature = "ble-bringup") as usize + cfg!(feature = "radio-wifi") as usize;
/// The WiFi fleet's member budget: how many peers the supervisor carries at once. Each costs only a
/// descriptor + a status slot, never a lane buffer, so it is sized generously.
const MEMBERS: usize = 24;
/// The engine-interface (descriptor + pacer) pool: the fixed interfaces (USB, TCP, LoRa, plus ESP-NOW
/// under `radio-wifi`) and the WiFi members. Distinct from the lane count `IFACES` — decoupling them
/// is the whole point of the shared lane, so a generous member budget costs descriptors, not buffers.
const MAX_IFACES: usize = 3 + MEMBERS + cfg!(feature = "radio-wifi") as usize;
/// The WiFi supervisor's fleet lane (slot 2) key: an `AutoWifi`-kind id, so every `WifiPeer` child
/// routes to this one lane by the kind byte (`lane_serves`). Also the WiFi card's aggregate id.
const WIFI_FLEET_ID: InterfaceId =
    InterfaceId::new([InterfaceKind::AutoWifi as u8, 0, 0, 0, 0, 0, 0, 0]);
/// The fleet lane's pool slot, after USB (0) and TCP (1).
const WIFI_FLEET_SLOT: usize = 2;
const LANE_DEPTH: usize = 1;
/// Slot 1: the always-on TCP client wire (parallel to USB at slot 0), so the WiFi members never
/// claim it.
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
type Handle = EmbassyPrnsHandle<'static, Mtx, COMMANDS_CAP, COMPLETIONS_CAP>;
/// The fully-spelled node type, so it can ride to core 1 as a concrete `#[task]` argument — which
/// is why `on_event` is a fn pointer and the host's entropy is a fn pointer, not closures.
type S3Node = Prns<
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

/// The WiFi supervisor's shared aggregate + per-peer status (written + read on core 0).
static WIFI_SHARED: AutoWifiShared<MEMBERS> = AutoWifiShared::new(WIFI_FLEET_ID);

/// Under `ble-bringup` the BLE supervisor reuses the (WiFi-free) fleet slot 2, keyed by its own kind
/// so `BluetoothPeer` members route to it. The radio carries `BLE_MEMBERS` concurrent connections (the
/// pooled `ble.rs` backend sizes its slot pool + trouble-host `CONNECTIONS` to this) — 2 since the
/// reduced embedded MTU ceiling (1472) freed the internal lane RAM to carry a second peer.
#[cfg(feature = "ble-bringup")]
pub const BLE_MEMBERS: usize = 2;
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
static INTERFACE_COUNTS: EmbassyInterfaceStore<Mtx, INTERFACE_STORE_CAP> =
    EmbassyInterfaceStore::new();
const INTERFACE_STORE_CAP: usize = 32;

/// The engine's entropy: the hardware TRNG blocks until WiFi RF is live (wifi::new enables it, but
/// the radio is not associated when the engine starts), so entropy is a board-unique software PRNG
/// over this `static` state. Acceptable ONLY because this whole identity is a NEVER-ship bring-up
/// fixture; the long-term fix is to gate the TRNG on RF-up. A fn (not a closure) so the host type
/// stays nameable for the cross-core move.
static ENTROPY_STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);

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

/// Print the allocator's per-region high-water footprint over the boot log: the `External` region's
/// size is the PSRAM the chip mapped (2 MiB vs 8 MiB), its `used` is the live cost of the engine's
/// boxed columns, the `Internal` region is the 56 KiB SRAM heap, and `Max usage` is the high-water
/// across both since boot. Safe only before the USB interface claims the USB-serial-JTAG, so it is a
/// construction-time probe, never a run-loop one.
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

/// Everything [`Esp32S3Board::bringup`] hands the shared core: the board-built peripherals (display,
/// battery, radio) plus the leftover singletons the core still wires up itself. Owning `Peripherals`
/// in `bringup` (rather than reaching into it from the generic core) is what lets each board move out
/// the *different* GPIO/I2C/ADC fields it needs — esp-hal singletons can't be partially moved through
/// a borrow, so the board takes the whole `Peripherals` and returns what is left here.
pub struct Bringup<D, B> {
    pub display: D,
    pub oled_ok: bool,
    pub battery: B,
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

/// The per-board seam: the ~6% of an ESP32-S3 Hopspot that actually differs between boards (its
/// identity strings, its display driver + flush, its battery source, and the power/pin bring-up).
/// Everything else lives in [`run_core`], so a change to the shared engine/WiFi/render path can never
/// again rot one board while the other compiles (the SoftAP/`wifi.run` drift that motivated this).
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
    /// Own `Peripherals`: esp-hal singletons can't be partial-moved through a borrow, so the board
    /// (not the generic core) takes the whole set, brings up power/display/battery/SX1262, and hands
    /// the rest back in [`Bringup`]. Runs the shared early init via [`boot_common`].
    async fn bringup(
        p: esp_hal::peripherals::Peripherals,
        spawner: &Spawner,
    ) -> Bringup<Self::Display, Self::Battery>;
}

/// The identical ESP32-S3 early boot every board's `bringup` runs first: allocators (internal + PSRAM
/// + the reclaimed D-cache region), the RTOS timer, and the RTC with its watchdogs disabled for the
/// slow PSRAM-backed engine construction. A block expression (so its bindings escape macro hygiene)
/// that owns `$p`'s early peripherals and yields `(software_interrupt1, timebase, rtc)` — the bits the
/// board threads into [`Bringup`] for the shared core (core 1's interrupt, the engine clock, the
/// kept-alive RTC handle).
macro_rules! boot_common {
    ($p:ident, $banner:expr) => {{
        ::esp_println::logger::init_logger_from_env();
        ::esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 42 * 1024);
        ::esp_alloc::psram_allocator!($p.PSRAM, ::esp_hal::psram);
        $crate::esp32s3::reclaim_dcache_region();
        let timg0 = ::esp_hal::timer::timg::TimerGroup::new($p.TIMG0);
        let sw_int =
            ::esp_hal::interrupt::software::SoftwareInterruptControl::new($p.SW_INTERRUPT);
        ::esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
        let mut rtc = ::esp_hal::rtc_cntl::Rtc::new($p.LPWR);
        // The engine construction allocates + zeroes PSRAM-backed columns synchronously; PSRAM is
        // slow, so it can overrun the RTC watchdog's ~2s timeout. Disable RWDT/SWD over the boot.
        rtc.rwdt.disable();
        rtc.swd.disable();
        let timebase = ::personal_rns::interfaces::substrate::EmbassyTimebase::start_at(
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
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut RADIO_MODE_FLAG: u32 = 0;

fn boot_radio_mode() -> RadioMode {
    #[cfg(feature = "softap")]
    {
        let flag = unsafe { core::ptr::addr_of!(RADIO_MODE_FLAG).read() };
        if flag == RADIO_MODE_AP {
            RadioMode::AccessPoint
        } else {
            RadioMode::Ble
        }
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
        RadioMode::Ble => 0,
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

/// Platform run on core 0: the self-identity crypto, the radios + WiFi/TCP, and the I/O run-loops +
/// screen — everything an ESP32-S3 Hopspot does once its board (`B`) has brought its hardware up. The
/// engine is built *and* owned by core 1 — it constructs the node on its own stack (the dalek-heavy
/// transient) then runs the reactor there, so core 0 never touches the node. True parallelism
/// (engine ⊥ I/O) over the cross-core lane channels. Never returns: this frame is core 0's I/O drive.
#[allow(clippy::too_many_lines)]
pub async fn run_core<B: Esp32S3Board>(spawner: Spawner, b: Bringup<B::Display, B::Battery>) {
    let mut display = b.display;
    let oled_ok = b.oled_ok;
    let mut battery_source = b.battery;
    let radio_mode = boot_radio_mode();

    let usb_status = B::usb_status();
    usb_status.set_enabled(false);

    let mac = base_mac_address();
    let mut mac_octets = [0u8; 6];
    mac_octets.copy_from_slice(&mac.as_bytes()[..6]);
    let secret_key = fixture_identity_secret_key(&mac);

    let (self_destination, transport_id) = {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = personal_rns::routing::announce::expand_name("lxmf", &["delivery"])
            .expect("valid name");
        let destination = personal_rns::routing::announce::derive_destination_hash(
            &signer.identity_hash(),
            &name,
        );
        let transport = TransportId::new(*signer.identity_hash().as_bytes());
        (destination, transport)
    };
    #[cfg(feature = "ble-bringup")]
    let node_identity: [u8; 16] = *transport_id.as_bytes();
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
            espnow_channel_policy(),
            espnow_status,
        )
    });

    let tcp_built = tcp_stack.and_then(build_tcp);
    let tcp_status = tcp_built.as_ref().map(|(_, status, _)| *status);
    let tcp_id = tcp_built.as_ref().map(|(_, _, id)| *id);

    let handle: Handle = EmbassyPrnsHandle::new(COMMANDS.sender(), &COMPLETION);
    let plumbing = ReactorPlumbing::new(
        inbound,
        PooledEgress::new(egress_lanes),
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new_with_timebase(b.timebase, seeded_entropy as fn(&mut [u8]));

    let recipe = PrnsRecipe {
        transport: Some(transport_id),
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "lxmf",
            aspects: &["delivery"],
            identity: secret_key,
            announce_app_data: B::ANNOUNCE_APP_DATA,
            proof: personal_rns::routing::ProofStrategy::ProveAll,
            ratchet: RatchetPolicy::Ratcheted,
        }],
        app_state: (),
        storage: EngineStorageType::default(),
        routes: personal_rns::routes![],
        interfaces: personal_rns::interfaces![],
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };

    #[cfg(feature = "radio-wifi")]
    let lora_cfg = lora.descriptor();
    #[cfg(feature = "radio-wifi")]
    let espnow_cfg = espnow.as_ref().map(|e| e.descriptor());
    let tcp_cfg = tcp_built.as_ref().map(|(t, _, _)| t.descriptor());
    let has_wifi = wifi.is_some();

    // The engine is built and run on core 1: its stack carries the dalek-heavy construction transient
    // and then the reactor reuses that space (see `CORE1_STACK_BYTES`). Core 0 keeps only its I/O +
    // screen loop.
    let core1_stack = mk_static!(CpuStack<CORE1_STACK_BYTES>, CpuStack::new());
    esp_rtos::start_second_core(b.cpu_ctrl, b.sw_int1, core1_stack, move || {
        static NODE: StaticCell<S3Node> = StaticCell::new();
        let node: &'static mut S3Node =
            NODE.init_with(|| Prns::new(recipe, plumbing, host, HVec::new()));
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
        node.set_interface_store(&INTERFACE_COUNTS);
        log_heap_footprint("post-construction (engine columns boxed into PSRAM)");

        static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
        EXECUTOR
            .init(esp_rtos::embassy::Executor::new())
            .run(|spawner| {
                spawner.spawn(reactor_core(node).expect("reactor task fits"));
            })
    });

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
            MemberWire {
                inbound: in_producer,
                outbound: out_consumer,
                notify: NOTIFY.sender(),
                outbound_wake: &OUTBOUND_WAKE,
            },
            LIFECYCLE.sender(),
        )
    };
    // The WiFi-auto run loop's two MTU receive buffers live on the heap (the D-cache donation: internal
    // DMA SRAM, fast), not on the core-0 main-task stack. Folding the SoftAP segment in adds a second
    // 1196 B buffer + a deeper select to run()'s future, and that future rides the bounded main-task
    // stack (`#[esp_rtos::main]`). Boxing them off it relieves the stack while the alloc-free embassy
    // AutoWifi just borrows them. Leaked: they live for the program's whole life anyway.
    #[cfg(feature = "radio-wifi")]
    let wifi_data_buf: &'static mut [u8] = alloc::vec![0u8; wifi_core::HARDWARE_MTU].leak();
    #[cfg(feature = "radio-wifi")]
    let wifi_sec_data_buf: &'static mut [u8] = alloc::vec![0u8; wifi_core::HARDWARE_MTU].leak();
    #[cfg(feature = "ble-bringup")]
    let ble_fleet: Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP> = {
        let (in_producer, out_consumer) =
            iface_halves[BLE_FLEET_SLOT].take().expect("ble fleet half");
        Fleet::new(
            MemberWire {
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
        let site_footer = (radio_mode == RadioMode::AccessPoint)
            .then_some(screen::UiFooter::new("Docs site", Some("192.168.4.1")));
        #[cfg(not(feature = "softap"))]
        let site_footer = None;
        let has_site_footer = site_footer.is_some();
        let mut ticks_to_battery: u8 = 0;
        #[cfg(feature = "ble-bringup")]
        let mut ble_announce_ticks: u8 = 0;
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

            let mut cards = build_cards(
                usb_status,
                wifi_status.as_ref(),
                wifi_id,
                tcp_status,
                tcp_id,
                lora_status,
                lora_status.id(),
                espnow_card_status,
                espnow_card_id,
            );
            let now_ms = embassy_time::Instant::now().as_millis();
            let activity_secs = (now_ms / 1000).min(u64::from(u32::MAX)) as u32;
            activity.update(&mut cards, activity_secs);
            let card_count = cards.len();
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
                screen::draw_with_state_footer_at(
                    &mut display,
                    &cards,
                    battery_state,
                    &ui_state,
                    site_footer,
                    now_ms,
                );
                B::flush(&mut display);
            }
            if settle_after_draw {
                Timer::after(Duration::from_millis(screen::COALESCE_MS)).await;
                settle_after_draw = false;
            }

            match select3(
                INTERFACE_COUNTS.changed(),
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
                    #[cfg(feature = "ble-bringup")]
                    {
                        ble_announce_ticks += 1;
                        if ble_announce_ticks >= 60 {
                            ble_announce_ticks = 0;
                            let issued = handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
                                destination: self_destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Registered,
                            }));
                            log::info!("hopspot: auto-announce issued={}", issued.is_some());
                        }
                    }
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
            crate::ble::run(
                ble_connector,
                mac_octets,
                node_identity,
                ble_fleet,
                &BLE_SHARED,
            ),
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
                let ble_connector = esp_radio::ble::controller::BleConnector::new(
                    b.bt,
                    esp_radio::ble::Config::default().with_task_stack_size(4096),
                )
                .expect("ble connector");
                let ble_run = crate::ble::run(
                    ble_connector,
                    mac_octets,
                    node_identity,
                    ble_fleet,
                    &BLE_SHARED,
                );
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
                let _ = (b.bt, node_identity, ble_fleet);
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
    node.run_reactor().await
}

/// Build the card set: the USB host, the WiFi aggregate, and one card per confirmed peer —
/// classified into USB / WiFi / `Peer <hex>`, the same shape the desktop face renders.
fn build_cards(
    usb: &EmbassyInterfaceStatus,
    wifi: Option<&AutoWifiStatus<MEMBERS>>,
    wifi_id: Option<InterfaceId>,
    tcp: Option<&EmbassyInterfaceStatus>,
    tcp_id: Option<InterfaceId>,
    lora: &EmbassyInterfaceStatus,
    lora_id: InterfaceId,
    espnow: Option<&EmbassyInterfaceStatus>,
    espnow_id: Option<InterfaceId>,
) -> HVec<screen::Card, 8> {
    use personal_rns::interfaces::InterfaceStatus;
    let usb_id = usb.id();
    #[cfg(feature = "ble-bringup")]
    let ble = BluetoothAutoStatus::new(&BLE_SHARED);
    let classify = |id: InterfaceId| -> Option<(screen::CardKind, screen::CardLabel)> {
        if id == usb_id {
            Some((screen::CardKind::Usb, screen::card_label("USB")))
        } else if id == lora_id {
            Some((screen::CardKind::LoRa, screen::card_label("LoRa")))
        } else if Some(id) == wifi_id {
            Some((screen::CardKind::Wifi, screen::card_label("WiFi/LAN")))
        } else if Some(id) == espnow_id {
            Some((screen::CardKind::EspNow, screen::card_label("ESP-NOW")))
        } else if Some(id) == tcp_id {
            Some((
                screen::CardKind::Tcp,
                screen::tcp_card_label(HOPSPOT_TCP_TARGET),
            ))
        } else {
            #[cfg(feature = "ble-bringup")]
            if id == BLE_FLEET_ID {
                return Some((screen::CardKind::Ble, screen::card_label("BLE")));
            }
            let bytes = id.as_bytes();
            let mut label = screen::CardLabel::new();
            let _ = write!(label, "Peer {:02x}{:02x}", bytes[1], bytes[2]);
            Some((screen::CardKind::Peer, label))
        }
    };
    let mut entries: HVec<(&dyn InterfaceStatus, Membership), 8> = HVec::new();
    let _ = entries.push((usb, Membership::Independent));
    let _ = entries.push((lora, Membership::Independent));
    if let Some(espnow) = espnow {
        let _ = entries.push((espnow, Membership::Independent));
    }
    if let Some(tcp) = tcp {
        let _ = entries.push((tcp, Membership::Independent));
    }
    if let Some(wifi) = wifi {
        let supervisor_id = wifi.id();
        let _ = entries.push((wifi, Membership::Independent));
        for member in wifi.members() {
            let _ = entries.push((member, Membership::FleetMember { supervisor_id }));
        }
    }
    #[cfg(feature = "ble-bringup")]
    {
        let supervisor_id = ble.id();
        let _ = entries.push((&ble, Membership::Independent));
        for member in ble.members() {
            let _ = entries.push((member, Membership::FleetMember { supervisor_id }));
        }
    }
    let mut snapshots: HVec<InterfaceSnapshot, 8> = HVec::new();
    for (status, membership) in &entries {
        let id = status.id();
        let counts = INTERFACE_COUNTS.counts(id);
        let _ = snapshots.push(InterfaceSnapshot {
            id,
            connection: status.connection(),
            rx_bytes: status.rx_bytes(),
            tx_bytes: status.tx_bytes(),
            transfer_rates: status.transfer_rates(),
            destinations: counts.destinations,
            links: counts.links,
            transported_links: counts.transported_links,
            membership: *membership,
        });
    }
    screen::snapshots_to_cards(&snapshots, classify)
}

/// Stand the TCP client up from [`HOPSPOT_TCP_TARGET`] over the WiFi `stack`: parse its `ip:port`
/// (unset or unparseable leaves it down), mint the interface id and its status under the same key,
/// and lease the socket's smoltcp buffers from `static`s. Hands back the interface, its status
/// handle (the render reads it for the card), and its id (the classifier names it).
fn build_tcp(
    stack: Stack<'static>,
) -> Option<(
    TcpClient<'static>,
    &'static EmbassyInterfaceStatus,
    InterfaceId,
)> {
    let addr = HOPSPOT_TCP_TARGET.parse::<::core::net::SocketAddr>().ok()?;
    let target = IpEndpoint::new(addr.ip().into(), addr.port());
    let tag = HOPSPOT_TCP_TARGET.as_bytes();
    let id = TcpClient::interface_id(tag);
    let status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new(id, ConnectionState::Initializing)
    );
    let rx_buffer: &'static mut [u8] = mk_static!([u8; TCP_SOCKET_BUF], [0u8; TCP_SOCKET_BUF]);
    let tx_buffer: &'static mut [u8] = mk_static!([u8; TCP_SOCKET_BUF], [0u8; TCP_SOCKET_BUF]);
    let tcp = TcpClient::new(
        stack,
        target,
        tag,
        TCP_BITRATE_BPS,
        Duration::from_secs(5),
        rx_buffer,
        tx_buffer,
        status,
    );
    Some((tcp, status, id))
}

/// A random per-boot SoftAP SSID suffix, cached so every `set_config` within a boot reuses the same
/// name (regenerating per call would flap the SSID). 0 = unset. Random rather than MAC-derived so the
/// AP name leaks no device identity; it re-rolls on reboot, which is acceptable (preferred, even).
#[cfg(feature = "softap")]
static AP_SSID_SUFFIX: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "softap")]
fn ap_config() -> AccessPointConfig {
    let mut suffix = AP_SSID_SUFFIX.load(Ordering::Relaxed);
    if suffix == 0 {
        let mut r = [0u8; 2];
        Rng::new().read(&mut r);
        suffix = u64::from(u16::from_le_bytes(r)) | 1;
        AP_SSID_SUFFIX.store(suffix, Ordering::Relaxed);
    }
    let ssid = alloc::format!("Hopspot-{:04X}", suffix as u16);
    AccessPointConfig::default()
        .with_ssid(ssid)
        .with_max_connections(4)
}

/// The WiFi mode to request for a station config: APSTA (station + the SoftAP "Hopspot") when the
/// `softap` feature is on, plain station otherwise. Used at every `set_config` so the AP rides
/// alongside the station and survives reconnects — a bare `Station` set_config would drop the AP.
#[cfg(feature = "radio-wifi")]
#[cfg_attr(not(feature = "softap"), allow(unused_variables))]
fn station_wifi_mode(station: StationConfig, ap_enabled: bool) -> WifiConfig {
    #[cfg(feature = "softap")]
    if ap_enabled {
        return WifiConfig::AccessPointStation(station, ap_config());
    }
    WifiConfig::Station(station)
}

/// Stand a second embassy-net Stack on the AP netif and drive it, so the SoftAP is a real interface
/// (APSTA). Sized like the station's; the AP takes the station MAC + 1 for its link-local (matching
/// the SoftAP's own BSSID) so the two netifs are distinct.
#[cfg(feature = "softap")]
fn build_ap_netif(
    spawner: &Spawner,
    ap_iface: WifiStaDevice<'static>,
    mac: [u8; 6],
) -> Stack<'static> {
    let mut ap_mac = mac;
    ap_mac[5] = ap_mac[5].wrapping_add(1);
    let ap_link_local = wifi_core::link_local_from_mac(MacAddress::new(ap_mac));
    // The SoftAP is the gateway, not a DHCP client: a static IPv4 (192.168.4.1/24) lets it serve DHCP +
    // host the TCP rendezvous, plus the static v6 link-local for WiFi-auto's UDP. (The IPv4 multicast
    // path is moot anyway — the SoftAP can't pass multicast; see the rendezvous DHCP server below.)
    let mut ap_net_config = NetConfig::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 4, 1), 24),
        gateway: None,
        dns_servers: Default::default(),
    });
    ap_net_config.ipv6 = ConfigV6::Static(StaticConfigV6 {
        address: Ipv6Cidr::new(ap_link_local, 64),
        gateway: None,
        dns_servers: Default::default(),
    });
    let ap_resources = mk_static!(StackResources<10>, StackResources::new());
    let ap_seed = {
        let mut b = [0u8; 8];
        Rng::new().read(&mut b);
        u64::from_le_bytes(b)
    };
    let (ap_stack, ap_runner) = embassy_net::new(ap_iface, ap_net_config, ap_resources, ap_seed);
    spawner.spawn(net_task(ap_runner).expect("ap net task fits"));
    ap_stack
}

/// A minimal DHCPv4 server for the SoftAP. A device joining "Hopspot" DISCOVERs/REQUESTs and we lease it
/// 192.168.4.2 with the SoftAP (192.168.4.1) as its router + DNS. The lease is incidental; the *gateway*
/// is the point: once the joiner's default route is the Heltec, its WiFi-auto client auto-dials the TCP
/// rendezvous on the gateway (port 42699), sidestepping the SoftAP's broken multicast entirely. One
/// static lease is enough to start; the wire format is hand-rolled (embassy-net ships only a client).
#[cfg(feature = "softap")]
#[embassy_executor::task]
async fn dhcp_server_task(stack: Stack<'static>) -> ! {
    let rx_meta: &'static mut [PacketMetadata] = alloc::vec![PacketMetadata::EMPTY; 4].leak();
    let rx_buf: &'static mut [u8] = alloc::vec![0u8; 1024].leak();
    let tx_meta: &'static mut [PacketMetadata] = alloc::vec![PacketMetadata::EMPTY; 4].leak();
    let tx_buf: &'static mut [u8] = alloc::vec![0u8; 1024].leak();
    let mut sock = UdpSocket::new(stack, rx_meta, rx_buf, tx_meta, tx_buf);
    if sock.bind(67u16).is_err() {
        loop {
            Timer::after(Duration::from_secs(3600)).await;
        }
    }
    let req: &'static mut [u8] = alloc::vec![0u8; 600].leak();
    let reply: &'static mut [u8] = alloc::vec![0u8; 512].leak();
    loop {
        let Ok((len, _meta)) = sock.recv_from(&mut req[..]).await else {
            continue;
        };
        // BOOTREQUEST (op=1) with the DHCP magic cookie + a parseable message-type option.
        if len < 240 || req[0] != 1 || req[236..240] != [0x63, 0x82, 0x53, 0x63] {
            continue;
        }
        let reply_type = match dhcp_message_type(&req[240..len]) {
            Some(1) => 2, // DISCOVER -> OFFER
            Some(3) => 5, // REQUEST  -> ACK
            _ => continue,
        };
        let n = build_dhcp_reply(&req[..len], &mut reply[..], reply_type);
        let m = &req[28..34];
        log::info!(
            "dhcp: {} 192.168.4.2 -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            if reply_type == 2 { "OFFER" } else { "ACK" },
            m[0],
            m[1],
            m[2],
            m[3],
            m[4],
            m[5]
        );
        // The client has no IP yet, so broadcast the reply (build_dhcp_reply sets the broadcast flag).
        // 255.255.255.255 is the DHCP standard; if smoltcp refuses the limited broadcast, 192.168.4.255
        // (the directed subnet broadcast) is the fallback.
        let _ = sock
            .send_to(
                &reply[..n],
                (IpAddress::Ipv4(Ipv4Address::new(255, 255, 255, 255)), 68u16),
            )
            .await;
    }
}

/// Scan DHCP options (TLV) for option 53 (message type); returns its value (1=DISCOVER, 3=REQUEST, ...).
#[cfg(feature = "softap")]
fn dhcp_message_type(mut opts: &[u8]) -> Option<u8> {
    while let Some(&code) = opts.first() {
        if code == 255 {
            return None; // end
        }
        if code == 0 {
            opts = &opts[1..]; // pad
            continue;
        }
        let len = *opts.get(1)? as usize;
        let val = opts.get(2..2 + len)?;
        if code == 53 {
            return val.first().copied();
        }
        opts = &opts[2 + len..];
    }
    None
}

/// Build a BOOTREPLY (OFFER/ACK) leasing 192.168.4.2 with the SoftAP (192.168.4.1) as server, router,
/// and DNS; returns the reply length. `msg_type` is 2 (OFFER) or 5 (ACK).
#[cfg(feature = "softap")]
fn build_dhcp_reply(req: &[u8], out: &mut [u8], msg_type: u8) -> usize {
    out.fill(0);
    out[0] = 2; // op = BOOTREPLY
    out[1] = 1; // htype = ethernet
    out[2] = 6; // hlen
    out[4..8].copy_from_slice(&req[4..8]); // xid
    out[10] = 0x80; // flags: broadcast (client has no IP yet)
    out[16..20].copy_from_slice(&[192, 168, 4, 2]); // yiaddr (the lease)
    out[20..24].copy_from_slice(&AP_IPV4); // siaddr (server)
    out[28..44].copy_from_slice(&req[28..44]); // chaddr
    out[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]); // magic cookie
    let mut pos = 240;
    if !write_dhcp_option(out, &mut pos, 53, &[msg_type]) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 54, &AP_IPV4) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 51, &[0, 0, 0x0E, 0x10]) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 1, &[255, 255, 255, 0]) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 3, &AP_IPV4) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 6, &AP_IPV4) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 114, CAPTIVE_PORTAL_API_URL.as_bytes()) {
        return finish_dhcp_options(out, pos);
    }
    finish_dhcp_options(out, pos)
}

#[cfg(feature = "softap")]
fn write_dhcp_option(out: &mut [u8], pos: &mut usize, code: u8, value: &[u8]) -> bool {
    if *pos + 2 + value.len() + 1 > out.len() || value.len() > u8::MAX as usize {
        return false;
    }
    out[*pos] = code;
    out[*pos + 1] = value.len() as u8;
    out[*pos + 2..*pos + 2 + value.len()].copy_from_slice(value);
    *pos += 2 + value.len();
    true
}

#[cfg(feature = "softap")]
fn finish_dhcp_options(out: &mut [u8], pos: usize) -> usize {
    let pos = pos.min(out.len().saturating_sub(1));
    out[pos] = 255; // end
    pos + 1
}

/// Captive DNS for the SoftAP: every A/ANY query resolves to 192.168.4.1, which makes
/// OS connectivity checks and typed hostnames land on the Hopspot HTTP server.
#[cfg(feature = "softap")]
#[embassy_executor::task]
async fn dns_server_task(stack: Stack<'static>) -> ! {
    let rx_meta: &'static mut [PacketMetadata] = alloc::vec![PacketMetadata::EMPTY; 4].leak();
    let rx_buf: &'static mut [u8] = alloc::vec![0u8; 512].leak();
    let tx_meta: &'static mut [PacketMetadata] = alloc::vec![PacketMetadata::EMPTY; 4].leak();
    let tx_buf: &'static mut [u8] = alloc::vec![0u8; 512].leak();
    let mut sock = UdpSocket::new(stack, rx_meta, rx_buf, tx_meta, tx_buf);
    if sock.bind(53u16).is_err() {
        loop {
            Timer::after(Duration::from_secs(3600)).await;
        }
    }
    let req: &'static mut [u8] = alloc::vec![0u8; 512].leak();
    let reply: &'static mut [u8] = alloc::vec![0u8; 512].leak();
    loop {
        let Ok((len, meta)) = sock.recv_from(&mut req[..]).await else {
            continue;
        };
        let Some(reply_len) = build_dns_reply(&req[..len], &mut reply[..]) else {
            continue;
        };
        let _ = sock.send_to(&reply[..reply_len], meta.endpoint).await;
    }
}

#[cfg(feature = "softap")]
fn build_dns_reply(req: &[u8], out: &mut [u8]) -> Option<usize> {
    if req.len() < 12 || req[2] & 0x80 != 0 {
        return None;
    }
    let qdcount = u16::from_be_bytes([req[4], req[5]]);
    if qdcount == 0 {
        return None;
    }
    let (question_end, qtype) = dns_question_end(req)?;
    let answer_a = qtype == 1 || qtype == 255; // A or ANY.
    let reply_len = question_end + if answer_a { 16 } else { 0 };
    if reply_len > out.len() {
        return None;
    }

    out[..question_end].copy_from_slice(&req[..question_end]);
    out[2] = 0x81; // response + recursion desired
    out[3] = 0x80; // recursion available, no error
    out[4..6].copy_from_slice(&1u16.to_be_bytes());
    out[6..8].copy_from_slice(&(answer_a as u16).to_be_bytes());
    out[8..10].copy_from_slice(&0u16.to_be_bytes());
    out[10..12].copy_from_slice(&0u16.to_be_bytes());

    if answer_a {
        let mut pos = question_end;
        out[pos..pos + 2].copy_from_slice(&[0xC0, 0x0C]); // pointer to query name
        pos += 2;
        out[pos..pos + 2].copy_from_slice(&1u16.to_be_bytes()); // A
        pos += 2;
        out[pos..pos + 2].copy_from_slice(&1u16.to_be_bytes()); // IN
        pos += 2;
        out[pos..pos + 4].copy_from_slice(&30u32.to_be_bytes()); // short TTL
        pos += 4;
        out[pos..pos + 2].copy_from_slice(&4u16.to_be_bytes());
        pos += 2;
        out[pos..pos + 4].copy_from_slice(&AP_IPV4);
    }

    Some(reply_len)
}

#[cfg(feature = "softap")]
fn dns_question_end(req: &[u8]) -> Option<(usize, u16)> {
    let mut pos = 12;
    loop {
        let len = *req.get(pos)?;
        if len & 0xC0 != 0 {
            return None;
        }
        pos += 1;
        if len == 0 {
            break;
        }
        pos = pos.checked_add(len as usize)?;
        if pos > req.len() {
            return None;
        }
    }
    if pos + 4 > req.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([req[pos], req[pos + 1]]);
    Some((pos + 4, qtype))
}

#[cfg(feature = "softap")]
#[embassy_executor::task(pool_size = 4)]
async fn http_server_task(stack: Stack<'static>) -> ! {
    let rx_buffer: &'static mut [u8] = alloc::vec![0u8; 4096].leak();
    let tx_buffer: &'static mut [u8] = alloc::vec![0u8; 16384].leak();
    let request_buffer: &'static mut [u8] = alloc::vec![0u8; 4096].leak();
    let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
    socket.set_timeout(Some(Duration::from_secs(15)));

    loop {
        if socket.accept(80u16).await.is_err() {
            Timer::after(Duration::from_millis(250)).await;
            continue;
        }
        let peer = socket.remote_endpoint();
        let _ = serve_site_connection(&mut socket, request_buffer).await;
        socket.close();
        let _ = with_timeout(Duration::from_secs(2), socket.flush()).await;
        socket.abort();
        if let Some(peer) = peer {
            log::debug!("http: served {peer:?}");
        }
        Timer::after(Duration::from_millis(50)).await;
    }
}

#[cfg(feature = "softap")]
async fn serve_site_connection(
    socket: &mut TcpSocket<'static>,
    request_buffer: &mut [u8],
) -> Result<(), ()> {
    let len = read_http_request(socket, request_buffer).await?;
    let request = core::str::from_utf8(&request_buffer[..len]).map_err(|_| ())?;
    let Some(line) = request.lines().next() else {
        return Err(());
    };
    let mut parts = line.split_ascii_whitespace();
    let method = parts.next().unwrap_or("");
    let raw_path = parts.next().unwrap_or("/");
    let is_head = method == "HEAD";
    if method != "GET" && !is_head {
        return send_site_response(
            socket,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            b"method not allowed\n",
            is_head,
            None,
            false,
            "no-store",
        )
        .await;
    }

    let path = normalize_http_path(raw_path);
    if path == "/captive-portal/api" {
        return send_captive_portal_api(socket, is_head).await;
    }
    if is_captive_probe_path(path) {
        return send_captive_portal_redirect(socket, is_head).await;
    }
    let Some(asset) = find_site_asset(path) else {
        return send_site_response(
            socket,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found\n",
            is_head,
            None,
            false,
            "no-store",
        )
        .await;
    };
    let accepts_gzip = request_accepts_gzip(request);
    let (body, content_encoding) = match (accepts_gzip, asset.gzip_bytes) {
        (true, Some(gzip_bytes)) => (gzip_bytes, Some("gzip")),
        _ => (asset.bytes, None),
    };
    send_site_response(
        socket,
        "200 OK",
        asset.content_type,
        body,
        is_head,
        content_encoding,
        asset.gzip_bytes.is_some(),
        site_cache_control(asset.path),
    )
    .await
}

#[cfg(feature = "softap")]
async fn read_http_request(
    socket: &mut TcpSocket<'static>,
    request_buffer: &mut [u8],
) -> Result<usize, ()> {
    let mut len = 0;
    loop {
        if len == request_buffer.len() {
            return Ok(len);
        }
        let timeout = if len == 0 {
            Duration::from_secs(5)
        } else {
            Duration::from_millis(750)
        };
        match with_timeout(timeout, socket.read(&mut request_buffer[len..])).await {
            Ok(Ok(0)) if len > 0 => return Ok(len),
            Ok(Ok(0)) => return Err(()),
            Ok(Ok(read)) => {
                len += read;
                if http_headers_complete(&request_buffer[..len]) {
                    return Ok(len);
                }
            }
            _ if len > 0 => return Ok(len),
            _ => return Err(()),
        }
    }
}

#[cfg(feature = "softap")]
fn http_headers_complete(bytes: &[u8]) -> bool {
    bytes.windows(4).any(|window| window == b"\r\n\r\n")
        || bytes.windows(2).any(|window| window == b"\n\n")
}

#[cfg(feature = "softap")]
fn normalize_http_path(raw_path: &str) -> &str {
    let path = raw_path.split_once('?').map_or(raw_path, |(path, _)| path);
    let path = path.strip_prefix("/.").unwrap_or(path);
    if path.is_empty() || path == "/" {
        "/index.html"
    } else {
        path
    }
}

#[cfg(feature = "softap")]
fn is_captive_probe_path(path: &str) -> bool {
    matches!(
        path,
        "/canonical.html"
            | "/connecttest.txt"
            | "/fwlink"
            | "/generate_204"
            | "/gen_204"
            | "/hotspot-detect.html"
            | "/kindle-wifi/wifistub.html"
            | "/library/test/success.html"
            | "/ncsi.txt"
            | "/redirect"
            | "/success.txt"
    )
}

#[cfg(feature = "softap")]
fn find_site_asset(path: &str) -> Option<&'static hopspot_site::SiteAsset> {
    hopspot_site::SITE_ASSETS
        .iter()
        .find(|asset| asset.path == path)
        .or_else(|| {
            let leaf = path.rsplit('/').next().unwrap_or(path);
            if leaf.contains('.') {
                None
            } else {
                hopspot_site::SITE_ASSETS
                    .iter()
                    .find(|asset| asset.path == "/index.html")
            }
        })
}

#[cfg(feature = "softap")]
fn request_accepts_gzip(request: &str) -> bool {
    request.lines().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.trim().eq_ignore_ascii_case("accept-encoding")
            && value.split(',').any(|encoding| {
                let encoding = encoding
                    .split_once(';')
                    .map_or(encoding, |(encoding, _)| encoding)
                    .trim();
                encoding.eq_ignore_ascii_case("gzip")
            })
    })
}

#[cfg(feature = "softap")]
fn site_cache_control(path: &str) -> &'static str {
    if path == "/index.html" || path == "/source.zip" || path == "/source.zip.sha256" {
        "no-cache"
    } else if path.contains("-dxh") {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    }
}

#[cfg(feature = "softap")]
async fn send_captive_portal_api(
    socket: &mut TcpSocket<'static>,
    head_only: bool,
) -> Result<(), ()> {
    let body = b"{\"captive\":true,\"user-portal-url\":\"http://192.168.4.1/\",\"venue-info-url\":\"http://192.168.4.1/\"}\n";
    send_site_response(
        socket,
        "200 OK",
        "application/captive+json",
        body,
        head_only,
        None,
        false,
        "no-store",
    )
    .await
}

#[cfg(feature = "softap")]
async fn send_captive_portal_redirect(
    socket: &mut TcpSocket<'static>,
    head_only: bool,
) -> Result<(), ()> {
    let body = b"<!doctype html><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Hopspot</title><p><a href=\"http://192.168.4.1/\">Open Hopspot</a></p>\n";
    let header = alloc::format!(
        "HTTP/1.1 302 Found\r\nLocation: {CAPTIVE_PORTAL_URL}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tcp_write_all(socket, header.as_bytes()).await?;
    if !head_only {
        tcp_write_all(socket, body).await?;
    }
    Ok(())
}

#[cfg(feature = "softap")]
async fn send_site_response(
    socket: &mut TcpSocket<'static>,
    status: &str,
    content_type: &str,
    body: &[u8],
    head_only: bool,
    content_encoding: Option<&str>,
    vary_accept_encoding: bool,
    cache_control: &str,
) -> Result<(), ()> {
    let mut header = alloc::format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: {cache_control}\r\n",
        body.len()
    );
    if let Some(encoding) = content_encoding {
        header.push_str("Content-Encoding: ");
        header.push_str(encoding);
        header.push_str("\r\n");
    }
    if vary_accept_encoding {
        header.push_str("Vary: Accept-Encoding\r\n");
    }
    header.push_str("Connection: close\r\n\r\n");
    tcp_write_all(socket, header.as_bytes()).await?;
    if !head_only {
        tcp_write_all(socket, body).await?;
    }
    Ok(())
}

#[cfg(feature = "softap")]
async fn tcp_write_all(socket: &mut TcpSocket<'static>, mut bytes: &[u8]) -> Result<(), ()> {
    while !bytes.is_empty() {
        let written = socket.write(bytes).await.map_err(|_| ())?;
        if written == 0 {
            return Err(());
        }
        bytes = &bytes[written..];
    }
    Ok(())
}

#[cfg(feature = "radio-wifi")]
/// Bring the WiFi radio up under the AP-primary model: the SoftAP is the always-on WiFi-auto base
/// (the device is a standalone hotspot), and joining an upstream AP as a station is an *opportunistic*
/// secondary uplink — added only when an SSID is configured, never a prerequisite. With no SSID the
/// station stays idle (keepalive, no scanning) so it can't drag the shared radio off the AP's channel.
/// Returns the supervisor, the station stack (for the opportunistic TCP uplink, when present), and the
/// ESP-NOW interface.
fn build_wifi(
    spawner: &Spawner,
    wifi: esp_hal::peripherals::WIFI<'static>,
    mac: [u8; 6],
    ap_enabled: bool,
) -> (
    Option<AutoWifi<'static, MEMBERS>>,
    Option<Stack<'static>>,
    Option<EspNow<'static>>,
) {
    // Trim WiFi RX buffering from the defaults (static_rx 10, rx_ba_win 6) so the full radio stack +
    // SoftAP fits in internal DMA SRAM: each static RX buffer is ~1.6 KiB, internal and never freed,
    // and Reticulum's small frames don't need deep buffering. The captive portal's DNS socket needs
    // AP join-time margin too, so this stays one notch tighter than the earlier 4/3 floor. (The
    // 16 KiB D-cache lever is unusable here — the S3 BT controller ROM requires a 32 KiB cache,
    // ESP-IDF #10268.)
    let wifi_config = ControllerConfig::default()
        .with_static_rx_buf_num(3)
        .with_rx_ba_win(2);
    let Ok((mut controller, interfaces)) = esp_radio::wifi::new(wifi, wifi_config) else {
        return (None, None, None);
    };
    let esp_now = interfaces.esp_now;

    // APSTA brings the SoftAP up whether or not a station uplink is configured; set_config calls
    // esp_wifi_start, so the AP is live here on core 0.
    let _ = controller.set_config(&station_wifi_mode(StationConfig::default(), ap_enabled));

    // Opportunistic station uplink: only with a configured SSID do we stand a station netif up and run
    // the connect loop. With no SSID the keepalive task just owns the controller (no scanning), so the
    // radio stays parked on the AP's channel instead of hopping to hunt a network that isn't there.
    let station_segment: Option<(Stack<'static>, UdpSocket<'static>, UdpSocket<'static>)> =
        if !WIFI_SSID.is_empty() {
            let link_local = wifi_core::link_local_from_mac(MacAddress::new(mac));
            // Dual-stack: the v6 link-local carries WiFi-auto's discovery/data UDP; v4 over DHCP gives
            // the board a routable address to dial a Reticulum TCP node by ip:port.
            let mut net_config = NetConfig::dhcpv4(DhcpConfig::default());
            net_config.ipv6 = ConfigV6::Static(StaticConfigV6 {
                address: Ipv6Cidr::new(link_local, 64),
                gateway: None,
                dns_servers: Default::default(),
            });
            let resources = mk_static!(StackResources<6>, StackResources::new());
            let seed = {
                let mut bytes = [0u8; 8];
                Rng::new().read(&mut bytes);
                u64::from_le_bytes(bytes)
            };
            let (stack, runner) = embassy_net::new(interfaces.station, net_config, resources, seed);
            let discovery = {
                static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
                    ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
                static RX_BUF: ConstStaticCell<[u8; 128]> = ConstStaticCell::new([0u8; 128]);
                static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
                    ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
                static TX_BUF: ConstStaticCell<[u8; 128]> = ConstStaticCell::new([0u8; 128]);
                UdpSocket::new(
                    stack,
                    RX_META.take(),
                    RX_BUF.take(),
                    TX_META.take(),
                    TX_BUF.take(),
                )
            };
            let data = {
                static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
                    ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
                static RX_BUF: ConstStaticCell<[u8; 1280]> = ConstStaticCell::new([0u8; 1280]);
                static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
                    ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
                static TX_BUF: ConstStaticCell<[u8; 1280]> = ConstStaticCell::new([0u8; 1280]);
                UdpSocket::new(
                    stack,
                    RX_META.take(),
                    RX_BUF.take(),
                    TX_META.take(),
                    TX_BUF.take(),
                )
            };
            let wifi_status = AutoWifiStatus::new(&WIFI_SHARED);
            spawner.spawn(net_task(runner).expect("net task fits"));
            spawner.spawn(
                wifi_connect_task(controller, wifi_status, ap_enabled)
                    .expect("wifi connect task fits"),
            );
            Some((stack, discovery, data))
        } else {
            spawner.spawn(
                wifi_radio_keepalive_task(controller).expect("wifi radio keepalive task fits"),
            );
            None
        };
    let tcp_stack = station_segment.as_ref().map(|(s, _, _)| *s);

    // The SoftAP is the always-on PRIMARY WiFi-auto segment; the station (if any) is folded in as the
    // opportunistic secondary. The AP link-local is the station MAC + 1 (build_ap_netif derives it from
    // `mac`), and the supervisor hashes its peering token over that AP link-local, so it takes `ap_mac`.
    #[cfg(feature = "softap")]
    if ap_enabled {
        let mut ap_mac = mac;
        ap_mac[5] = ap_mac[5].wrapping_add(1);
        let ap_stack = build_ap_netif(spawner, interfaces.access_point, mac);
        // Hand joiners a 192.168.4.x lease with the SoftAP as their default gateway, so their WiFi-auto
        // client auto-dials the TCP rendezvous on the gateway (multicast can't cross the SoftAP).
        spawner.spawn(dhcp_server_task(ap_stack).expect("dhcp server task fits"));
        spawner.spawn(dns_server_task(ap_stack).expect("dns server task fits"));
        for _ in 0..4 {
            spawner.spawn(http_server_task(ap_stack).expect("http server task fits"));
        }
        let ap_discovery = {
            static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static RX_BUF: ConstStaticCell<[u8; 512]> = ConstStaticCell::new([0u8; 512]);
            static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static TX_BUF: ConstStaticCell<[u8; 512]> = ConstStaticCell::new([0u8; 512]);
            UdpSocket::new(
                ap_stack,
                RX_META.take(),
                RX_BUF.take(),
                TX_META.take(),
                TX_BUF.take(),
            )
        };
        let ap_data = {
            static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static RX_BUF: ConstStaticCell<[u8; 2048]> = ConstStaticCell::new([0u8; 2048]);
            static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
                ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
            static TX_BUF: ConstStaticCell<[u8; 2048]> = ConstStaticCell::new([0u8; 2048]);
            UdpSocket::new(
                ap_stack,
                RX_META.take(),
                RX_BUF.take(),
                TX_META.take(),
                TX_BUF.take(),
            )
        };
        let mut wifi = AutoWifi::new(ap_stack, ap_discovery, ap_data, ap_mac, &WIFI_SHARED);
        if let Some((s, d, dt)) = station_segment {
            // The station segment beacons over the station MAC's link-local (the address it sends from),
            // not the AP's — so the peering token validates against the source the peer actually sees.
            wifi = wifi.with_secondary_netif(s, d, dt, mac);
        }
        return (Some(wifi), tcp_stack, Some(esp_now));
    }

    match station_segment {
        Some((s, d, dt)) => {
            let wifi = AutoWifi::new(s, d, dt, mac, &WIFI_SHARED);
            (Some(wifi), tcp_stack, Some(esp_now))
        }
        None => (None, None, Some(esp_now)),
    }
}

#[cfg(feature = "radio-wifi")]
/// Hold the WiFi controller alive with no AP association — dropping it would stop the radio — so
/// ESP-NOW keeps the WiFi MAC up on a fixed channel when no SSID is configured. The radio was started
/// synchronously by [`build_wifi`] before this task takes the controller.
#[embassy_executor::task]
async fn wifi_radio_keepalive_task(_controller: WifiController<'static>) -> ! {
    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}

/// Adapts esp-radio's `EspNow` handle to the engine's [`EspNowRadio`] seam — the unsafe-free board
/// side of the boundary, the way the SX1262 driver sits behind `SpiDevice`. Broadcast-only; a
/// transient `NO_MEM` while the radio is off serving a BLE connection event is retried a few times
/// before the frame is dropped for the engine to resend.
#[cfg(feature = "radio-wifi")]
struct EspNowAdapter {
    manager: EspNowManager<'static>,
    sender: EspNowSender<'static>,
    receiver: EspNowReceiver<'static>,
    rate_applied: bool,
}

#[cfg(feature = "radio-wifi")]
const ESPNOW_SEND_RETRIES: u8 = 8;
#[cfg(feature = "radio-wifi")]
const ESPNOW_SEND_RETRY_DELAY: Duration = Duration::from_millis(5);
/// The pinned ESP-NOW PHY rate: 802.11g 12 Mbps, QPSK rate-1/2 OFDM. HT/HE *broadcast* RX is
/// hard-pinned to 1M DSSS by the closed WiFi blob (no public override) so MCS rates transmit but
/// never receive; the legacy OFDM-g family is the broadcast-compatible way to keep OFDM's good
/// multipath, and 12M is the QPSK-1/2 sweet spot (good range at ~the USB-feed budget).
///
/// Off-by-one shim: esp-radio 0.18's `set_rate` casts the sequential `WifiPhyRate` discriminant
/// straight into the C `wifi_phy_rate_t`, which reserves a gap at value 4 — so every variant past the
/// gap programs the rate one slot below its name (`Rate12m` -> C 24M). The discriminant of `Rate6m`
/// (10) equals C `WIFI_PHY_RATE_12M`, so `Rate6m` is what actually selects g-12M. This one spot
/// localizes the workaround; TODO: patch esp-radio's enum upstream and return `Rate12m`.
#[cfg(feature = "radio-wifi")]
const fn espnow_phy_rate() -> WifiPhyRate {
    WifiPhyRate::Rate6m
}

#[cfg(feature = "radio-wifi")]
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

    /// Pin the PHY rate once, lazily on first transmit — by then the radio is started (set_config runs
    /// before the interface loop in both the associated and off-grid paths), which
    /// `esp_wifi_config_espnow_rate` requires.
    fn ensure_rate(&mut self) {
        if !self.rate_applied {
            let _ = self.manager.set_rate(espnow_phy_rate());
            self.rate_applied = true;
        }
    }
}

#[cfg(feature = "radio-wifi")]
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

/// A node pinned to a WiFi access point is channel-locked to it (ESP-NOW must follow the station's
/// channel, never retune and break the association); a node with no WiFi configured is free to sit on
/// the default rendezvous channel. The locked/free seam a future scan-and-follow layer extends.
#[cfg(feature = "radio-wifi")]
fn espnow_channel_policy() -> ChannelPolicy {
    if WIFI_SSID.is_empty() {
        ChannelPolicy::Fixed(EspNowChannel::DEFAULT)
    } else {
        ChannelPolicy::FollowStation
    }
}

#[cfg(feature = "radio-wifi")]
/// Drive the embassy-net stack forever (the link/neighbor/socket machinery), on core 0.
#[embassy_executor::task(pool_size = 2)]
async fn net_task(mut runner: Runner<'static, WifiStaDevice<'static>>) -> ! {
    runner.run().await
}

/// Join the configured network in station mode and hold the association up, reconnecting on drop.
///
#[cfg(feature = "radio-wifi")]
/// A mesh (e.g. eero) hands the same SSID out on many BSSIDs across its nodes and bands and bridges
/// multicast between them unreliably, so a station left to roam can land on a node that never
/// receives the discovery group. To avoid that, this scans first and pins to the strongest BSSID
/// for the SSID — landing the Heltec V4 on one node and holding it there, where the discovery
/// multicast reaches it.
#[embassy_executor::task]
async fn wifi_connect_task(
    mut controller: WifiController<'static>,
    status: AutoWifiStatus<MEMBERS>,
    ap_enabled: bool,
) -> ! {
    let base = StationConfig::default()
        .with_ssid(WIFI_SSID)
        .with_password(WIFI_PASSWORD.into());

    let _ = controller.set_config(&station_wifi_mode(base.clone(), ap_enabled));
    loop {
        while !status.is_enabled() {
            if controller.is_connected() {
                let _ = controller.disconnect_async().await;
            }
            Timer::after(Duration::from_millis(250)).await;
        }
        if controller.is_connected() {
            Timer::after(Duration::from_secs(2)).await;
            continue;
        }
        let mut station = base.clone();
        if let Ok(networks) = controller.scan_async(&ScanConfig::default()).await {
            let mut best: Option<([u8; 6], u8, i8)> = None;
            for ap in &networks {
                if ap.ssid.as_str() == WIFI_SSID
                    && best.is_none_or(|(_, _, rssi)| ap.signal_strength > rssi)
                {
                    best = Some((ap.bssid, ap.channel, ap.signal_strength));
                }
            }
            if let Some((bssid, channel, rssi)) = best {
                log::info!(
                    "wifi: pinned to BSSID {:02x?} channel {} (rssi {})",
                    bssid,
                    channel,
                    rssi
                );
                station = base.clone().with_bssid(bssid).with_channel(channel);
            }
        }
        if !status.is_enabled() {
            continue;
        }
        if controller
            .set_config(&station_wifi_mode(station, ap_enabled))
            .is_err()
        {
            Timer::after(Duration::from_secs(2)).await;
            continue;
        }
        if !status.is_enabled() {
            continue;
        }
        if controller.connect_async().await.is_ok() {
            let _ = controller.set_power_saving(PowerSaveMode::Minimum);
        } else {
            Timer::after(Duration::from_secs(2)).await;
        }
    }
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

/// The user button worker (core 0): turn raw active-low edges on GPIO0 into the same
/// [`InputEvent`](screen::InputEvent)s the desktop face produces.
#[embassy_executor::task]
async fn button_task(mut button: Input<'static>) -> ! {
    loop {
        button.wait_for_falling_edge().await;
        match embassy_futures::select::select(
            button.wait_for_rising_edge(),
            Timer::after(BUTTON_LONG_PRESS),
        )
        .await
        {
            embassy_futures::select::Either::First(()) => {
                BUTTON_EVENTS.send(screen::InputEvent::ShortPress).await
            }
            embassy_futures::select::Either::Second(()) => {
                BUTTON_EVENTS.send(screen::InputEvent::LongPress).await;
                button.wait_for_rising_edge().await;
            }
        }
        Timer::after(BUTTON_DEBOUNCE).await;
    }
}
