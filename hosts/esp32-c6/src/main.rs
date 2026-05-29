#![no_std]
#![no_main]

mod usb_serial;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::main;
use esp_hal::rng::{Rng, TrngSource};
use esp_hal::time::Instant;
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_println::println;

use personal_rns::engine::{DefaultEngineState, InboundPacket, InstantMillis};
use personal_rns::host::HostAdapter;
use personal_rns::interfaces::{Interface, InterfaceId};
use personal_rns::runtime::step;

use usb_serial::Esp32UsbSerialInterface;

esp_app_desc!();

/// Stable id for the C6's USB Serial/JTAG interface. The byte pattern is
/// arbitrary (the engine treats it as opaque) but using `[0xC6; 16]` keeps it
/// visually obvious in logs and aligns with the board family.
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xC6; 16]);

/// Synthetic source id for the boot-time seed announce. Distinct from
/// [`USB_INTERFACE_ID`] so the engine's fanout (registered minus source)
/// includes USB and the rebroadcast actually leaves the device.
const SEED_SOURCE_ID: InterfaceId = InterfaceId::new([0x7A; 16]);

/// One real RNS 1.3.1 announce (the same vector personal-rns's wire/runtime
/// tests validate against). Embedded directly so the boot path doesn't depend
/// on inline hex decoding.
static SEED_ANNOUNCE: &[u8] = include_bytes!("../resources/seed_announce.bin");

/// ESP32-C6 Host adapter: real timer clock + hardware CSPRNG (TRNG-backed
/// Rng), with the on-board USB Serial/JTAG peripheral wired as a single
/// point-to-point interface. No heap: the alloc-free core runs on bare
/// metal with no allocator at all.
///
/// The CSPRNG contract holds via the `TrngSource` constructed in `main`
/// (kept alive for the whole runtime); without it, `Rng` falls back to a
/// silicon PRNG, which would silently make future crypto forgeable.
struct Esp32Host<'d> {
    usb: Esp32UsbSerialInterface<'d>,
    /// Owned backing storage for `drain_inbound_packets`. Holds the boot
    /// seed announce while `seed_pending` is true, then becomes a stale
    /// slice that's never returned again.
    seed_buffer: [InboundPacket<'static>; 1],
    seed_pending: bool,
}

impl<'d> Esp32Host<'d> {
    fn new(usb: Esp32UsbSerialInterface<'d>) -> Self {
        Self {
            usb,
            seed_buffer: [InboundPacket {
                arrived_at: InstantMillis(0),
                source_interface: SEED_SOURCE_ID,
                bytes: SEED_ANNOUNCE,
            }],
            seed_pending: true,
        }
    }
}

impl HostAdapter for Esp32Host<'_> {
    type Error = core::convert::Infallible;

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
        if self.seed_pending {
            self.seed_pending = false;
            Ok(&self.seed_buffer)
        } else {
            Ok(&[])
        }
    }

    fn handle_egress(&mut self, bytes: &[u8], fire_on: &[InterfaceId]) -> Result<(), Self::Error> {
        // USB is the only registered transport, so it's the only id the
        // engine puts in fire_on. Log-and-swallow a write failure (per the
        // HostAdapter contract) so one bad directive can't halt the step.
        for id in fire_on {
            if *id == self.usb.id() {
                if let Err(e) = self.usb.write(bytes) {
                    println!("ESP32C6_HOST_EGRESS_ERR {e:?}");
                }
            }
        }
        Ok(())
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

    let usb = UsbSerialJtag::new(peripherals.USB_DEVICE);
    let usb_iface = Esp32UsbSerialInterface::new(USB_INTERFACE_ID, usb);

    let mut state: DefaultEngineState = DefaultEngineState::default();
    state
        .register_interface(USB_INTERFACE_ID)
        .expect("first interface always fits the registry cap");
    state
        .register_interface(SEED_SOURCE_ID)
        .expect("two interfaces always fit the registry cap");

    let mut host = Esp32Host::new(usb_iface);
    let delay = Delay::new();

    for _ in 0..5 {
        step(&mut state, &mut host).expect("c6 host ops are infallible");
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
        step(&mut state, &mut host).expect("c6 host ops are infallible");
        println!(
            "ESP32C6_HOST_HEARTBEAT count={heartbeat} ticks={}",
            state.tick_count()
        );
        delay.delay_millis(5_000);
    }
}
