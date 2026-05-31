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
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::Rng;
use esp_hal::time::{Instant, Rate};
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;

use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, Runner, Stack, StackResources};
use static_cell::StaticCell;

use core::fmt::Write as _;
use core::sync::atomic::AtomicBool;
use embassy_futures::join::join;
use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant as EmbassyInstant, Ticker, Timer};
use heapless::{String as HString, Vec as HVec};
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{self, Config as WifiConfig, Interface as WifiStaInterface, PowerSaveMode};

use personal_rns::engine::{DefaultEngineState, ReannounceSchedule, SelfAnnounceConfig};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::auto_interface::embassy::{
    run as run_auto_worker, EmbassyAutoInterface, LinkUp, OutboundChannel, OutboundReceiver,
};
use personal_rns::interfaces::{InterfaceId, InterfaceWorker};
use personal_rns::runtime::manifold::impls::embassy::{
    run as run_manifold, InboundChannel, InboundReceiver, InboundSender, RuntimeSnapshotWatch,
};
use personal_rns::runtime::{InterfaceView, Manifold, RuntimeSnapshot};

mod display;

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

/// Inbound mailbox buffer size: the worker's own `PACKET_BUFFER_SIZE`, so the
/// host sizes the shared mailbox off one well-known number — no runtime sizing,
/// the same compile-time-knob discipline as the engine preset below.
const PACKET_BUFFER_SIZE: usize = EmbassyAutoInterface::PACKET_BUFFER_SIZE;

/// Live link state, shared from the worker shell (writer) to its handle's
/// `health()` (reader) so the runtime snapshot's `online` is honest.
static LINK_UP: LinkUp = AtomicBool::new(false);

/// The runtime fires its post-cycle [`RuntimeSnapshot`] out on this; the OLED
/// render loop subscribes and wakes only when engine state changes — no poll.
static SNAPSHOT_WATCH: RuntimeSnapshotWatch = RuntimeSnapshotWatch::new();

/// Small engine-state preset for the S3: a desk node tracks a handful of
/// destinations, so the desktop default (256 dests / 4096-id history / 8 KB
/// arena) is wildly oversized and doesn't fit alongside WiFi + the worker. The
/// params are `<tracked_dests, ids_per_dest, app_data_arena, history_floor,
/// history_overflow, held_cache>`.
type S3EngineState = DefaultEngineState<24, 32, 1024, 4, 128, 4>;

/// LXMF display name this node announces as (so Sideband/Columba list it).
const DISPLAY_NAME: &str = "Personal Node (S3)";

/// Heltec V3 VBAT sense: the on-board divider is ~4.9x ((390k+100k)/100k), so
/// VBAT(mV) = pin(mV) * 49 / 10. Tune against a multimeter once on battery.
const VBAT_DIVIDER_NUM: u32 = 49;
const VBAT_DIVIDER_DEN: u32 = 10;
/// LiPo range for the %; `VBAT_EXTERNAL_MV` and up reads as on USB/charging.
const VBAT_EMPTY_MV: u32 = 3300;
const VBAT_FULL_MV: u32 = 4200;
const VBAT_EXTERNAL_MV: u32 = 4300;
/// Below this no connected LiPo is plausible (a protected cell cuts off ~3.0 V;
/// USB with no battery reads ~0), so show `Unknown` rather than a misleading 0%.
const VBAT_ABSENT_MV: u32 = 3000;

