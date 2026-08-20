use core::cell::RefCell;

use embassy_futures::select::{select, Either};
use embassy_nrf::gpio::{Input, Output};
use embassy_nrf::uarte::Uarte;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use prns_core::capabilities::positioning::gnss::{GnssSnapshot, NmeaParser};

const RESET_HOLD: Duration = Duration::from_millis(100);
const READ_BYTES: usize = 64;
const ERROR_RETRY: Duration = Duration::from_millis(100);

static COMMAND: Signal<CriticalSectionRawMutex, bool> = Signal::new();
static SNAPSHOT: Mutex<CriticalSectionRawMutex, RefCell<GnssSnapshot>> =
    Mutex::new(RefCell::new(GnssSnapshot::Disabled));

/// The T096-specific UC6580 transport and power controls. NMEA interpretation and the public
/// observation contract live in `prns-core`; this module owns only the board adapter.
pub(crate) struct T096Gnss {
    uart: Uarte<'static>,
    enable: Output<'static>,
    reset: Output<'static>,
    _pulse_per_second: Input<'static>,
}

impl T096Gnss {
    pub(crate) fn new(
        uart: Uarte<'static>,
        enable: Output<'static>,
        reset: Output<'static>,
        pulse_per_second: Input<'static>,
    ) -> Self {
        Self {
            uart,
            enable,
            reset,
            _pulse_per_second: pulse_per_second,
        }
    }

    fn stop(&mut self) {
        // Both controls are active-low. Holding reset asserted while EN is inactive mirrors the
        // reference firmwares and prevents a disabled receiver from driving stale UART data.
        self.reset.set_low();
        self.enable.set_high();
    }

    async fn start(&mut self) {
        self.reset.set_low();
        self.enable.set_low();
        Timer::after(RESET_HOLD).await;
        self.reset.set_high();
    }
}

pub(crate) fn set_enabled(enabled: bool) {
    publish(if enabled {
        GnssSnapshot::Starting
    } else {
        GnssSnapshot::Disabled
    });
    COMMAND.signal(enabled);
}

pub(crate) fn snapshot() -> GnssSnapshot {
    SNAPSHOT.lock(|snapshot| *snapshot.borrow())
}

pub(crate) async fn drive(mut gnss: T096Gnss) -> ! {
    gnss.stop();
    publish(GnssSnapshot::Disabled);

    loop {
        if !COMMAND.wait().await {
            gnss.stop();
            publish(GnssSnapshot::Disabled);
            continue;
        }

        publish(GnssSnapshot::Starting);
        gnss.start().await;
        let mut parser = NmeaParser::new();
        publish(GnssSnapshot::Searching { satellites: 0 });
        let mut enabled = true;

        while enabled {
            let mut bytes = [0u8; READ_BYTES];
            match select(gnss.uart.read(&mut bytes), COMMAND.wait()).await {
                Either::First(Ok(())) => {
                    for byte in bytes {
                        if let Some(snapshot) = parser.feed(byte) {
                            publish(snapshot);
                        }
                    }
                }
                Either::First(Err(_)) => {
                    publish(GnssSnapshot::Error);
                    match select(Timer::after(ERROR_RETRY), COMMAND.wait()).await {
                        Either::First(()) => {}
                        Either::Second(command) => enabled = command,
                    }
                }
                Either::Second(command) => enabled = command,
            }
        }

        gnss.stop();
        publish(GnssSnapshot::Disabled);
    }
}

fn publish(snapshot: GnssSnapshot) {
    SNAPSHOT.lock(|current| *current.borrow_mut() = snapshot);
}
