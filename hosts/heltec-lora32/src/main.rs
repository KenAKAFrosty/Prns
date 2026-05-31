//! Heltec WiFi LoRa 32 (ESP32-S3 + SX1262 + OLED) host.
//!
//! - **Stage A/B** (done): the Personal Reticulum engine runs on the S3, with
//!   live state on the OLED.
//! - **RNSAutoInterface Milestone 1** (this file): associate to the WiFi AP via
//!   `esp-radio`, the first step toward the RNS-compatible UDP-multicast LAN
//!   interface. esp-radio is async-first, so this brings up an embassy executor
//!   (`#[esp_rtos::main]`); the engine tick + OLED now live in an async loop.
//!   No IP stack / multicast yet (M2+).
//!
//! Board: Heltec WiFi LoRa 32 V3 (ESP32-S3). OLED `SDA=17 SCL=18 RST=21`,
//! `Vext=GPIO36` (active-low). WiFi creds come from build-time env
//! `WIFI_SSID` / `WIFI_PASSWORD` so they never enter source.

#![no_std]
#![no_main]

extern crate alloc;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::time::{Instant, Rate};
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;

use embassy_executor::Spawner;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{Config as NetConfig, IpAddress, Ipv6Address, Runner, StackResources};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;

use core::fmt::Write as _;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};
use heapless::String as HString;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{self, Config as WifiConfig, Interface as WifiStaInterface};