/// Blank the OLED after this long with no Reticulum activity (no change to any
/// interface's traffic / destinations / liveness); it wakes instantly when
/// traffic resumes. Saves the panel's draw on battery; on a busy fabric it
/// effectively never blanks because announces keep arriving.
const OLED_IDLE_BLANK_SECS: u64 = 30;

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
    inbound: InboundSender<PACKET_BUFFER_SIZE>,
    outbound: OutboundReceiver,
) {
    run_auto_worker(stack, INTERFACE_ID, mac, inbound, outbound, &LINK_UP).await
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
    // Portrait, title at the far end from the buttons (the non-RST button will
    // scroll the card stack once there's more than one interface).
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

    println!("HELTEC_S3 WIFI connecting (ssid len {})", WIFI_SSID.len());
    match controller.connect_async().await {
        Ok(_) => println!("HELTEC_S3 WIFI connected"),
        Err(e) => println!("HELTEC_S3 WIFI connect failed: {e:?}"),
    }
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
    static INBOUND: StaticCell<InboundChannel<PACKET_BUFFER_SIZE>> = StaticCell::new();
    static OUTBOUND: StaticCell<OutboundChannel> = StaticCell::new();
    let inbound_ch: &'static InboundChannel<PACKET_BUFFER_SIZE> =
        INBOUND.init(InboundChannel::new());
    let outbound_ch: &'static OutboundChannel = OUTBOUND.init(OutboundChannel::new());
    let inbound_tx: InboundSender<PACKET_BUFFER_SIZE> = inbound_ch.sender();
    let inbound_rx: InboundReceiver<PACKET_BUFFER_SIZE> = inbound_ch.receiver();
    let outbound_tx = outbound_ch.sender();
    let outbound_rx: OutboundReceiver = outbound_ch.receiver();

    let worker = EmbassyAutoInterface::new(INTERFACE_ID, outbound_tx, &LINK_UP);
    let manifold = Manifold::new(state, worker);

    spawner.spawn(
        auto_worker_task(stack, sta_mac, inbound_tx, outbound_rx).expect("spawn auto worker"),
    );
    println!("HELTEC_S3 worker spawned (node {dest_hex}); manifold running");

    // Keep the radio alive (dropping the controller disconnects).
    let _controller = controller;

    // Battery sense (Heltec V3): VBAT divider on GPIO1 (ADC1), gated by ADC_Ctrl
    // on GPIO37 — drive it low to connect the divider, then leave it (the ~8uA
    // draw is negligible on a mains/USB-powered hotspot).
    let mut adc_ctrl = Output::new(peripherals.GPIO37, Level::Low, OutputConfig::default());
    adc_ctrl.set_low();
    let mut adc_cfg = AdcConfig::new();
    let mut vbat_pin =
        adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<_>>(peripherals.GPIO1, Attenuation::_11dB);
    let mut vbat_adc = Adc::new(peripherals.ADC1, adc_cfg);

    // Run the manifold loop: aggregate the worker's inbound, drive the engine,
    // route egress back, and fire each cycle's snapshot out on SNAPSHOT_WATCH.
    // CSPRNG entropy from the (radio-seeded) RNG per cycle.
    let manifold_fut = run_manifold(manifold, inbound_rx, SNAPSHOT_WATCH.sender(), || {
        let mut bytes = [0u8; 8];
        Rng::new().read(&mut bytes);
        u64::from_le_bytes(bytes)
    });

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
        let mut battery_tick = Ticker::every(Duration::from_secs(2));
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
            let battery = if vbat_mv >= VBAT_EXTERNAL_MV {
                display::BatteryState::Charging
            } else if vbat_mv < VBAT_ABSENT_MV {
                display::BatteryState::Unknown
            } else {
                let span = VBAT_FULL_MV - VBAT_EMPTY_MV;
                let pct = (vbat_mv.saturating_sub(VBAT_EMPTY_MV) * 100 / span).min(100) as u8;
                display::BatteryState::Percent(pct)
            };
            log::info!("BATT pin_mv={pin_mv} vbat_mv={vbat_mv}");

            // Reticulum activity = the interfaces view changed (traffic bytes,
            // destinations, or liveness). Battery drift alone doesn't count, so
            // a quiet node still sleeps its panel.
            if let Some(snap) = &snapshot {
                if shown_interfaces.as_ref() != Some(&snap.interfaces) {
                    last_active = EmbassyInstant::now();
                    shown_interfaces = Some(snap.interfaces.clone());
                }
            }
            let idle = last_active.elapsed() >= Duration::from_secs(OLED_IDLE_BLANK_SECS);
            if idle && panel_on {
                let _ = display.set_display_on(false);
                panel_on = false;
            } else if !idle && !panel_on {
                let _ = display.set_display_on(true);
                panel_on = true;
            }

            if panel_on {
                // One card per interface in the latest snapshot. Mapping an
                // interface id to its icon/label is the host's job (one match);
                // today there's a single WiFi auto-interface.
                let mut cards: HVec<display::Card, 8> = HVec::new();
                if let Some(snap) = &snapshot {
                    for view in &snap.interfaces {
                        let _ = cards.push(display::Card {
                            kind: display::CardKind::Wifi,
                            label: "WiFi LAN",
                            online: view.online,
                            tx_bytes: view.reticulum_tx_bytes,
                            rx_bytes: view.reticulum_rx_bytes,
                            destinations: view.tracked_destinations,
                        });
                    }
                }
                display::draw(&mut display, &cards, battery);
                let _ = display.flush();
            }

            // Sleep until the engine state changes or the battery cadence
            // elapses — whichever first. A short floor coalesces an announce
            // burst into at most one render per ~100ms.
            match select(snapshot_rx.changed(), battery_tick.next()).await {
                Either::First(new_snapshot) => snapshot = Some(new_snapshot),
                Either::Second(()) => {}
            }
            Timer::after(Duration::from_millis(100)).await;
        }
    };

    join(manifold_fut, oled_fut).await;
}
