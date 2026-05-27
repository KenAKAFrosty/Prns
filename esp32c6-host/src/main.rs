#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::main;
use esp_hal::time::Instant;
use esp_println::println;

use personal_rns::engine::{EngineState, InboundPacket, InstantMillis};
use personal_rns::host::Host;
use personal_rns::runtime::step;

esp_app_desc!();

/// ESP32-C6 Host adapter: real timer clock, no transport wired yet — so it
/// reports no inbound and refuses to transmit, just like every other minimal
/// body so far. No heap: the alloc-free core runs on bare metal with no
/// allocator at all.
struct Esp32Host;

#[derive(Debug)]
enum Esp32HostError {
    NoTransport,
}

impl Host for Esp32Host {
    type Error = Esp32HostError;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
        Ok(InstantMillis(Instant::now().duration_since_epoch().as_millis()))
    }

    fn drain_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
        Ok(&[])
    }

    fn transmit_packet(&mut self, _bytes: &[u8]) -> Result<(), Self::Error> {
        Err(Esp32HostError::NoTransport)
    }
}

#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let _peripherals = esp_hal::init(config);

    println!("ESP32C6_HOST: boot");

    let mut state = EngineState::default();
    let mut host = Esp32Host;
    let delay = Delay::new();

    for _ in 0..5 {
        step(&mut state, &mut host).expect("clock-only step cannot fail");
        delay.delay_millis(10);
    }
    let now = host.now_millis().expect("c6 timer is readable");
    println!(
        "ESP32C6_HOST_OK ticks={} now_ms={}",
        state.tick_count(),
        now.0
    );

    let mut heartbeat: u32 = 0;
    loop {
        heartbeat = heartbeat.wrapping_add(1);
        step(&mut state, &mut host).expect("clock-only step cannot fail");
        println!(
            "ESP32C6_HOST_HEARTBEAT count={heartbeat} ticks={}",
            state.tick_count()
        );
        delay.delay_millis(5_000);
    }
}
