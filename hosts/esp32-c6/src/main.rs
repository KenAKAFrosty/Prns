#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::main;
use esp_hal::rng::{Rng, TrngSource};
use esp_hal::time::Instant;
use esp_println::println;

use personal_rns::engine::{DefaultEngineState, InboundPacket, InstantMillis};
use personal_rns::host::HostAdapter;
use personal_rns::interfaces::InterfaceId;
use personal_rns::runtime::step;

esp_app_desc!();

/// ESP32-C6 Host adapter: real timer clock + hardware CSPRNG (TRNG-backed
/// Rng), no transport wired yet — so it reports no inbound and refuses to
/// transmit any non-empty batch, just like every other minimal body so far.
/// No heap: the alloc-free core runs on bare metal with no allocator at all.
///
/// The CSPRNG contract holds via the `TrngSource` constructed in `main`
/// (kept alive for the whole runtime); without it, `Rng` falls back to a
/// silicon PRNG, which would silently make future crypto forgeable.
struct Esp32Host;

#[derive(Debug)]
enum Esp32HostError {
    NoTransport,
}

impl HostAdapter for Esp32Host {
    type Error = Esp32HostError;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
        Ok(InstantMillis(
            Instant::now().duration_since_epoch().as_millis(),
        ))
    }

    fn fill_entropy(&mut self, buf: &mut [u8]) -> Result<(), Self::Error> {
        // `Rng::new()` is a zero-sized handle; `read` blocks just long enough
        // to drain the on-chip RNG FIFO for `buf.len()` bytes. With the
        // `TrngSource` active in main (ADC1 entropy mixed in), this is
        // CSPRNG-grade per the esp-hal docs.
        Rng::new().read(buf);
        Ok(())
    }

    fn drain_inbound_packets(&mut self) -> Result<&[InboundPacket<'_>], Self::Error> {
        Ok(&[])
    }

    fn handle_egress(
        &mut self,
        _bytes: &[u8],
        _fire_on: &[InterfaceId],
    ) -> Result<(), Self::Error> {
        // No transport wired yet — every egress fails honestly until a
        // real interface lands.
        Err(Esp32HostError::NoTransport)
    }
}

#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // Enable the hardware TRNG entropy source (ADC1 + RNG silicon). The
    // returned guard stays alive for the whole program: dropping it would
    // disable the TRNG and silently downgrade `Rng::read` to PRNG output,
    // which is exactly the silent-quality-bug the host's CSPRNG contract
    // promises to avoid. The `_` binding keeps it owned without unused-var
    // noise.
    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);

    println!("ESP32C6_HOST: boot");

    let mut state: DefaultEngineState = DefaultEngineState::default();
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
