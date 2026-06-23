use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::{Flex, Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::Rng;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::system::Stack as CpuStack;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;

use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{select3, Either3};
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{
    Config as NetConfig, ConfigV6, DhcpConfig, IpEndpoint, Ipv6Cidr, Runner, Stack, StackResources,
    StaticConfigV6,
};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel;
use embassy_time::{Delay, Duration, Ticker, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use heapless::Vec as HVec;
use portable_atomic::{AtomicU64, Ordering};
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};
use static_cell::{ConstStaticCell, StaticCell};

#[cfg(not(feature = "ble-bringup"))]
use esp_radio::wifi::scan::ScanConfig;
#[cfg(not(feature = "ble-bringup"))]
use esp_radio::wifi::sta::StationConfig;
#[cfg(not(feature = "ble-bringup"))]
use esp_radio::wifi::{
    Config as WifiConfig, ControllerConfig, Interface as WifiStaDevice, WifiController,
};

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, InstantMillis, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
#[cfg(feature = "ble-bringup")]
use personal_rns::interfaces::bluetooth_auto::{BluetoothAutoShared, BluetoothAutoStatus};
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
use personal_rns::subghz_rf::{BoardConfig, Sx126x, TcxoVoltage};
use personal_rns::wire::TransportId;

use crate::engine_storage::EngineStorageType;

use personal_hopspot_ui as screen;

esp_app_desc!();

/// This board's USB-auto interface id (the always-present top-level wire on pool slot 0).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltecv4");

/// This node's `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])`
/// = `fixarray(2)` ‖ `bin8("Personal Hopspot HeltecV4")` ‖ `nil`, the shape LXMF apps parse.
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x19Personal Hopspot HeltecV4\xc0";

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
/// they share slot 2 — so the expensive MTU buffers number four, not four-plus-every-peer.
const IFACES: usize = 4;
/// The WiFi fleet's member budget: how many peers the supervisor carries at once. Each costs only a
/// descriptor + a status slot, never a lane buffer, so it is sized generously.
const MEMBERS: usize = 24;
/// The engine-interface (descriptor + pacer) pool: the three fixed interfaces (USB, TCP, LoRa) plus
/// the WiFi members. Distinct from the lane count `IFACES` — decoupling them is the whole point of
/// the shared lane, so a generous member budget costs descriptors, not buffers.
const MAX_IFACES: usize = 3 + MEMBERS;
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
pub const NOTIFY_CAP: usize = 16;
const COMMANDS_CAP: usize = 8;
pub const LIFECYCLE_CAP: usize = 8;
const COMPLETIONS_CAP: usize = 4;

/// Core 1's stack carries *both* the one-time engine *construction* (the big, dalek-heavy
/// transient) and the per-poll ingest crypto the reactor runs afterward — never at once, since
/// construction returns before the reactor loop, so it is sized for the construction peak and the
/// reactor reuses that space. Core 0's main-task stack only drives its I/O + screen loop, so it
/// stays far shallower.
const CORE1_STACK_BYTES: usize = 96 * 1024;

const VBAT_DIVIDER_NUM: u32 = 49;
const VBAT_DIVIDER_DEN: u32 = 10;
const VBAT_EMPTY_MV: u32 = 3300;
const VBAT_FULL_MV: u32 = 4200;
const VBAT_ABSENT_MV: u32 = 3000;

const RENDER_INTERVAL: Duration = Duration::from_millis(500);
const RENDER_TICKS_PER_BATTERY: u8 = 4;

const BUTTON_LONG_PRESS: Duration = Duration::from_millis(650);
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

/// The USB interface's live state, written by the device task (core 0) and read by the render loop
/// (core 0) — the engine on core 1 reaches it through the lanes, this `static` is a face-side view.
static USB_STATUS: EmbassyInterfaceStatus =
    EmbassyInterfaceStatus::new(USB_INTERFACE_ID, ConnectionState::Initializing);

/// The WiFi supervisor's shared aggregate + per-peer status (written + read on core 0).
static WIFI_SHARED: AutoWifiShared<MEMBERS> = AutoWifiShared::new(WIFI_FLEET_ID);

/// Under `ble-bringup` the BLE supervisor reuses the (WiFi-free) fleet slot 2, keyed by its own kind
/// so `BluetoothPeer` members route to it. The radio carries one connection, so one member slot.
#[cfg(feature = "ble-bringup")]
pub const BLE_MEMBERS: usize = 1;
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

/// Platform bring-up on core 0: the OLED, the self-identity crypto, the radios + WiFi/TCP, and the
/// I/O run-loops + screen. The engine is built *and* owned by core 1 — it constructs the node on
/// its own stack (the dalek-heavy transient) then runs the reactor there, so core 0 never touches
/// the node. True parallelism (engine ⊥ I/O) over the cross-core lane channels. Never returns:
/// this frame is core 0's I/O + screen drive.
#[allow(clippy::too_many_lines)]
pub async fn run(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 70 * 1024);
    esp_alloc::psram_allocator!(p.PSRAM, esp_hal::psram);
    let timg0 = TimerGroup::new(p.TIMG0);
    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let mut rtc = Rtc::new(p.LPWR);
    // The engine construction allocates + zeroes PSRAM-backed columns synchronously; PSRAM is slow,
    // so it can overrun the RTC watchdog's ~2s timeout. Disable RWDT/SWD over the boot build.
    rtc.rwdt.disable();
    rtc.swd.disable();
    let timebase = EmbassyTimebase::start_at(InstantMillis(rtc.current_time_us() / 1000));

    println!("HOPSPOT_HELTECV4 boot — recipe runtime, engine core 1 + I/O core 0");

    // OLED (Heltec V4: Vext active-low gates panel power; pulse RST; I2C0 on 17/18).
    let mut _vext = Output::new(p.GPIO36, Level::Low, OutputConfig::default());
    let mut rst = Output::new(p.GPIO21, Level::High, OutputConfig::default());
    rst.set_low();
    Timer::after(Duration::from_millis(20)).await;
    rst.set_high();
    Timer::after(Duration::from_millis(20)).await;
    let i2c = I2c::new(
        p.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("i2c0")
    .with_sda(p.GPIO17)
    .with_scl(p.GPIO18);
    let mut display = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate90,
    )
    .into_buffered_graphics_mode();
    let oled_ok = display.init().is_ok();
    if oled_ok {
        screen::splash(&mut display, "Personal Hopspot");
        let _ = display.flush();
    }

    USB_STATUS.set_enabled(false);

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
        let _ = inbound.push((FREE_SLOT, in_consumer));
        let _ = egress_lanes.push((FREE_SLOT, out_producer));
        iface_halves[slot] = Some((in_producer, out_consumer));
    }

    // LoRa is held fully offline under `ble-bringup` (radio uninitialized, interface unbuilt) so the
    // BLE data-plane bring-up has no confounding second radio/interface. Only the cheap id + status
    // are kept so the card list renders a (perpetually-initializing) LoRa tile.
    #[cfg(not(feature = "ble-bringup"))]
    let lora_radio = {
        let lora_spi = Spi::new(
            p.SPI2,
            SpiConfig::default().with_frequency(Rate::from_mhz(8)),
        )
        .expect("lora spi2")
        .with_sck(p.GPIO9)
        .with_mosi(p.GPIO10)
        .with_miso(p.GPIO11)
        .into_async();
        let lora_cs = Output::new(p.GPIO8, Level::High, OutputConfig::default());
        let lora_spi_device =
            ExclusiveDevice::new(lora_spi, lora_cs, Delay).expect("lora spi device");
        let lora_reset = Output::new(p.GPIO12, Level::High, OutputConfig::default());
        let lora_busy = Input::new(p.GPIO13, InputConfig::default());
        let lora_dio1 = Input::new(p.GPIO14, InputConfig::default());
        let _lora_pa_pwr = Output::new(p.GPIO7, Level::High, OutputConfig::default());
        let mut lora_csd = Flex::new(p.GPIO2);
        lora_csd.apply_input_config(&InputConfig::default());
        lora_csd.set_input_enable(true);
        let lora_is_kct8103l = lora_csd.is_high();
        lora_csd.set_output_enable(true);
        lora_csd.set_high();
        let _lora_fem_switch = if lora_is_kct8103l {
            Output::new(p.GPIO5, Level::High, OutputConfig::default())
        } else {
            Output::new(p.GPIO46, Level::High, OutputConfig::default())
        };
        Sx126x::new(
            lora_spi_device,
            lora_busy,
            lora_dio1,
            lora_reset,
            Delay,
            BoardConfig {
                tcxo_voltage: Some(TcxoVoltage::V1_8),
                use_dcdc: true,
                rx_boost: true,
                dio2_as_rf_switch: true,
            },
        )
    };
    let lora_profile = DEFAULT_915_PROFILE;
    let lora_id = InterfaceId::from_channel_tag(InterfaceKind::LoRa, &channel_tag(&lora_profile));
    let lora_status: &'static EmbassyInterfaceStatus = mk_static!(
        EmbassyInterfaceStatus,
        EmbassyInterfaceStatus::new(lora_id, ConnectionState::Initializing)
    );
    #[cfg(not(feature = "ble-bringup"))]
    let lora = LoRaInterface::new(
        lora_radio,
        lora_profile,
        &LORA_CONTROL,
        lora_status,
        LIFECYCLE.dyn_sender(),
    );

    // The WiFi stack carries both the WiFi-auto UDP and the TCP client, so it stands up before the
    // node moves to core 1 — activating the TCP slot is a core-0-only act.
    #[cfg(not(feature = "ble-bringup"))]
    let wifi_built = build_wifi(&spawner, p.WIFI, mac_octets);
    #[cfg(feature = "ble-bringup")]
    let wifi_built: Option<(AutoWifi<'static, MEMBERS>, Stack<'static>)> = None;
    let stack = wifi_built.as_ref().map(|(_, stack)| *stack);
    let wifi = wifi_built.map(|(wifi, _)| wifi);
    let tcp_built = stack.and_then(build_tcp);
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
    let host = EmbassyHost::new_with_timebase(timebase, seeded_entropy as fn(&mut [u8]));

    let recipe = PrnsRecipe {
        transport: Some(transport_id),
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "lxmf",
            aspects: &["delivery"],
            identity: secret_key,
            announce_app_data: ANNOUNCE_APP_DATA,
            proof: personal_rns::routing::ProofStrategy::ProveAll,
            ratchet: RatchetPolicy::Ratcheted,
        }],
        app_state: (),
        storage: EngineStorageType::default(),
        routes: personal_rns::routes![],
        interfaces: personal_rns::interfaces![],
        on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
    };

    #[cfg(not(feature = "ble-bringup"))]
    let lora_cfg = lora.descriptor();
    let tcp_cfg = tcp_built.as_ref().map(|(t, _, _)| t.descriptor());
    let has_wifi = wifi.is_some();

    // The engine is built and run on core 1: its stack carries the dalek-heavy construction transient
    // and then the reactor reuses that space (see `CORE1_STACK_BYTES`). Core 0 keeps only its I/O +
    // screen loop.
    let core1_stack = mk_static!(CpuStack<CORE1_STACK_BYTES>, CpuStack::new());
    esp_rtos::start_second_core(
        p.CPU_CTRL,
        sw_int.software_interrupt1,
        core1_stack,
        move || {
            static NODE: StaticCell<S3Node> = StaticCell::new();
            let node: &'static mut S3Node =
                NODE.init_with(|| Prns::new(recipe, plumbing, host, HVec::new()));
            if let Some(cfg) = tcp_cfg {
                node.activate(TCP_SLOT, cfg);
            }
            #[cfg(not(feature = "ble-bringup"))]
            node.activate(LORA_SLOT, lora_cfg);
            #[cfg(feature = "ble-bringup")]
            node.activate_fleet(WIFI_FLEET_SLOT, BLE_FLEET_ID);
            #[cfg(not(feature = "ble-bringup"))]
            if has_wifi {
                node.activate_fleet(WIFI_FLEET_SLOT, WIFI_FLEET_ID);
            }
            node.set_interface_store(&INTERFACE_COUNTS);
            log_heap_footprint("post-construction (engine columns boxed into PSRAM)");

            static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
            EXECUTOR
                .init(esp_rtos::embassy::Executor::new())
                .run(|spawner| {
                    spawner.spawn(reactor_core(node).expect("reactor task fits"));
                })
        },
    );

    #[cfg(not(feature = "ble-bringup"))]
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

    let tcp = tcp_built.map(|(tcp, _, _)| {
        let (in_producer, out_consumer) = iface_halves[TCP_SLOT].take().expect("tcp slot half");
        let seam = EmbassyInterfaceSeam::new(tcp.id(), in_producer, NOTIFY.sender(), out_consumer);
        (tcp, seam)
    });

    // The whole WiFi fleet shares slot 2's one lane: the supervisor funnels every peer's frames
    // through it, tagged by the peer's id, and the reactor demuxes by kind. Members are descriptors,
    // not lanes — so no per-peer wire is taken here.
    let (wifi_in_producer, wifi_out_consumer) = iface_halves[WIFI_FLEET_SLOT]
        .take()
        .expect("wifi fleet half");
    let fleet: Fleet<Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN, NOTIFY_CAP, LIFECYCLE_CAP> = Fleet::new(
        MemberWire {
            inbound: wifi_in_producer,
            outbound: wifi_out_consumer,
            notify: NOTIFY.sender(),
            outbound_wake: &OUTBOUND_WAKE,
        },
        LIFECYCLE.sender(),
    );

    let button = Input::new(p.GPIO0, InputConfig::default().with_pull(Pull::Up));
    spawner.spawn(button_task(button).expect("button task fits"));

    // Battery sense (Heltec V4): VBAT divider on GPIO1 (ADC1_CH0), gated by ADC_Ctrl on GPIO37.
    let mut adc_ctrl = Output::new(p.GPIO37, Level::High, OutputConfig::default());
    adc_ctrl.set_high();
    let mut adc_cfg = AdcConfig::new();
    let mut vbat_pin =
        adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<_>>(p.GPIO1, Attenuation::_11dB);
    let mut vbat_adc = Adc::new(p.ADC1, adc_cfg);

    let wifi_status = wifi.as_ref().map(AutoWifi::status);
    let wifi_id = wifi_status.as_ref().map(|status| {
        use personal_rns::interfaces::InterfaceStatus;
        status.id()
    });

    let render = async move {
        let mut ui_state = screen::UiState::new();
        let mut working_lora_profile = DEFAULT_915_PROFILE;
        let mut vbat_ema_mv: u32 = 0;
        let mut battery_state = screen::BatteryState::Unknown;
        let mut ticks_to_battery: u8 = 0;
        #[cfg(feature = "ble-bringup")]
        let mut ble_announce_ticks: u8 = 0;
        let mut render_tick = Ticker::every(RENDER_INTERVAL);
        let mut settle_after_draw = false;
        loop {
            if ticks_to_battery == 0 {
                let mut pin_mv = 0u16;
                for _ in 0..1000 {
                    if let Ok(value) = vbat_adc.read_oneshot(&mut vbat_pin) {
                        pin_mv = value;
                        break;
                    }
                }
                let vbat_mv = pin_mv as u32 * VBAT_DIVIDER_NUM / VBAT_DIVIDER_DEN;
                battery_state = if vbat_mv < VBAT_ABSENT_MV {
                    screen::BatteryState::Unknown
                } else {
                    vbat_ema_mv = if vbat_ema_mv == 0 {
                        vbat_mv
                    } else {
                        (vbat_ema_mv * 7 + vbat_mv) / 8
                    };
                    let span = VBAT_FULL_MV - VBAT_EMPTY_MV;
                    let pct =
                        (vbat_ema_mv.saturating_sub(VBAT_EMPTY_MV) * 100 / span).min(100) as u8;
                    screen::BatteryState::Level(pct)
                };
                ticks_to_battery = RENDER_TICKS_PER_BATTERY;
            }

            let cards = build_cards(
                &USB_STATUS,
                wifi_status.as_ref(),
                wifi_id,
                tcp_status,
                tcp_id,
                lora_status,
                lora_status.id(),
            );
            let card_count = cards.len();
            ui_state.sync_card_count(card_count);
            if oled_ok {
                screen::draw_with_state(&mut display, &cards, battery_state, &ui_state);
                let _ = display.flush();
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
                    let selected_kind = ui_state
                        .selected_card(card_count)
                        .and_then(|index| cards.get(index))
                        .map(|card| card.kind);
                    match ui_state.handle_input(event, card_count, selected_kind) {
                        screen::UiAction::Announce => {
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
                                if card.id == USB_INTERFACE_ID {
                                    USB_STATUS.set_enabled(!USB_STATUS.is_enabled());
                                } else if card.id == lora_status.id() {
                                    lora_status.set_enabled(!lora_status.is_enabled());
                                } else if let (Some(tcp), Some(tcp_id)) = (tcp_status, tcp_id) {
                                    if card.id == tcp_id {
                                        tcp.set_enabled(!tcp.is_enabled());
                                    }
                                }
                            }
                        }
                        screen::UiAction::OpenLoRaEditor => {
                            ui_state.open_lora_editor(working_lora_profile);
                        }
                        screen::UiAction::SetLoRaProfile(profile) => {
                            working_lora_profile = profile;
                            LORA_CONTROL.signal(profile);
                        }
                        screen::UiAction::None => {}
                    }
                }
            }
        }
    };

    #[cfg(feature = "ble-bringup")]
    let ble_connector = esp_radio::ble::controller::BleConnector::new(p.BT, Default::default())
        .expect("ble connector");

    #[cfg(feature = "ble-bringup")]
    {
        let _ = (wifi, tcp, has_wifi);
        join(
            crate::ble::run(ble_connector, mac_octets, node_identity, fleet, &BLE_SHARED),
            render,
        )
        .await;
    }
    #[cfg(not(feature = "ble-bringup"))]
    {
        let lora_run = lora.run(lora_seam);
        match (wifi, tcp) {
            (Some(wifi), Some((tcp, tcp_seam))) => {
                join(
                    join(lora_run, join(wifi.run(fleet), tcp.run(tcp_seam))),
                    render,
                )
                .await;
            }
            (Some(wifi), None) => {
                join(join(lora_run, wifi.run(fleet)), render).await;
            }
            (None, _) => {
                join(lora_run, render).await;
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
) -> HVec<screen::Card, 8> {
    use personal_rns::interfaces::InterfaceStatus;
    #[cfg(feature = "ble-bringup")]
    let ble = BluetoothAutoStatus::new(&BLE_SHARED);
    let classify = |id: InterfaceId| -> Option<(screen::CardKind, screen::CardLabel)> {
        if id == USB_INTERFACE_ID {
            Some((screen::CardKind::Usb, screen::card_label("USB")))
        } else if id == lora_id {
            Some((screen::CardKind::LoRa, screen::card_label("LoRa")))
        } else if Some(id) == wifi_id {
            Some((screen::CardKind::Wifi, screen::card_label("WiFi")))
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

#[cfg(not(feature = "ble-bringup"))]
/// Bring the WiFi stack up in station mode and hand back the supervisor. `None` with no SSID (the
/// board then runs USB-only). Spawns the net runner + the connect/reconnect loop on core 0.
fn build_wifi(
    spawner: &Spawner,
    wifi: esp_hal::peripherals::WIFI<'static>,
    mac: [u8; 6],
) -> Option<(AutoWifi<'static, MEMBERS>, Stack<'static>)> {
    if WIFI_SSID.is_empty() {
        return None;
    }
    let (controller, interfaces) = esp_radio::wifi::new(wifi, ControllerConfig::default()).ok()?;

    let link_local = wifi_core::link_local_from_mac(MacAddress::new(mac));
    // Dual-stack: the v6 link-local carries WiFi-auto's discovery/data UDP (peer-to-peer on the
    // segment); v4 over DHCP gives the board a routable address to dial a Reticulum TCP node by
    // ip:port.
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
        static RX_BUF: ConstStaticCell<[u8; 512]> = ConstStaticCell::new([0u8; 512]);
        static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
            ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
        static TX_BUF: ConstStaticCell<[u8; 512]> = ConstStaticCell::new([0u8; 512]);
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
        static RX_BUF: ConstStaticCell<[u8; 2048]> = ConstStaticCell::new([0u8; 2048]);
        static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
            ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
        static TX_BUF: ConstStaticCell<[u8; 2048]> = ConstStaticCell::new([0u8; 2048]);
        UdpSocket::new(
            stack,
            RX_META.take(),
            RX_BUF.take(),
            TX_META.take(),
            TX_BUF.take(),
        )
    };

    spawner.spawn(net_task(runner).expect("net task fits"));
    spawner.spawn(wifi_connect_task(controller).expect("wifi connect task fits"));
    Some((
        AutoWifi::new(stack, discovery, data, mac, &WIFI_SHARED),
        stack,
    ))
}

#[cfg(not(feature = "ble-bringup"))]
/// Drive the embassy-net stack forever (the link/neighbor/socket machinery), on core 0.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiStaDevice<'static>>) -> ! {
    runner.run().await
}

/// Join the configured network in station mode and hold the association up, reconnecting on drop.
///
#[cfg(not(feature = "ble-bringup"))]
/// A mesh (e.g. eero) hands the same SSID out on many BSSIDs across its nodes and bands and bridges
/// multicast between them unreliably, so a station left to roam can land on a node that never
/// receives the discovery group. To avoid that, this scans first and pins to the strongest BSSID
/// for the SSID — landing the Heltec V4 on one node and holding it there, where the discovery
/// multicast reaches it.
#[embassy_executor::task]
async fn wifi_connect_task(mut controller: WifiController<'static>) -> ! {
    let base = StationConfig::default()
        .with_ssid(WIFI_SSID)
        .with_password(WIFI_PASSWORD.into());

    let _ = controller.set_config(&WifiConfig::Station(base.clone()));
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
    let config = WifiConfig::Station(station);
    loop {
        if controller.is_connected() {
            Timer::after(Duration::from_secs(2)).await;
            continue;
        }
        if controller.set_config(&config).is_err() {
            Timer::after(Duration::from_secs(2)).await;
            continue;
        }
        if controller.connect_async().await.is_err() {
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
