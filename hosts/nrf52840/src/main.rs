//! nRF52840 Dongle (PCA10059) host: USB CDC heartbeat + engine touch.
//!
//! Phase 2: exposes a USB CDC ACM device so the host can read engine
//! status. Every second we write a heartbeat line (`tick=N route=N
//! held=N\r\n`) so a `cat /dev/ttyACMx` on the laptop sees the engine is
//! actually running on the dongle and its state is moving. The green LED
//! (LD1, P0_06) blinks at 1 Hz independently as a visible "alive" signal.

#![no_std]
#![no_main]

use core::fmt::Write as _;

use panic_halt as _;

use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, gpio, peripherals, usb};
use embassy_time::{Duration, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, Config};
use heapless::String;
use static_cell::StaticCell;

use personal_rns::engine::EngineState;
use personal_rns::routing::storage::FixedInline;

/// nRF-tuned preset: 64 tracked destinations, 16 held — ~35 KiB engine
/// state, comfortable in the 256 KiB SRAM alongside the USB stack.
type NrfEngineState = EngineState<FixedInline<64, 32, 4096, 4, 256, 16, 8, 8, 8, 128>>;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    POWER_CLOCK => usb::vbus_detect::InterruptHandler;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) -> ! {
    // USB needs HFCLK sourced from the external 16 MHz crystal; default is
    // internal RC which is not USB-compatible. PCA10059 has the crystal
    // populated.
    let mut nrf_config = embassy_nrf::config::Config::default();
    nrf_config.hfclk_source = embassy_nrf::config::HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(nrf_config);

    // Engine state lives in BSS — too big for the executor's task stack.
    static ENGINE: StaticCell<NrfEngineState> = StaticCell::new();
    let state = ENGINE.init(NrfEngineState::default());

    // PCA10059 LD1 (green) on P0_06; active-low.
    let mut led = gpio::Output::new(p.P0_06, gpio::Level::High, gpio::OutputDrive::Standard);

    let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));

    let mut config = Config::new(0x1209, 0x0001);
    config.manufacturer = Some("Stay Personal");
    config.product = Some("Reticulum nRF52840 Test");
    config.serial_number = Some("PERSONAL-RNS-001");
    config.max_packet_size_0 = 64;

    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static USB_STATE: StaticCell<State> = StaticCell::new();

    let mut builder = Builder::new(
        driver,
        config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 256]),
        CONTROL_BUF.init([0; 64]),
    );
    let mut class = CdcAcmClass::new(&mut builder, USB_STATE.init(State::new()), 64);
    let mut usb = builder.build();

    let usb_fut = usb.run();

    let heartbeat_fut = async {
        loop {
            class.wait_connection().await;
            // Once connected, keep writing one line per second until the
            // host disconnects.
            loop {
                let mut line: String<64> = String::new();
                let _ = write!(
                    line,
                    "tick={} route={} held={}\r\n",
                    state.tick_count(),
                    state.route_count(),
                    state.held_announce_count()
                );
                if class.write_packet(line.as_bytes()).await.is_err() {
                    break;
                }
                Timer::after(Duration::from_millis(1000)).await;
            }
        }
    };

    let blink_fut = async {
        loop {
            led.set_low();
            Timer::after(Duration::from_millis(500)).await;
            led.set_high();
            Timer::after(Duration::from_millis(500)).await;
        }
    };

    join3(usb_fut, heartbeat_fut, blink_fut).await;
    loop {}
}
