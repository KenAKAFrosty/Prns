//! Heltec WiFi LoRa 32 (ESP32-S3 + SX1262 + OLED) host — **Stage A bring-up**.
//!
//! Goal of this stage: prove the whole Personal Reticulum engine builds and runs
//! on the S3's dual-core Xtensa LX7 arch (vs the C6's single-core RISC-V),
//! printing live state over USB-JTAG serial. No OLED (Stage B) and no SX1262
//! LoRa interface (Stage C) yet — those are the substantial follow-ons; this is
//! the lightweight "does our stack run on the new silicon" proof.

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::rng::TrngSource;
use esp_hal::time::Instant;
use esp_println::{print, println};

use personal_rns::engine::{
    tick, DefaultEngineState, InstantMillis, ReannounceSchedule, SelfAnnounceConfig,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

esp_app_desc!();

/// Milliseconds since boot from the SystemTimer — the same source the C6 host
/// uses, so `InstantMillis` means the same thing on both boards.
fn now_millis() -> InstantMillis {
    InstantMillis(Instant::now().duration_since_epoch().as_millis())
}

#[esp_hal::main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    // Hardware TRNG online (the engine's entropy contract wants CSPRNG quality).
    let _trng = TrngSource::new(peripherals.RNG, peripherals.ADC1);
    let delay = Delay::new();

    println!("HELTEC_S3: boot — Personal Reticulum engine on ESP32-S3 (Xtensa LX7, dual-core)");

    // The pinned fixture identity (X25519 0x22 / Ed25519 0x11). Announcing it
    // derives the known `personal.node` destination, so the printed hash is a
    // cross-arch checksum: if Xtensa produces the same c3cfae69… as the x86 host
    // and the RISC-V C6, our crypto + hashing are byte-identical on the S3.
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

    if let Some(dest) = state.self_announced_destination() {
        print!("HELTEC_S3 announce_dest=");
        for byte in dest.as_bytes() {
            print!("{byte:02x}");
        }
        println!(" (expect c3cfae69b36bb6e3bbfd96a3b5867a59)");
    }

    // No interface is registered yet (the SX1262 radio is Stage C), so the
    // engine can't emit — but it still advances its periodic work each tick,
    // which is the point: the engine runs on this arch. The deadline-driven
    // `next_wakeup` is printed too, so we can watch the same scheduling logic the
    // daemon uses behave identically here.
    let mut cycle: u32 = 0;
    loop {
        let now = now_millis();
        let _ = tick(&mut state, now, 0xA5A5_A5A5_A5A5_A5A5);
        cycle = cycle.wrapping_add(1);
        println!(
            "HELTEC_S3_CYCLE {cycle} now_ms={} tick={} routes={} next_wakeup={:?}",
            now.0,
            state.tick_count(),
            state.route_count(),
            state.next_wakeup(now),
        );
        delay.delay_millis(1000);
    }
}
