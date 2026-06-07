//! Heltec WiFi LoRa 32 (ESP32-S3 + SX1262 + OLED) host.
//!
//! - **Stage A/B** (done): the Personal Reticulum engine runs on the S3.
//! - **RNSAutoInterface** (now an `InterfaceWorker`): the RNS-compatible WiFi/IP
//!   LAN interface lives in `personal-rns`
//!   (`interfaces::impls::rns_parity::auto_interface`) as a shared brain + an embassy
//!   worker shell. This host's job shrinks to platform bring-up: WiFi
//!   association, the embassy-net IP stack (SLAAC link-local), the channels, and
//!   spawning the worker + running the [`Runtime`] loop. The worker owns all of
//!   discovery, peers, fan-out, and the data plane opaquely; the engine sees
//!   only bytes. Announces ride OTA to/from stock Reticulum, surfaced in LXMF
//!   apps as an `lxmf.delivery` destination.
//!
//! Board: Heltec WiFi LoRa 32 V4 (ESP32-S3). OLED `SDA=17 SCL=18 RST=21`,
//! `Vext=GPIO36` (active-low). WiFi creds come from build-time env
//! `WIFI_SSID` / `WIFI_PASSWORD` so they never enter source; optional
//! `WIFI_BSSID` pins the STA to one AP (mesh units don't bridge the
//! link-local multicast RNS discovery rides on).

#![no_std]
#![no_main]

extern crate alloc;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Flex, Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::Rng;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::{Instant, Rate};
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use esp_println::println;

use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, Runner, Stack, StackResources};
use static_cell::StaticCell;

use core::convert::Infallible;
use core::fmt::Write as _;
use embassy_futures::join::join4;
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Delay, Duration, Instant as EmbassyInstant, Ticker, Timer};
use heapless::{String as HString, Vec as HVec};
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use embedded_hal_bus::spi::ExclusiveDevice;
use lora_phy::iv::GenericSx126xInterfaceVariant;
use lora_phy::sx126x::{Config as Sx126xConfig, Sx1262, Sx126x, TcxoCtrlVoltage};
use lora_phy::LoRa;

use esp_radio::esp_now::{EspNowError, EspNowReceiver, EspNowSender, BROADCAST_ADDRESS};
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{self, Config as WifiConfig, Interface as WifiStaInterface, PowerSaveMode};

use personal_rns::engine::self_announce::AnnounceConfig;
use personal_rns::engine::{EngineState, ReannounceSchedule};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::impls::esp_now::core::descriptor as espnow_descriptor;
use personal_rns::interfaces::impls::esp_now::embassy::{serve as serve_esp_now, EspNowLink};
use personal_rns::interfaces::impls::rns_parity::auto_interface::core::HARDWARE_MTU;
use personal_rns::interfaces::impls::rns_parity::auto_interface::embassy::{
    descriptor as auto_descriptor, serve as serve_auto,
};
use personal_rns::interfaces::impls::rns_parity::rnode_lora::core::{
    descriptor as lora_descriptor, DEFAULT_915_LORA_PROFILE,
};
use personal_rns::interfaces::impls::rns_parity::rnode_lora::embassy::serve as serve_lora;
use personal_rns::interfaces::impls::rns_parity::serial::serve as serve_serial;
use personal_rns::interfaces::impls::rns_parity::serial::{
    descriptor as serial_descriptor, SERIAL_MTU,
};
use personal_rns::interfaces::storage::{FixedInterfaceSet, InterfaceSet};
use personal_rns::interfaces::substrate::{
    new_wake_signal, EmbassyHostSubstrate, EmbassyInterfaceChannels, EmbassyInterfaceHandle,
    EmbassyInterfaceSeam, WakeSignal,
};
use personal_rns::interfaces::MacAddress;
use personal_rns::interfaces::{
    ControlReport, DriverMode, InboundPacket, InterfaceHandle, InterfaceId, InterfaceWorkerContext,
    SendError, StartedInterface,
};
use personal_rns::routing::storage::FixedInline;
use personal_rns::engine::RatchetPolicy;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::channels::embassy::RuntimeSnapshotWatch;
use personal_rns::runtime::host::impls::EmbassyContractHost;
use personal_rns::runtime::{InterfaceView, PrnsEvent, Runtime, RuntimeSnapshot};
use personal_rns::wire::MTU;

use personal_hopspot_ui as display;

esp_app_desc!();

/// WiFi credentials, baked in at build time (never committed to source):
/// `WIFI_SSID="…" WIFI_PASSWORD="…" cargo build --release`.
const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");
/// Optional BSSID to pin the STA to (e.g. `WIFI_BSSID=24:2d:6c:11:aa:48`). On a
/// multi-unit mesh, link-local multicast doesn't bridge between units, so RNS
/// AutoInterface discovery only works when this node shares a physical AP with
/// its peers. Unset = associate to the strongest BSSID (may roam between units).
const WIFI_BSSID: Option<&str> = option_env!("WIFI_BSSID");