use personal_rns::engine::{
    tick, DefaultEngineState, InstantMillis, ReannounceSchedule, SelfAnnounceConfig,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

esp_app_desc!();

/// WiFi credentials, baked in at build time (never committed to source):
/// `WIFI_SSID="…" WIFI_PASSWORD="…" cargo build --release`.
const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

fn now_millis() -> InstantMillis {
    InstantMillis(Instant::now().duration_since_epoch().as_millis())
}

/// Busy-wait a few ms during setup (before the async loop runs).
fn block_ms(ms: u64) {
    let target = Instant::now().duration_since_epoch().as_millis() + ms;
    while Instant::now().duration_since_epoch().as_millis() < target {}
}

/// The embassy-net background task: polls the WiFi device and runs the IP stack.
/// Must own the device + resources for 'static, hence the `StaticCell` below.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiStaInterface<'static>>) -> ! {
    runner.run().await
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // esp-radio needs a heap and a preemptive scheduler, started before the radio.
    esp_alloc::heap_allocator!(size: 72 * 1024);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    println!("HELTEC_S3: boot — Personal Reticulum on ESP32-S3, WiFi bring-up (RNSAutoInterface M1)");

    // --- Engine: announcing node, pinned fixture identity. ---
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);
    let mut state: DefaultEngineState = DefaultEngineState::announcing(
        &secret_key,
        SelfAnnounceConfig {
            app_name: "personal",
            aspects: &["node"],
            app_data: b"heltec-s3",
            schedule: ReannounceSchedule::default(),
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

    // Show "connecting" before we block on association.
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
    let sta = StationConfig::default()
        .with_ssid(WIFI_SSID)
        .with_password(WIFI_PASSWORD.into());
    controller
        .set_config(&WifiConfig::Station(sta))
        .expect("set STA config");

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

    // --- M2: IP stack (embassy-net) with SLAAC → IPv6 link-local. ---
    // Capture the STA MAC before the device moves into the stack; we use it to
    // report the link-local (embassy-net assigns it from the MAC via EUI-64, but
    // config_v6() only surfaces static/global addresses, not link-local).
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
    // The SLAAC link-local: fe80::<EUI-64 of the MAC> (U/L bit flipped).
    let h0 = (((sta_mac[0] ^ 0x02) as u16) << 8) | sta_mac[1] as u16;
    let h1 = ((sta_mac[2] as u16) << 8) | 0x00ff;
    let h2 = 0xfe00u16 | sta_mac[3] as u16;
    let h3 = ((sta_mac[4] as u16) << 8) | sta_mac[5] as u16;
    println!("HELTEC_S3 NET link up; IPv6 link-local fe80::{h0:x}:{h1:x}:{h2:x}:{h3:x}");
    let mut ip6_line: HString<24> = HString::new();
    let _ = write!(ip6_line, "ll ..{h2:x}:{h3:x}");

    // --- M3: join the RNS discovery multicast group and beacon on it. ---
    // group "reticulum" → sha256 → ff12:0:d70b:fb1c:16e4:5e39:485e:31e1 (RNS
    // temporary + link-local scope), discovery port 29716. (RNS AutoInterface.)
    const RNS_DISCOVERY_PORT: u16 = 29716;
    let rns_group = Ipv6Address::new(
        0xff12, 0x0, 0xd70b, 0xfb1c, 0x16e4, 0x5e39, 0x485e, 0x31e1,
    );
    match stack.join_multicast_group(IpAddress::Ipv6(rns_group)) {
        Ok(()) => println!("HELTEC_S3 MCAST joined ff12:0:d70b:fb1c:16e4:5e39:485e:31e1"),
        Err(e) => println!("HELTEC_S3 MCAST join failed: {e:?}"),
    }

    let mut rx_meta = [PacketMetadata::EMPTY; 8];
    let mut rx_buf = [0u8; 512];
    let mut tx_meta = [PacketMetadata::EMPTY; 8];
    let mut tx_buf = [0u8; 512];
    let mut sock = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buf,
        &mut tx_meta,
        &mut tx_buf,
    );
    sock.bind(RNS_DISCOVERY_PORT).expect("bind discovery port");

    // Our link-local as 16 bytes — the beacon payload (RNS discovery beacons
    // carry the sender's link-local so peers can unicast back). Tagged so the
    // laptop sniffer can recognise us this milestone.
    let link_local: [u8; 16] = [
        0xfe, 0x80, 0, 0, 0, 0, 0, 0,
        sta_mac[0] ^ 0x02, sta_mac[1], sta_mac[2], 0xff,
        0xfe, sta_mac[3], sta_mac[4], sta_mac[5],
    ];
    let mut beacon = [0u8; 32];
    beacon[..16].copy_from_slice(b"PERSONAL-RNS-S3:");
    beacon[16..].copy_from_slice(&link_local);

    // --- Engine loop. ---
    let mut beacons: u32 = 0;
    let _controller = controller; // keep the radio alive (dropping disconnects)
    let mut cycle: u32 = 0;
    loop {
        let now = now_millis();
        let _ = tick(&mut state, now, 0xA5A5_A5A5_A5A5_A5A5);
        cycle = cycle.wrapping_add(1);

        // Beacon on the RNS discovery group.
        match sock
            .send_to(&beacon, (IpAddress::Ipv6(rns_group), RNS_DISCOVERY_PORT))
            .await
        {
            Ok(()) => beacons = beacons.wrapping_add(1),
            Err(e) => println!("HELTEC_S3 BEACON send err: {e:?}"),
        }

        println!(
            "HELTEC_S3_CYCLE {cycle} now_ms={} tick={} {wifi_line} beacons={beacons}",
            now.0,
            state.tick_count(),
        );
        if oled_ok {
            display.clear_buffer();
            let _ = Text::with_baseline("Personal RNS  S3", Point::new(0, 0), text, Baseline::Top)
                .draw(&mut display);
            let mut l: HString<24> = HString::new();
            let _ = write!(l, "node {dest_hex}");
            let _ = Text::with_baseline(&l, Point::new(0, 13), text, Baseline::Top)
                .draw(&mut display);
            let _ = Text::with_baseline(wifi_line, Point::new(0, 26), text, Baseline::Top)
                .draw(&mut display);
            let _ = Text::with_baseline(&ip6_line, Point::new(0, 39), text, Baseline::Top)
                .draw(&mut display);
            l.clear();
            let _ = write!(l, "mcast beacons {beacons}");
            let _ = Text::with_baseline(&l, Point::new(0, 52), text, Baseline::Top)
                .draw(&mut display);
            let _ = display.flush();
        }
        Timer::after(Duration::from_secs(1)).await;
    }
}
