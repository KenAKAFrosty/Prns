//! Heltec WiFi LoRa 32 (ESP32-S3 + SX1262 + OLED) host.
//!
//! - **Stage A/B** (done): the Personal Reticulum engine runs on the S3.
//! - **RNSAutoInterface** (now an `InterfaceWorker`): the RNS-compatible WiFi/IP
//!   LAN interface lives in `personal-rns`
//!   (`interfaces::impls::rns_parity::auto_interface`) as a shared brain + an embassy
//!   worker shell. This host's job shrinks to platform bring-up: WiFi
//!   association, the embassy-net IP stack (SLAAC link-local), the channels, and
//!   spawning the worker + running the [`Manifold`] loop. The worker owns all of
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
use esp_hal::gpio::{Flex, Input, InputConfig, Level, Output, OutputConfig};
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

use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU16};
use embassy_futures::join::join4;
use embassy_futures::select::{select, Either};
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

use personal_rns::engine::{
    EngineCycleEntropySeed, FixedCapacityEngineState, OutboundPacket, ReannounceSchedule,
    SelfAnnounceConfig, ENGINE_CYCLE_ENTROPY_LEN,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::impls::esp_now::embassy::{
    run as run_esp_now_worker, EmbassyEspNowInterface, EspNowLink,
    OutboundChannel as EspNowOutboundChannel, OutboundReceiver as EspNowOutboundReceiver,
};
use personal_rns::interfaces::impls::rns_parity::auto_interface::embassy::{
    run as run_auto_worker, EmbassyAutoInterface, LinkUp, OutboundChannel, OutboundReceiver,
};
use personal_rns::interfaces::impls::rns_parity::rnode_lora::core::DEFAULT_915_LORA_PROFILE;
use personal_rns::interfaces::impls::rns_parity::rnode_lora::embassy::{
    run as run_lora_worker, EmbassyRnodeLoraInterface, OutboundChannel as LoraOutboundChannel,
    OutboundReceiver as LoraOutboundReceiver,
};
use personal_rns::interfaces::impls::rns_parity::serial::embassy::{
    run as run_serial_worker, EmbassySerialInterface, OutboundChannel as SerialOutboundChannel,
    OutboundReceiver as SerialOutboundReceiver,
};
use personal_rns::interfaces::MacAddress;
use personal_rns::interfaces::{
    InterfaceDescriptor, InterfaceId, InterfaceStats, InterfaceWorker, QueueFull,
};
use personal_rns::runtime::host::impls::EmbassyHost;
use personal_rns::runtime::manifold::impls::embassy::{
    InboundChannel, InboundReceiver, InboundSender, RuntimeSnapshotWatch,
};
use personal_rns::runtime::{run, InterfaceView, Manifold, RuntimeSnapshot};

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

/// The S3 runs four interface workers, so the multi-worker manifold holds them as
/// one concrete type — this enum. Dispatch is explicit per the no-wildcard rule.
enum HostWorker {
    Wifi(EmbassyAutoInterface),
    Serial(EmbassySerialInterface),
    LoRa(EmbassyRnodeLoraInterface),
    EspNow(EmbassyEspNowInterface),
}

impl InterfaceWorker for HostWorker {
    // The shared inbound mailbox sizes to this, so it must fit any worker's
    // frames — the max across all three.
    const PACKET_BUFFER_SIZE: usize = {
        let wifi = EmbassyAutoInterface::PACKET_BUFFER_SIZE;
        let serial = EmbassySerialInterface::PACKET_BUFFER_SIZE;
        let lora = EmbassyRnodeLoraInterface::PACKET_BUFFER_SIZE;
        let espnow = EmbassyEspNowInterface::PACKET_BUFFER_SIZE;
        let m = if wifi > serial { wifi } else { serial };
        let m = if m > lora { m } else { lora };
        if m > espnow {
            m
        } else {
            espnow
        }
    };

    fn descriptor(&self) -> InterfaceDescriptor {
        match self {
            HostWorker::Wifi(w) => w.descriptor(),
            HostWorker::Serial(s) => s.descriptor(),
            HostWorker::LoRa(l) => l.descriptor(),
            HostWorker::EspNow(e) => e.descriptor(),
        }
    }

    fn health(&self) -> InterfaceStats {
        match self {
            HostWorker::Wifi(w) => w.health(),
            HostWorker::Serial(s) => s.health(),
            HostWorker::LoRa(l) => l.health(),
            HostWorker::EspNow(e) => e.health(),
        }
    }

    fn submit(&mut self, packet: OutboundPacket) -> Result<(), QueueFull> {
        match self {
            HostWorker::Wifi(w) => w.submit(packet),
            HostWorker::Serial(s) => s.submit(packet),
            HostWorker::LoRa(l) => l.submit(packet),
            HostWorker::EspNow(e) => e.submit(packet),
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

/// Inbound mailbox buffer size: the largest worker's `PACKET_BUFFER_SIZE`, so
/// the host sizes the shared mailbox off one well-known number — no runtime
/// sizing, the same compile-time-knob discipline as the engine preset below.
const PACKET_BUFFER_SIZE: usize = HostWorker::PACKET_BUFFER_SIZE;

/// Live link state, shared from each worker shell (writer) to its handle's
/// `health()` (reader) so the runtime snapshot's `online` is honest.
static LINK_UP: LinkUp = AtomicBool::new(false);
static WIFI_PEERS: AtomicU16 = AtomicU16::new(0);
static SERIAL_LINK_UP: AtomicBool = AtomicBool::new(false);
static LORA_LINK_UP: AtomicBool = AtomicBool::new(false);
static ESPNOW_LINK_UP: AtomicBool = AtomicBool::new(false);

/// The runtime fires its post-cycle [`RuntimeSnapshot`] out on this; the OLED
/// render loop subscribes and wakes only when engine state changes — no poll.
static SNAPSHOT_WATCH: RuntimeSnapshotWatch = RuntimeSnapshotWatch::new();

/// Small engine-state preset for the S3: a desk node tracks a handful of
/// destinations, so the `FixedCapacityEngineState` default (64 dests /
/// 64 ids-per-dest / 4 KB app-data arena, ~65 KB total) is oversized and
/// doesn't fit comfortably alongside WiFi + the worker — this preset is ~12 KB.
/// The params are `<tracked_dests, ids_per_dest, app_data_arena, history_floor,
/// history_overflow, held_cache>`.
type S3EngineState = FixedCapacityEngineState<24, 32, 1024, 4, 128, 4>;

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
    run_auto_worker(
        stack,
        INTERFACE_ID,
        MacAddress::new(mac),
        inbound,
        outbound,
        &LINK_UP,
        &WIFI_PEERS,
    )
    .await
}

/// The USB serial worker — its own task. Owns the usb-serial-jtag halves and
/// talks to the manifold only through the channels (shared inbound mailbox +
/// its own outbound queue).
#[embassy_executor::task]
async fn serial_worker_task(
    rx: UsbSerialJtagRx<'static, Async>,
    tx: UsbSerialJtagTx<'static, Async>,
    inbound: InboundSender<PACKET_BUFFER_SIZE>,
    outbound: SerialOutboundReceiver,
) {
    run_serial_worker(
        rx,
        tx,
        SERIAL_INTERFACE_ID,
        inbound,
        outbound,
        &SERIAL_LINK_UP,
    )
    .await
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

    // --- Worker channels + manifold. ---
    // One shared inbound mailbox (both workers stamp into it; the manifold
    // drains it), and a per-worker outbound queue (the manifold fills, the
    // worker shell drains).
    static INBOUND: StaticCell<InboundChannel<PACKET_BUFFER_SIZE>> = StaticCell::new();
    static OUTBOUND: StaticCell<OutboundChannel> = StaticCell::new();
    static SERIAL_OUTBOUND: StaticCell<SerialOutboundChannel> = StaticCell::new();
    static LORA_OUTBOUND: StaticCell<LoraOutboundChannel> = StaticCell::new();
    static ESPNOW_OUTBOUND: StaticCell<EspNowOutboundChannel> = StaticCell::new();
    let inbound_ch: &'static InboundChannel<PACKET_BUFFER_SIZE> =
        INBOUND.init(InboundChannel::new());
    let outbound_ch: &'static OutboundChannel = OUTBOUND.init(OutboundChannel::new());
    let serial_outbound_ch: &'static SerialOutboundChannel =
        SERIAL_OUTBOUND.init(SerialOutboundChannel::new());
    let lora_outbound_ch: &'static LoraOutboundChannel =
        LORA_OUTBOUND.init(LoraOutboundChannel::new());
    let espnow_outbound_ch: &'static EspNowOutboundChannel =
        ESPNOW_OUTBOUND.init(EspNowOutboundChannel::new());
    let inbound_rx: InboundReceiver<PACKET_BUFFER_SIZE> = inbound_ch.receiver();
    let outbound_rx: OutboundReceiver = outbound_ch.receiver();
    let serial_outbound_rx: SerialOutboundReceiver = serial_outbound_ch.receiver();
    let lora_outbound_rx: LoraOutboundReceiver = lora_outbound_ch.receiver();
    let espnow_outbound_rx: EspNowOutboundReceiver = espnow_outbound_ch.receiver();

    // The S3's USB-C is the native usb-serial-jtag; share it for RNS frames (the
    // serial worker) and esp-println logs (register pokes) — the C6 precedent.
    let (usb_rx, usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE)
        .into_async()
        .split();

    // Four workers — WiFi LAN + USB serial + LoRa + ESP-NOW — all in the manifold.
    let wifi_worker =
        EmbassyAutoInterface::new(INTERFACE_ID, outbound_ch.sender(), &LINK_UP, &WIFI_PEERS);
    let serial_worker = EmbassySerialInterface::new(
        SERIAL_INTERFACE_ID,
        serial_outbound_ch.sender(),
        &SERIAL_LINK_UP,
    );
    let lora_worker =
        EmbassyRnodeLoraInterface::new(LORA_INTERFACE_ID, lora_outbound_ch.sender(), &LORA_LINK_UP);
    let espnow_worker = EmbassyEspNowInterface::new(
        ESPNOW_INTERFACE_ID,
        espnow_outbound_ch.sender(),
        &ESPNOW_LINK_UP,
    );
    let manifold = Manifold::new(
        state,
        [
            HostWorker::Wifi(wifi_worker),
            HostWorker::Serial(serial_worker),
            HostWorker::LoRa(lora_worker),
            HostWorker::EspNow(espnow_worker),
        ],
    );

    spawner.spawn(
        auto_worker_task(stack, sta_mac, inbound_ch.sender(), outbound_rx)
            .expect("spawn auto worker"),
    );
    spawner.spawn(
        serial_worker_task(usb_rx, usb_tx, inbound_ch.sender(), serial_outbound_rx)
            .expect("spawn serial worker"),
    );
    println!("HELTEC_S3 workers spawned (node {dest_hex}); manifold running");

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
                run_lora_worker(
                    lora,
                    LORA_INTERFACE_ID,
                    DEFAULT_915_LORA_PROFILE,
                    inbound_ch.sender(),
                    lora_outbound_rx,
                    &LORA_LINK_UP,
                )
                .await;
            }
            Err(e) => println!("HELTEC_S3 LORA init FAILED: {e:?}"),
        }
    };

    // The ESP-NOW worker: hand it the host's radio adapter (esp-radio sender +
    // receiver) and run. No init handshake — ESP-NOW is connectionless — so the
    // shell is live as soon as it starts.
    let espnow_fut = run_esp_now_worker(
        S3EspNowLink {
            sender: esp_now_sender,
            receiver: esp_now_receiver,
        },
        ESPNOW_INTERFACE_ID,
        inbound_ch.sender(),
        espnow_outbound_rx,
        &ESPNOW_LINK_UP,
    );

    // Run the manifold loop: aggregate the worker's inbound, drive the engine,
    // route egress back, and fire each cycle's snapshot out on SNAPSHOT_WATCH.
    // CSPRNG entropy from the (radio-seeded) RNG per cycle.
    let host = EmbassyHost::new(inbound_rx, || {
        let mut bytes = [0u8; ENGINE_CYCLE_ENTROPY_LEN];
        Rng::new().read(&mut bytes);
        EngineCycleEntropySeed::new(bytes)
    });
    let snapshot_tx = SNAPSHOT_WATCH.sender();
    let manifold_fut = run(manifold, host, move |snapshot: &RuntimeSnapshot| {
        snapshot_tx.send(snapshot.clone());
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
        // Smoothed VBAT (0 mV = uninitialized) so the bar level doesn't jitter
        // on ADC noise.
        let mut vbat_ema_mv: u32 = 0;
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
                            "HELTEC_S3 IFACE {label} online={} dest={} rx={} tx={}",
                            view.online,
                            view.tracked_destinations,
                            view.reticulum_rx_bytes,
                            view.reticulum_tx_bytes,
                        );
                    }
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
                // interface id to its icon/label is the host's job (one match).
                let mut cards: HVec<display::Card, 8> = HVec::new();
                if let Some(snap) = &snapshot {
                    for view in &snap.interfaces {
                        // TEMP (ESP-NOW bring-up): hide the USB card so ESP-NOW —
                        // the 4th interface — fits the 3-card panel and is visible.
                        // Revert when card scrolling lands.
                        if view.id == SERIAL_INTERFACE_ID {
                            continue;
                        }
                        let (kind, label) = if view.id == SERIAL_INTERFACE_ID {
                            (display::CardKind::Usb, "USB")
                        } else if view.id == LORA_INTERFACE_ID {
                            (display::CardKind::LoRa, "LoRa")
                        } else if view.id == ESPNOW_INTERFACE_ID {
                            (display::CardKind::EspNow, "ESP-NOW")
                        } else {
                            (display::CardKind::Wifi, "WiFi LAN")
                        };
                        let _ = cards.push(display::Card {
                            kind,
                            label,
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

    join4(manifold_fut, oled_fut, lora_fut, espnow_fut).await;
}
