//! Heltec WiFi LoRa 32 (ESP32-S3 + SX1262 + OLED) host.
//!
//! - **Stage A** (done): the Personal Reticulum engine runs on the S3's Xtensa
//!   LX7, state printed over USB-JTAG serial.
//! - **Stage B** (this file): drive the onboard SSD1306 OLED with live state —
//!   visual feedback so the board reads as a real node, not just serial logs.
//! - **Stage C** (next): the SX1262 LoRa radio as the first real RF interface.
//!
//! ## Board pinout
//!
//! Wired for the **Heltec WiFi LoRa 32 V3** (ESP32-S3) — if your silkscreen is a
//! different revision and the screen stays dark, these four constants are the
//! first place to look (Heltec moves pins between revs):
//! - OLED is an SSD1306 128×64 on I2C: `SDA=GPIO17`, `SCL=GPIO18`, `RST=GPIO21`.
//! - `Vext` (`GPIO36`, active-LOW) gates power to the OLED — it stays dark until
//!   this is driven low, which is the classic "correct pins, blank screen" trap.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::rng::TrngSource;
use esp_hal::time::{Instant, Rate};
use esp_println::println;

use core::fmt::Write as _;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};
use heapless::String;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use personal_rns::engine::{
    tick, DefaultEngineState, InstantMillis, NextScheduledWakeup, ReannounceSchedule,
    SelfAnnounceConfig,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

esp_app_desc!();

fn now_millis() -> InstantMillis {
    InstantMillis(Instant::now().duration_since_epoch().as_millis())
}

/// First 4 bytes of a destination as hex, e.g. `c3cfae69` — enough to recognise
/// our node on a 128px-wide line.
fn dest_short(dest_bytes: &[u8], out: &mut String<16>) {
    for byte in dest_bytes.iter().take(4) {
        let _ = write!(out, "{byte:02x}");
    }
}

fn wake_label(wake: NextScheduledWakeup) -> &'static str {
    match wake {
        NextScheduledWakeup::Immediate => "now",
        NextScheduledWakeup::At(_) => "scheduled",
        NextScheduledWakeup::Idle => "idle",
    }
}

#[esp_hal::main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    let _trng = TrngSource::new(peripherals.RNG, peripherals.ADC1);
    let delay = Delay::new();

    println!("HELTEC_S3: boot — Personal Reticulum engine on ESP32-S3 (Xtensa LX7, dual-core)");

    // --- Engine: an announcing node with the pinned fixture identity. ---
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

    let mut dest_hex: String<16> = String::new();
    if let Some(dest) = state.self_announced_destination() {
        dest_short(dest.as_bytes(), &mut dest_hex);
        println!("HELTEC_S3 announce_dest={dest_hex}… (expect c3cfae69…)");
    }

    // --- OLED bring-up (Heltec V3 pinout). ---
    // Power the peripheral rail (Vext active-low) and let it settle before I2C.
    let mut vext = Output::new(peripherals.GPIO36, Level::Low, OutputConfig::default());
    vext.set_low();
    delay.delay_millis(50);
    // Reset pulse for the SSD1306.
    let mut oled_rst = Output::new(peripherals.GPIO21, Level::High, OutputConfig::default());
    oled_rst.set_low();
    delay.delay_millis(20);
    oled_rst.set_high();
    delay.delay_millis(20);

    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("i2c0 config")
    .with_sda(peripherals.GPIO17)
    .with_scl(peripherals.GPIO18);

    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    let oled_ok = match display.init() {
        Ok(()) => {
            println!("HELTEC_S3 OLED init ok");
            true
        }
        Err(e) => {
            // A NACK here usually means a wrong pin / address / Vext — surfaced
            // over serial so we can fix it without a reset loop. Engine runs on.
            println!("HELTEC_S3 OLED init FAILED ({e:?}) — check SDA/SCL/RST/Vext pins");
            false
        }
    };

    let text = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);

    let mut cycle: u32 = 0;
    loop {
        let now = now_millis();
        let _ = tick(&mut state, now, 0xA5A5_A5A5_A5A5_A5A5);
        cycle = cycle.wrapping_add(1);

        let wake = state.next_wakeup(now);
        println!(
            "HELTEC_S3_CYCLE {cycle} now_ms={} tick={} routes={} next_wakeup={wake:?}",
            now.0,
            state.tick_count(),
            state.route_count(),
        );

        if oled_ok {
            display.clear_buffer();
            let mut line: String<24> = String::new();

            let _ = Text::with_baseline("Personal RNS  S3", Point::new(0, 0), text, Baseline::Top)
                .draw(&mut display);

            line.clear();
            let _ = write!(line, "node {dest_hex}");
            let _ = Text::with_baseline(&line, Point::new(0, 13), text, Baseline::Top)
                .draw(&mut display);

            line.clear();
            let _ = write!(line, "tick {} rt {}", state.tick_count(), state.route_count());
            let _ = Text::with_baseline(&line, Point::new(0, 26), text, Baseline::Top)
                .draw(&mut display);

            line.clear();
            let _ = write!(line, "announce: {}", wake_label(wake));
            let _ = Text::with_baseline(&line, Point::new(0, 39), text, Baseline::Top)
                .draw(&mut display);

            line.clear();
            let _ = write!(line, "up {}s", now.0 / 1000);
            let _ = Text::with_baseline(&line, Point::new(0, 52), text, Baseline::Top)
                .draw(&mut display);

            let _ = display.flush();
        }

        delay.delay_millis(1000);
    }
}