/// Engine-facing id for this host's RNS AutoInterface (WiFi LAN). Opaque to the
/// engine; a readable label so it's obvious in `fire_on` logs.
const INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltec-s3-rnsaut");

/// Engine-facing id for this host's USB serial interface (the cable to a laptop
/// or another board). Opaque to the engine; readable in `fire_on` logs.
const SERIAL_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltec-s3-usbser");

/// Engine-facing id for this host's LoRa interface (the SX1262 radio). Opaque to
/// the engine; readable in `fire_on` logs.
const LORA_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltec-s3-lora62");

/// Engine-facing id for this host's ESP-NOW interface (broadcast over the 2.4 GHz
/// radio to other Hopspots). Opaque to the engine; readable in `fire_on` logs.
const ESPNOW_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltec-s3-espnow");

/// Max buffered packets per interface seam (inbound + outbound). Modest — a desk
/// node's bursts are small; ESP-NOW gets more to give its coalescing room.
const AUTO_MAX_BUFFERED_PACKETS: usize = 4;
const SERIAL_MAX_BUFFERED_PACKETS: usize = 8;
const LORA_MAX_BUFFERED_PACKETS: usize = 4;
const ESPNOW_MAX_BUFFERED_PACKETS: usize = 8;

/// Each interface's four channels live in one board `static` (the embassy idiom);
/// every seam shares the one [`WakeSignal`] the contract host sleeps on. Sized to
/// each interface's own MTU — the AutoInterface's 1196 B, the engine MTU elsewhere.
static AUTO_CH: EmbassyInterfaceChannels<HARDWARE_MTU, AUTO_MAX_BUFFERED_PACKETS> =
    EmbassyInterfaceChannels::new();
static SERIAL_CH: EmbassyInterfaceChannels<SERIAL_MTU, SERIAL_MAX_BUFFERED_PACKETS> =
    EmbassyInterfaceChannels::new();
static LORA_CH: EmbassyInterfaceChannels<MTU, LORA_MAX_BUFFERED_PACKETS> =
    EmbassyInterfaceChannels::new();
static ESPNOW_CH: EmbassyInterfaceChannels<MTU, ESPNOW_MAX_BUFFERED_PACKETS> =
    EmbassyInterfaceChannels::new();
static WAKE: WakeSignal = new_wake_signal();

/// The worker-side seam each *spawned* interface task runs against (auto + serial;
/// lora + espnow are joined in `main`, their context types inferred from the split).
type AutoContext =
    InterfaceWorkerContext<EmbassyHostSubstrate<HARDWARE_MTU, AUTO_MAX_BUFFERED_PACKETS>>;
type SerialContext =
    InterfaceWorkerContext<EmbassyHostSubstrate<SERIAL_MTU, SERIAL_MAX_BUFFERED_PACKETS>>;

/// The runtime holds one handle type, but the S3's interfaces have
/// differently-sized seams (the AutoInterface's 1196 B vs the engine MTU) — so
/// their `EmbassyInterfaceHandle`s are distinct types. This enum unifies them
/// into the one `InterfaceHandle` the [`Runtime`] pools, dispatching each call
/// to the held variant (the contract analog of the old `HostWorker` enum;
/// explicit per the no-wildcard rule).
enum HeltecHandle {
    Auto(EmbassyInterfaceHandle<HARDWARE_MTU>),
    Serial(EmbassyInterfaceHandle<SERIAL_MTU>),
    Lora(EmbassyInterfaceHandle<MTU>),
    EspNow(EmbassyInterfaceHandle<MTU>),
}

impl InterfaceHandle for HeltecHandle {
    fn next_inbound<R>(&mut self, f: impl FnOnce(InboundPacket<'_>) -> R) -> Option<R> {
        match self {
            HeltecHandle::Auto(h) => h.next_inbound(f),
            HeltecHandle::Serial(h) => h.next_inbound(f),
            HeltecHandle::Lora(h) => h.next_inbound(f),
            HeltecHandle::EspNow(h) => h.next_inbound(f),
        }
    }

    fn acquire_send_grant(
        &mut self,
        fill: impl FnOnce(&mut [u8]) -> usize,
    ) -> Result<usize, SendError> {
        match self {
            HeltecHandle::Auto(h) => h.acquire_send_grant(fill),
            HeltecHandle::Serial(h) => h.acquire_send_grant(fill),
            HeltecHandle::Lora(h) => h.acquire_send_grant(fill),
            HeltecHandle::EspNow(h) => h.acquire_send_grant(fill),
        }
    }

    fn request_stop(&mut self) {
        match self {
            HeltecHandle::Auto(h) => h.request_stop(),
            HeltecHandle::Serial(h) => h.request_stop(),
            HeltecHandle::Lora(h) => h.request_stop(),
            HeltecHandle::EspNow(h) => h.request_stop(),
        }
    }

    fn next_report(&mut self) -> Option<ControlReport> {
        match self {
            HeltecHandle::Auto(h) => h.next_report(),
            HeltecHandle::Serial(h) => h.next_report(),
            HeltecHandle::Lora(h) => h.next_report(),
            HeltecHandle::EspNow(h) => h.next_report(),
        }
    }
}

/// The host's ESP-NOW radio adapter: implements `personal-rns`'s [`EspNowLink`]
/// over esp-radio's split sender/receiver, so the worker shell names no chip HAL.
/// Broadcast goes to the ESP-NOW broadcast address; receive copies one frame in.
struct S3EspNowLink<'d> {
    sender: EspNowSender<'d>,
    receiver: EspNowReceiver<'d>,
}

