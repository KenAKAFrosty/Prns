#![no_std]
#![no_main]

mod coordinator;
mod usb_serial;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::main;
use esp_hal::rng::TrngSource;
use esp_hal::time::Instant;
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_println::println;

use personal_rns::engine::{DefaultEngineState, InstantMillis};
use personal_rns::interfaces::{InterfaceId, NoAllocLoopback};

use coordinator::{new_loopback_queue, DualInterfaceCoordinator};
use usb_serial::Esp32UsbSerialInterface;

esp_app_desc!();

/// Stable interface ids (opaque to the engine; byte patterns chosen for log
/// legibility — `C6` for USB, `10`/`11` for the loopback pair).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xC6; 16]);
const LOOPBACK_INTERFACE_ID: InterfaceId = InterfaceId::new([0x10; 16]);
const LOOPBACK_ECHO_WIRE_ID: InterfaceId = InterfaceId::new([0x11; 16]);

/// Synthetic, deliberately *unregistered* source for the boot seed, so its
/// rebroadcast fans out to both registered interfaces rather than being
/// excluded as the source.
const SEED_SOURCE_ID: InterfaceId = InterfaceId::new([0x7A; 16]);

/// One genuine RNS 1.3.1 announce (the same vector personal-rns's wire tests
/// validate against). Embedded so the boot path needs no hex decoding.
static SEED_ANNOUNCE: &[u8] = include_bytes!("../resources/seed_announce.bin");

/// Bounded, logged demonstration phase. 16 × 100 ms spans the 500 ms
/// rebroadcast jitter window, so we always capture the seed's fanout to both
/// interfaces and the loopback echo getting deduped.
const DEMONSTRATION_STEPS: u32 = 16;
const STEP_INTERVAL_MS: u32 = 100;
const HEARTBEAT_INTERVAL_MS: u32 = 5_000;

fn now_millis() -> InstantMillis {
    InstantMillis(Instant::now().duration_since_epoch().as_millis())
}

#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // Hardware TRNG entropy source; the guard stays alive for the whole run so
    // `Rng::read` never silently downgrades to a non-CSPRNG PRNG.
    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);

    println!("ESP32C6_HOST: boot (spike-a: sync dual-interface coordinator)");

    let usb =
        Esp32UsbSerialInterface::new(USB_INTERFACE_ID, UsbSerialJtag::new(peripherals.USB_DEVICE));

    // Caller-owned loopback queues: both halves borrow these for their whole
    // lifetime, so they must outlive the coordinator (alloc-free, on the
    // stack). `registered_to_echo` carries what the engine transmits on the
    // registered half toward the reflector; `echo_to_registered` carries the
    // reflected echo back.
    let registered_to_echo = new_loopback_queue();
    let echo_to_registered = new_loopback_queue();
    let (loopback, echo_wire) = NoAllocLoopback::pair(
        LOOPBACK_INTERFACE_ID,
        LOOPBACK_ECHO_WIRE_ID,
        &registered_to_echo,
        &echo_to_registered,
    );

    let mut coordinator =
        DualInterfaceCoordinator::new(usb, loopback, echo_wire, SEED_ANNOUNCE, SEED_SOURCE_ID);

    let mut state: DefaultEngineState = DefaultEngineState::default();
    coordinator
        .register(&mut state)
        .expect("usb + loopback are connected and transmit");
    println!(
        "ESP32C6_HOST: registered {} interfaces",
        state.registered_interfaces().len()
    );

    let delay = Delay::new();

    // Demonstration: the boot seed enters as an unregistered source, so once
    // the 500 ms jitter window elapses its rebroadcast fans out to BOTH
    // interfaces (egress=2); the loopback echoes it back and the engine dedups
    // the duplicate on a later step.
    for n in 0..DEMONSTRATION_STEPS {
        let summary = coordinator.step(&mut state, now_millis());
        println!(
            "ESP32C6_STEP {n} in_usb={} in_loop={} seeded={} egress={} accepted={} routes={} ticks={}",
            summary.inbound_from_usb as u8,
            summary.inbound_from_loopback as u8,
            summary.seeded as u8,
            summary.egress_dispatches,
            summary.accepted_announces,
            state.route_count(),
            state.tick_count(),
        );
        delay.delay_millis(STEP_INTERVAL_MS);
    }

    println!(
        "ESP32C6_SPIKE_A_OK routes={} ingested={} ticks={}",
        state.route_count(),
        state.ingested_packet_count(),
        state.tick_count()
    );

    // Liveness heartbeat: keep coordinating (now quiescent) so the link stays
    // up and steady-state behaviour is observable.
    let mut beat: u32 = 0;
    loop {
        beat = beat.wrapping_add(1);
        let summary = coordinator.step(&mut state, now_millis());
        println!(
            "ESP32C6_HEARTBEAT beat={beat} egress={} routes={} ticks={}",
            summary.egress_dispatches,
            state.route_count(),
            state.tick_count()
        );
        delay.delay_millis(HEARTBEAT_INTERVAL_MS);
    }
}
