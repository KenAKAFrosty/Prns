//! Heltec WiFi LoRa 32 (ESP32-S3 + SX1262 + OLED) host.
//!
//! - **Stage A/B** (done): the Personal Reticulum engine runs on the S3.
//! - **RNSAutoInterface** (now an `InterfaceWorker`): the RNS-compatible WiFi/IP
//!   LAN interface lives in `personal-rns`
//!   (`interfaces::rns_parity::auto_interface`) as a shared brain + an embassy
//!   worker shell. This host's job shrinks to platform bring-up: WiFi
//!   association, the embassy-net IP stack (SLAAC link-local), the channels, and
//!   spawning the worker + running the [`Manifold`] loop. The worker owns all of
//!   discovery, peers, fan-out, and the data plane opaquely; the engine sees
//!   only bytes. Announces ride OTA to/from stock Reticulum, surfaced in LXMF
//!   apps as an `lxmf.delivery` destination.
//!
//! Board: Heltec WiFi LoRa 32 V3 (ESP32-S3). OLED `SDA=17 SCL=18 RST=21`,
//! `Vext=GPIO36` (active-low). WiFi creds come from build-time env
//! `WIFI_SSID` / `WIFI_PASSWORD` so they never enter source; optional
//! `WIFI_BSSID` pins the STA to one AP (mesh units don't bridge the
//! link-local multicast RNS discovery rides on).

#![no_std]
#![no_main]

extern crate alloc;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::{Rng, TrngSource};
use esp_hal::time::{Instant, Rate};
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;

use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, Runner, Stack, StackResources};
use static_cell::StaticCell;

use core::fmt::Write as _;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};
use heapless::{String as HString, Vec as HVec};
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{self, Config as WifiConfig, Interface as WifiStaInterface, PowerSaveMode};

use personal_rns::engine::{DefaultEngineState, ReannounceSchedule, SelfAnnounceConfig};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::auto_interface::embassy::{
    run as run_auto_worker, run_manifold, EmbassyAutoInterface, InboundChannel, InboundReceiver,
    InboundSender, OutboundChannel, OutboundReceiver,
};
use personal_rns::interfaces::InterfaceId;
use personal_rns::runtime::Manifold;

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

/// Engine-facing id for this host's single RNS AutoInterface. Opaque to the
/// engine; a readable label so it's obvious in `fire_on` logs.
const INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltec-s3-rnsaut");

/// Small engine-state preset for the S3: a desk node tracks a handful of
/// destinations, so the desktop default (256 dests / 4096-id history / 8 KB
/// arena) is wildly oversized and doesn't fit alongside WiFi + the worker. The
/// params are `<tracked_dests, ids_per_dest, app_data_arena, history_floor,
/// history_overflow, held_cache>`.
type S3EngineState = DefaultEngineState<24, 32, 1024, 4, 128, 4>;

/// LXMF display name this node announces as (so Sideband/Columba list it).
const DISPLAY_NAME: &str = "Personal Node (S3)";

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