impl EspNowLink for S3EspNowLink<'_> {
    type Error = EspNowError;

    async fn broadcast(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        self.sender.send_async(&BROADCAST_ADDRESS, frame).await
    }

    async fn receive_into(&mut self, buf: &mut [u8]) -> usize {
        let data = self.receiver.receive_async().await;
        let src = data.data();
        let n = src.len().min(buf.len());
        buf[..n].copy_from_slice(&src[..n]);
        n
    }
}

/// The runtime fires its post-cycle [`RuntimeSnapshot`] out on this; the OLED
/// render loop subscribes and wakes only when engine state changes — no poll.
static SNAPSHOT_WATCH: RuntimeSnapshotWatch = RuntimeSnapshotWatch::new();

/// The user button's task posts short/long-press events here; the OLED loop is
/// the single consumer, turning them into [`UiState`](display::UiState)
/// transitions. A small queue so a quick double-tap mid-render isn't dropped.
static BUTTON_EVENTS: Channel<CriticalSectionRawMutex, display::InputEvent, 4> = Channel::new();

/// Small engine-state preset for the S3: a desk node tracks a handful of
/// destinations, so the default `FixedInline` recipe (64 dests /
/// 64 ids-per-dest / 4 KB app-data arena, ~65 KB total) is oversized and
/// doesn't fit comfortably alongside WiFi + the worker — this preset is ~12 KB.
/// The params are `<tracked_dests, ids_per_dest, app_data_arena, history_floor,
/// history_overflow, held_cache>`.
type S3EngineState = EngineState<FixedInline<24, 32, 1024, 4, 128, 4, 4, 4, 4, 32, 8, 8, 8>>;

/// LXMF display name this node announces as (so Sideband/Columba list it).
const DISPLAY_NAME: &str = "Personal Hopspot (Heltec V4)";

/// Heltec V4 VBAT sense: the on-board divider is ~4.9x ((390k+100k)/100k), so
/// VBAT(mV) = pin(mV) * 49 / 10. Tune against a multimeter once on battery.
const VBAT_DIVIDER_NUM: u32 = 49;
const VBAT_DIVIDER_DEN: u32 = 10;
/// LiPo range for the bar fill (datasheet: 3.3 V empty … 4.2 V full).
const VBAT_EMPTY_MV: u32 = 3300;
const VBAT_FULL_MV: u32 = 4200;
/// Below this no connected LiPo is plausible (a protected cell cuts off ~3.0 V;
/// USB with no battery reads ~0), so show `Unknown` rather than misleading bars.
const VBAT_ABSENT_MV: u32 = 3000;

// No charging/bolt indicator: the V4 exposes no charge-status pin, and this
// board's charger floats the cell at only ~4.10 V — inside a full pack's normal
// loaded range — so voltage alone can't tell charging from a full battery
// draining. We just show the level bars; a real charge signal could add a bolt
// later.

/// Blank the OLED after this long with no Reticulum activity (no change to any
/// interface's traffic / destinations / liveness); it wakes instantly when
/// traffic resumes. Saves the panel's draw on battery; on a busy fabric it
/// effectively never blanks because announces keep arriving.
const OLED_IDLE_BLANK_SECS: u64 = 30;

/// Hold the user button at least this long for a long press (open/close a menu);
/// anything shorter is a tap that advances focus. Matches the desktop face's
/// threshold so both faces feel the same.
const BUTTON_LONG_PRESS: Duration = Duration::from_millis(650);
/// Settle time after each press resolves, so the mechanical contact's bounce on
/// release isn't read as a fresh press.
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);

/// Busy-wait a few ms during setup (before the async loop runs).
fn block_ms(ms: u64) {
    let target = Instant::now().duration_since_epoch().as_millis() + ms;
    while Instant::now().duration_since_epoch().as_millis() < target {}
}

/// Parse a colon-separated MAC like "24:2d:6c:11:aa:48" into 6 bytes.
fn parse_bssid(s: &str) -> Option<[u8; 6]> {
    let mut out = [0u8; 6];
    let mut n = 0;
    for part in s.split(':') {
        if n >= 6 {
            return None;
        }
        out[n] = u8::from_str_radix(part, 16).ok()?;
        n += 1;
    }
    (n == 6).then_some(out)
}

/// The embassy-net background task: polls the WiFi device and runs the IP stack.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiStaInterface<'static>>) -> ! {
    runner.run().await
}

/// The RNS AutoInterface worker — its own task. Owns the discovery + data sockets
/// and meets the runtime only through its seam.
#[embassy_executor::task]
async fn auto_worker_task(stack: Stack<'static>, mac: [u8; 6], context: AutoContext) {
    serve_auto(stack, MacAddress::new(mac), context).await
}

/// The USB serial worker — its own task. Owns the usb-serial-jtag halves and meets
/// the runtime only through its seam.
#[embassy_executor::task]
async fn serial_worker_task(
    rx: UsbSerialJtagRx<'static, Async>,
    tx: UsbSerialJtagTx<'static, Async>,
    context: SerialContext,
) {
    serve_serial(rx, tx, context).await
}

/// The user button (PRG/BOOT, GPIO0 — the non-RST button) worker, its own task.
/// Turns the raw active-low edges into the same [`InputEvent`](display::InputEvent)s
/// the desktop face produces and posts them to the OLED loop: a tap (release
/// before [`BUTTON_LONG_PRESS`]) is a `ShortPress`; crossing the hold threshold
/// fires a `LongPress` the instant it's reached — so the menu opens without
/// waiting for release — and the eventual release is then swallowed.
#[embassy_executor::task]
async fn button_task(mut button: Input<'static>) {
    loop {
        // Active-low: a press pulls GPIO0 to ground (falling), the pull-up holds
        // it high on release (rising).
        button.wait_for_falling_edge().await;
        match select(
            button.wait_for_rising_edge(),
            Timer::after(BUTTON_LONG_PRESS),
        )
        .await
        {
            Either::First(()) => BUTTON_EVENTS.send(display::InputEvent::ShortPress).await,
            Either::Second(()) => {
                BUTTON_EVENTS.send(display::InputEvent::LongPress).await;
                button.wait_for_rising_edge().await;
            }
        }
        Timer::after(BUTTON_DEBOUNCE).await;
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // esp-radio needs a heap and a preemptive scheduler, started before the radio.
    esp_alloc::heap_allocator!(size: 60 * 1024);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // No TrngSource: the hardware RNG is already true-random whenever the RF
    // subsystem is enabled (our WiFi stays on), which frees ADC1 for the battery
    // sense (VBAT is on ADC1/GPIO1). All crypto/WPA/announce-jitter randomness
    // happens after WiFi is up, so it draws from the true RNG.

    // Route the worker's `log` output (in personal-rns) to the JTAG serial.
    esp_println::logger::init_logger(log::LevelFilter::Info);

    println!("HELTEC_S3: boot — Personal Reticulum on ESP32-S3 (RNSAutoInterface worker)");

    // --- Engine: announcing node, pinned fixture identity, lxmf.delivery dest. ---
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);

    // LXMF delivery announce app_data: `msgpack([display_name_bytes, stamp_cost])`
    // — name bin8-encoded, nil stamp cost — the shape LXMF 0.9.9 emits, so an
    // LXMF app surfaces us as a messageable peer.
    let mut lxmf_app_data: HVec<u8, 64> = HVec::new();
    let _ = lxmf_app_data.push(0x92); // msgpack: 2-element array
    let _ = lxmf_app_data.push(0xc4); // msgpack: bin8
    let _ = lxmf_app_data.push(DISPLAY_NAME.len() as u8);
    let _ = lxmf_app_data.extend_from_slice(DISPLAY_NAME.as_bytes());
    let _ = lxmf_app_data.push(0xc0); // msgpack: nil (no stamp cost)

    let mut state: S3EngineState = S3EngineState::new(secret_key);
    let node = state
        .transport_identity()
        .expect("new() holds the node identity");
    let lxmf_delivery = state
        .register_single_destination(
            &node,
            "lxmf",
            &["delivery"],
            ProofStrategy::ProveAll,
            RatchetPolicy::Ratcheted,
        )
        .expect("static destination config is valid");
    state
        .schedule_announce(
            &lxmf_delivery,
            AnnounceConfig {
                app_data: lxmf_app_data.as_slice(),
                // Fast re-announce so a listening node reliably catches us during
                // bring-up; production cadence is the 6 h `default()`.
                schedule: ReannounceSchedule::every(15_000),
            },
        )
        .expect("static announce config is valid");
    let state = state;
    let mut dest_hex: HString<16> = HString::new();
    if let Some(dest) = state.self_announced_destinations().first() {
        for byte in dest.as_bytes().iter().take(4) {
            let _ = write!(dest_hex, "{byte:02x}");
        }
    }

    // --- OLED (Heltec V4 pinout). ---
    let mut vext = Output::new(peripherals.GPIO36, Level::Low, OutputConfig::default());
    vext.set_low();
    let mut oled_rst = Output::new(peripherals.GPIO21, Level::High, OutputConfig::default());
    oled_rst.set_low();
    block_ms(20);
    oled_rst.set_high();
    block_ms(20);
    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("i2c0")
    .with_sda(peripherals.GPIO17)
    .with_scl(peripherals.GPIO18);
    // Portrait, title at the far end from the buttons (the non-RST button scrolls
    // the card stack and opens menus — see `button_task`).
    let mut display = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate90,
    )
    .into_buffered_graphics_mode();
    let oled_ok = display.init().is_ok();
    if oled_ok {
        display::splash(&mut display, "connecting");
        let _ = display.flush();
    }

    // --- WiFi association (esp-radio). ---
    let (mut controller, interfaces) =
        wifi::new(peripherals.WIFI, Default::default()).expect("esp-radio wifi::new");
    let mut sta = StationConfig::default()
        .with_ssid(WIFI_SSID)
        .with_password(WIFI_PASSWORD.into());
    if let Some(bssid_str) = WIFI_BSSID {
        match parse_bssid(bssid_str) {
            Some(bssid) => {
                sta = sta.with_bssid(bssid);
                println!("HELTEC_S3 WIFI pinning to bssid {bssid_str}");
            }
            None => println!("HELTEC_S3 WIFI ignoring malformed WIFI_BSSID '{bssid_str}'"),
        }
    }
    controller
        .set_config(&WifiConfig::Station(sta))
        .expect("set STA config");
    // A sleeping STA misses AP-buffered multicast — exactly how discovery
    // beacons arrive — so keep the receiver always on.
    controller
        .set_power_saving(PowerSaveMode::None)
        .expect("disable wifi power save");

    // Associate, retrying until the AP accepts us. ESP32 cold-boot association is
    // racy; a single failed `connect_async` used to fall straight through to the
    // unconditional `wait_link_up` below and hang forever on the "connecting"
    // splash. Loop instead — the splash clears the moment we're actually on.
    println!("HELTEC_S3 WIFI connecting (ssid len {})", WIFI_SSID.len());
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match controller.connect_async().await {
            Ok(_) => {
                println!("HELTEC_S3 WIFI connected (attempt {attempt})");
                break;
            }
            Err(e) => {
                println!("HELTEC_S3 WIFI connect attempt {attempt} failed: {e:?}; retrying");
                Timer::after(Duration::from_millis(1000)).await;
            }
        }
    }
    if let Ok(ap) = controller.ap_info() {
        let b = ap.bssid;
        println!(
            "HELTEC_S3 AP bssid {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        );
    }

    // --- ESP-NOW (Personal-native broadcast). Rides the same STA radio (esp-now
    // feature on esp-radio): once associated, ESP-NOW's channel is locked to the
    // AP's, so two Hopspots on the same AP are co-channel and hear each other's
    // broadcasts with no extra config. The broadcast peer is auto-added on split.
    let (esp_now_manager, esp_now_sender, esp_now_receiver) = interfaces.esp_now.split();
    match esp_now_manager.version() {
        Ok(v) => {
            println!("HELTEC_S3 ESPNOW up — version {v} (v2 => 1470 B frames, no fragmentation)")
        }
        Err(e) => println!("HELTEC_S3 ESPNOW version query failed: {e:?}"),
    }
    // Keep the manager alive for the program's life — it holds the endpoint up
    // alongside the sender/receiver the worker owns.
    let _esp_now_manager = esp_now_manager;

    // --- IP stack (embassy-net) with SLAAC → IPv6 link-local. ---
    let sta_mac = interfaces.station.mac_address();
    let net_config = NetConfig::slaac();
    static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
    let resources = RESOURCES.init(StackResources::new());
    let (stack, runner) = embassy_net::new(
        interfaces.station,
        net_config,
        resources,
        0x5eed_1234_c0ff_ee01,
    );
    spawner.spawn(net_task(runner).expect("spawn net_task"));
    stack.wait_link_up().await;
    println!("HELTEC_S3 NET link up");

    // --- Worker seams + runtime. ---
    // The embassy contract host owns the one shared wake every seam pokes, and draws
    // each cycle's CSPRNG from the radio-seeded RNG (true-random now WiFi is up). Glue
    // each interface's seam from it (split from a board `static`); the runtime keeps
    // the four handles (unified by `HeltecHandle`), each worker its context.
    let host = EmbassyContractHost::new(&WAKE, |bytes: &mut [u8]| {
        Rng::new().read(bytes);
    });
    let EmbassyInterfaceSeam {
        worker_context: auto_context,
        runtime_handle: auto_handle,
    } = host.glue_seam(INTERFACE_ID, &AUTO_CH);
    let EmbassyInterfaceSeam {
        worker_context: serial_context,
        runtime_handle: serial_handle,
    } = host.glue_seam(SERIAL_INTERFACE_ID, &SERIAL_CH);
    let EmbassyInterfaceSeam {
        worker_context: lora_context,
        runtime_handle: lora_handle,
    } = host.glue_seam(LORA_INTERFACE_ID, &LORA_CH);
    let EmbassyInterfaceSeam {
        worker_context: espnow_context,
        runtime_handle: espnow_handle,
    } = host.glue_seam(ESPNOW_INTERFACE_ID, &ESPNOW_CH);

    // The S3's USB-C is the native usb-serial-jtag; share it for RNS frames (the
    // serial worker) and esp-println logs (register pokes) — the C6 precedent.
    let (usb_rx, usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE)
        .into_async()
        .split();

    // Four self-driven interfaces — WiFi LAN + USB serial + LoRa + ESP-NOW — each a
    // descriptor + its runtime handle. auto + serial run as spawned tasks (below);
    // lora + espnow are joined in `main` (they borrow non-'static radio resources), so
    // their serve futures are built further down with the contexts captured here.
    let started: [StartedInterface<HeltecHandle, Infallible>; 4] = [
        StartedInterface {
            descriptor: auto_descriptor(INTERFACE_ID),
            handle: HeltecHandle::Auto(auto_handle),
            drive: DriverMode::SelfDriven,
        },
        StartedInterface {
            descriptor: serial_descriptor(SERIAL_INTERFACE_ID),
            handle: HeltecHandle::Serial(serial_handle),
            drive: DriverMode::SelfDriven,
        },
        StartedInterface {
            descriptor: lora_descriptor(LORA_INTERFACE_ID),
            handle: HeltecHandle::Lora(lora_handle),
            drive: DriverMode::SelfDriven,
        },
        StartedInterface {
            descriptor: espnow_descriptor(ESPNOW_INTERFACE_ID),
            handle: HeltecHandle::EspNow(espnow_handle),
            drive: DriverMode::SelfDriven,
        },
    ];

    let mut interfaces = FixedInterfaceSet::<_, 4>::new();
    for interface in started {
        let _ = interfaces.push(interface);
    }
    let runtime = Runtime::new(state, interfaces, host);

    spawner.spawn(auto_worker_task(stack, sta_mac, auto_context).expect("spawn auto worker"));
    spawner.spawn(serial_worker_task(usb_rx, usb_tx, serial_context).expect("spawn serial worker"));
    // The user button drives the OLED's short/long-press interaction. GPIO0 is the
    // non-RST (PRG/BOOT) button; the internal pull-up holds it high when released.
    let user_button = Input::new(
        peripherals.GPIO0,
        InputConfig::default().with_pull(Pull::Up),
    );
    spawner.spawn(button_task(user_button).expect("spawn button worker"));
    println!("HELTEC_S3 workers spawned (node {dest_hex}); runtime running");

    // Keep the radio alive (dropping the controller disconnects).
    let _controller = controller;

    // Battery sense (Heltec V4): VBAT divider on GPIO1 (ADC1_CH0), gated by
    // ADC_Ctrl on GPIO37. NOTE: the V4 flips the V3 convention — GPIO37 must be
    // driven HIGH to connect the divider (V3 used LOW), per the V4 datasheet.
    // Held high and left; the ~8uA through the 490k divider is negligible.
    let mut adc_ctrl = Output::new(peripherals.GPIO37, Level::High, OutputConfig::default());
    adc_ctrl.set_high();
    let mut adc_cfg = AdcConfig::new();
    let mut vbat_pin =
        adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<_>>(peripherals.GPIO1, Attenuation::_11dB);
    let mut vbat_adc = Adc::new(peripherals.ADC1, adc_cfg);

    // --- LoRa radio (SX1262) [slice 1c]. Build the radio + V4 front-end here in
    // main scope (the FEM GPIOs must stay driven for the program's life), then
    // hand the radio to the LoRa worker, joined below. SPI2 on SCK9/MOSI10/MISO11
    // + CS8; RESET12/BUSY13/DIO1-14; DIO2-as-RF-switch (so the variant takes no
    // GPIO switch pins). TCXO is 1.8 V on the V4 (3.3 V on the V3).
    let lora_spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default().with_frequency(Rate::from_mhz(8)),
    )
    .expect("lora spi2")
    .with_sck(peripherals.GPIO9)
    .with_mosi(peripherals.GPIO10)
    .with_miso(peripherals.GPIO11)
    .into_async();
    let lora_cs = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());
    let lora_spi_device = ExclusiveDevice::new(lora_spi, lora_cs, Delay).expect("lora spi device");
    let lora_reset = Output::new(peripherals.GPIO12, Level::High, OutputConfig::default());
    let lora_busy = Input::new(peripherals.GPIO13, InputConfig::default());
    let lora_dio1 = Input::new(peripherals.GPIO14, InputConfig::default());
    let lora_iv = GenericSx126xInterfaceVariant::new(lora_reset, lora_dio1, lora_busy, None, None)
        .expect("lora interface variant");
    let lora_radio = Sx126x::new(
        lora_spi_device,
        lora_iv,
        Sx126xConfig {
            chip: Sx1262,
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V8),
            use_dcdc: true,
            rx_boost: true,
        },
    );

    // V4 front-end (FEM) enable. PWR_EN(7)+CSD(2) power it; detect the IC the
    // RNode way (PWR_EN high, read CSD). Our board is the GC1109: DIO2 drives
    // CTX(5), we hold CPS(46) high (the non-DIO2 switch pin); never drive the
    // DIO2-owned pin (that'd fight DIO2). These GPIOs live in main scope so the
    // FEM stays powered for the program's life — the worker, joined below, never
    // returns.
    let _lora_pa_pwr = Output::new(peripherals.GPIO7, Level::High, OutputConfig::default());
    let mut lora_csd = Flex::new(peripherals.GPIO2);
    lora_csd.apply_input_config(&InputConfig::default());
    lora_csd.set_input_enable(true);
    Timer::after(Duration::from_millis(5)).await;
    let lora_is_kct8103l = lora_csd.is_high();
    lora_csd.set_output_enable(true);
    lora_csd.set_high(); // CSD high = FEM enabled (LNA/PA powered)
    let _lora_fem_switch = if lora_is_kct8103l {
        println!("HELTEC_S3 LORA FEM=KCT8103L (DIO2 drives CPS46; hold CTX5 high)");
        Output::new(peripherals.GPIO5, Level::High, OutputConfig::default())
    } else {
        println!("HELTEC_S3 LORA FEM=GC1109 (DIO2 drives CTX5; hold CPS46 high)");
        Output::new(peripherals.GPIO46, Level::High, OutputConfig::default())
    };

    // Init the radio (`false` = private network → RNode's 0x1424 sync) and run
    // the LoRa worker. On init failure the LoRa card stays offline but the rest
    // of the node runs.
    let lora_fut = async move {
        match LoRa::new(lora_radio, false, Delay).await {
            Ok(lora) => {
                println!("HELTEC_S3 LORA init ok — SX1262 up (TCXO 1.8V, private sync 0x1424)");
                serve_lora(lora, DEFAULT_915_LORA_PROFILE, lora_context).await;
            }
            Err(e) => println!("HELTEC_S3 LORA init FAILED: {e:?}"),
        }
    };

    // The ESP-NOW worker: hand it the host's radio adapter (esp-radio sender +
    // receiver) and run. No init handshake — ESP-NOW is connectionless — so the
    // shell is live as soon as it starts.
    let espnow_fut = serve_esp_now(
        S3EspNowLink {
            sender: esp_now_sender,
            receiver: esp_now_receiver,
        },
        espnow_context,
    );

    // Run the runtime loop: aggregate the workers' inbound, drive the engine,
    // route egress back, and fire each cycle's snapshot out on SNAPSHOT_WATCH.
    // (The host built above draws CSPRNG entropy from the radio-seeded RNG per
    // cycle and owns the embassy-time clock + sleep.)
    let snapshot_tx = SNAPSHOT_WATCH.sender();
    let runtime_fut = runtime.run(
        move |event: PrnsEvent<'_>| match event {
            PrnsEvent::SnapshotUpdated(snapshot) => snapshot_tx.send(snapshot.clone()),
            PrnsEvent::Delivered(_) => {}
            PrnsEvent::AnnounceHeard { .. } => {}
            PrnsEvent::CommandSettled { .. } => {}
        },
        || None,
    );

    // Render the Hopspot screen alongside it. Event-driven: subscribe to the
    // runtime snapshot and wake only when engine state changes (no polling),
    // plus a slow ticker so the (non-engine) battery readout still refreshes.
    // After OLED_IDLE_BLANK_SECS with no Reticulum activity the panel powers off
    // to save battery, waking the instant traffic resumes. Joined, not spawned,
    // so it can borrow `display`.
    let oled_fut = async {
        if !oled_ok {
            core::future::pending::<()>().await;
        }
        let mut snapshot_rx = SNAPSHOT_WATCH.receiver().expect("one snapshot receiver");
        let mut snapshot: Option<RuntimeSnapshot> = None;
        // The interfaces view as of the last activity, to detect change.
        let mut shown_interfaces: Option<HVec<InterfaceView, 8>> = None;
        let mut last_active = EmbassyInstant::now();
        let mut panel_on = true;
        // Smoothed VBAT (0 mV = uninitialized) so the bar level doesn't jitter
        // on ADC noise.
        let mut vbat_ema_mv: u32 = 0;
        let mut battery_tick = Ticker::every(Duration::from_secs(2));
        // Single-button focus/menu state, driven by `button_task`'s events.
        let mut ui_state = display::UiState::new();
        loop {
            // VBAT (mV at the pin, calibrated) scaled by the ~4.9x divider. An
            // implausibly low reading means no LiPo (USB-only) → Unknown.
            let mut pin_mv = 0u16;
            for _ in 0..1000 {
                if let Ok(v) = vbat_adc.read_oneshot(&mut vbat_pin) {
                    pin_mv = v;
                    break;
                }
            }
            let vbat_mv = pin_mv as u32 * VBAT_DIVIDER_NUM / VBAT_DIVIDER_DEN;

            // Battery level bars from the smoothed voltage; an implausibly low
            // reading means no LiPo (USB-only) → Unknown.
            let battery = if vbat_mv < VBAT_ABSENT_MV {
                display::BatteryState::Unknown
            } else {
                vbat_ema_mv = if vbat_ema_mv == 0 {
                    vbat_mv
                } else {
                    (vbat_ema_mv * 7 + vbat_mv) / 8
                };
                let span = VBAT_FULL_MV - VBAT_EMPTY_MV;
                let pct = (vbat_ema_mv.saturating_sub(VBAT_EMPTY_MV) * 100 / span).min(100) as u8;
                display::BatteryState::Level(pct)
            };
            log::info!("BATT pin_mv={pin_mv} vbat_mv={vbat_mv} ema={vbat_ema_mv}");

            // Reticulum activity = the interfaces view changed (traffic bytes,
            // destinations, or liveness). Battery drift alone doesn't count, so
            // a quiet node still sleeps its panel.
            if let Some(snap) = &snapshot {
                if shown_interfaces.as_ref() != Some(&snap.interfaces) {
                    last_active = EmbassyInstant::now();
                    shown_interfaces = Some(snap.interfaces.clone());
                    // Mirror what the OLED now shows, so per-interface counts are
                    // observable headlessly on the muxed CDC: destinations are
                    // attributed to the interface each route was learned on, not
                    // a shared global total.
                    for view in &snap.interfaces {
                        let label = if view.id == SERIAL_INTERFACE_ID {
                            "USB"
                        } else if view.id == LORA_INTERFACE_ID {
                            "LoRa"
                        } else if view.id == ESPNOW_INTERFACE_ID {
                            "ESP-NOW"
                        } else {
                            "WiFi"
                        };
                        log::info!(
                            "HELTEC_S3 IFACE {label} state={:?} dest={} rx={} tx={}",
                            view.connection_state,
                            view.tracked_destinations,
                            view.reticulum_rx_byte_count,
                            view.reticulum_tx_byte_count,
                        );
                    }
                }
            }
            // One card per interface in the latest snapshot. Mapping an interface
            // id to its icon/label is the host's job; returning `None` hides that
            // interface from the panel. Built every iteration (cheap) so the button
            // handler below always has the live card count for its focus/menu math.
            let cards: HVec<display::Card, 8> = match &snapshot {
                Some(snap) => display::snapshot_to_cards(snap, |id| {
                    if id == SERIAL_INTERFACE_ID {
                        Some((display::CardKind::Usb, "USB"))
                    } else if id == LORA_INTERFACE_ID {
                        Some((display::CardKind::LoRa, "LoRa"))
                    } else if id == ESPNOW_INTERFACE_ID {
                        Some((display::CardKind::EspNow, "ESP-NOW"))
                    } else {
                        Some((display::CardKind::Wifi, "WiFi LAN"))
                    }
                }),
                None => HVec::new(),
            };
            // Reconcile selection/window with the current interface count. All four
            // cards are reachable: a short press pages focus through them, so the
            // one that doesn't fit the panel scrolls into view.
            let card_count = cards.len();
            ui_state.sync_card_count(card_count);

            let idle = last_active.elapsed() >= Duration::from_secs(OLED_IDLE_BLANK_SECS);
            if idle && panel_on {
                let _ = display.set_display_on(false);
                panel_on = false;
            } else if !idle && !panel_on {
                let _ = display.set_display_on(true);
                panel_on = true;
            }

            if panel_on {
                display::draw_with_state(&mut display, &cards, battery, &ui_state);
                let _ = display.flush();
            }

            // Sleep until the engine state changes, the battery cadence elapses,
            // or the user presses the button — whichever first. A button press is
            // user activity, so it also un-blanks an idle panel.
            match select3(
                snapshot_rx.changed(),
                battery_tick.next(),
                BUTTON_EVENTS.receive(),
            )
            .await
            {
                Either3::First(new_snapshot) => {
                    snapshot = Some(new_snapshot);
                    // A short floor coalesces an announce burst into at most one
                    // render per ~100ms.
                    Timer::after(Duration::from_millis(100)).await;
                }
                Either3::Second(()) => {}
                Either3::Third(event) => {
                    ui_state.handle_input(event, card_count);
                    last_active = EmbassyInstant::now();
                }
            }
        }
    };

    join4(runtime_fut, oled_fut, lora_fut, espnow_fut).await;
}