/// The RNS AutoInterface worker — its own task. Owns the discovery + data
/// sockets and talks to the manifold only through the channels.
#[embassy_executor::task]
async fn auto_worker_task(
    stack: Stack<'static>,
    mac: [u8; 6],
    inbound: InboundSender,
    outbound: OutboundReceiver,
) {
    run_auto_worker(stack, INTERFACE_ID, mac, inbound, outbound).await
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

    // Hardware TRNG as the system entropy source, before the radio: esp-radio
    // draws WPA entropy from it, and the manifold draws announce-id entropy from
    // it (via the closure below). Held alive for the whole program.
    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);

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

    let state: S3EngineState = S3EngineState::announcing(
        &secret_key,
        SelfAnnounceConfig {
            app_name: "lxmf",
            aspects: &["delivery"],
            app_data: lxmf_app_data.as_slice(),
            // Fast re-announce so a listening node reliably catches us during
            // bring-up; production cadence is the 6 h `default()`.
            schedule: ReannounceSchedule::every(15_000),
        },
    )
    .expect("static self-announce config is valid");
    drop(secret_key);
    let mut dest_hex: HString<16> = HString::new();
    if let Some(dest) = state.self_announced_destination() {
        for byte in dest.as_bytes().iter().take(4) {
            let _ = write!(dest_hex, "{byte:02x}");
        }
    }

    // --- OLED (Heltec V3 pinout). ---
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
    let mut display = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate0,
    )
    .into_buffered_graphics_mode();
    let oled_ok = display.init().is_ok();
    let text = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    if oled_ok {
        display.clear_buffer();
        let _ = Text::with_baseline("Personal RNS  S3", Point::new(0, 0), text, Baseline::Top)
            .draw(&mut display);
        let mut l: HString<24> = HString::new();
        let _ = write!(l, "node {dest_hex}");
        let _ = Text::with_baseline(&l, Point::new(0, 13), text, Baseline::Top).draw(&mut display);
        let _ = Text::with_baseline("WiFi: connecting", Point::new(0, 26), text, Baseline::Top)
            .draw(&mut display);
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

    println!("HELTEC_S3 WIFI connecting (ssid len {})", WIFI_SSID.len());
    let wifi_line = match controller.connect_async().await {
        Ok(_) => {
            println!("HELTEC_S3 WIFI connected");
            "WiFi: UP"
        }
        Err(e) => {
            println!("HELTEC_S3 WIFI connect failed: {e:?}");
            "WiFi: FAIL"
        }
    };
    if let Ok(ap) = controller.ap_info() {
        let b = ap.bssid;
        println!(
            "HELTEC_S3 AP bssid {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        );
    }

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

    // --- Worker channels + manifold. ---
    static INBOUND: StaticCell<InboundChannel> = StaticCell::new();
    static OUTBOUND: StaticCell<OutboundChannel> = StaticCell::new();
    let inbound_ch: &'static InboundChannel = INBOUND.init(InboundChannel::new());
    let outbound_ch: &'static OutboundChannel = OUTBOUND.init(OutboundChannel::new());
    let inbound_tx: InboundSender = inbound_ch.sender();
    let inbound_rx: InboundReceiver = inbound_ch.receiver();
    let outbound_tx = outbound_ch.sender();
    let outbound_rx: OutboundReceiver = outbound_ch.receiver();

    let worker = EmbassyAutoInterface::new(INTERFACE_ID, outbound_tx);
    let manifold = Manifold::new(state, worker);

    spawner.spawn(auto_worker_task(stack, sta_mac, inbound_tx, outbound_rx).expect("spawn auto worker"));
    println!("HELTEC_S3 worker spawned (node {dest_hex}); manifold running");

    if oled_ok {
        display.clear_buffer();
        let _ = Text::with_baseline("Personal RNS  S3", Point::new(0, 0), text, Baseline::Top)
            .draw(&mut display);
        let mut l: HString<24> = HString::new();
        let _ = write!(l, "node {dest_hex}");
        let _ = Text::with_baseline(&l, Point::new(0, 13), text, Baseline::Top).draw(&mut display);
        let _ = Text::with_baseline(wifi_line, Point::new(0, 26), text, Baseline::Top)
            .draw(&mut display);
        let _ = Text::with_baseline("RNS auto: up", Point::new(0, 39), text, Baseline::Top)
            .draw(&mut display);
        let _ = display.flush();
    }

    // Keep the radio alive (dropping the controller disconnects).
    let _controller = controller;

    // Run the manifold loop: aggregate the worker's inbound, drive the engine,
    // route egress back. CSPRNG entropy from the hardware TRNG per cycle.
    run_manifold(manifold, inbound_rx, || {
        let mut bytes = [0u8; 8];
        Rng::new().read(&mut bytes);
        u64::from_le_bytes(bytes)
    })
    .await
}
