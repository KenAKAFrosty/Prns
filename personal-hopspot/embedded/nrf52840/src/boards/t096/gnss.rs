use embassy_futures::select::{select, Either};
use embassy_nrf::gpio::{Input, Output};
use embassy_nrf::uarte::Uarte;
use embassy_time::{Duration, Timer};
use prns_core::capabilities::positioning::gnss::{GnssReceiverCommand, GnssSnapshot, NmeaParser};

use crate::runtime::gnss::GnssShared;

const RESET_HOLD: Duration = Duration::from_millis(100);
const READ_BYTES: usize = 64;
const ERROR_RETRY: Duration = Duration::from_millis(100);

static STATE: GnssShared = GnssShared::new();

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

pub(crate) fn control(command: GnssReceiverCommand) {
    STATE.control(command);
}

pub(crate) fn snapshot() -> GnssSnapshot {
    STATE.snapshot()
}

pub(crate) async fn drive(mut gnss: T096Gnss) -> ! {
    gnss.stop();
    STATE.publish(GnssSnapshot::Disabled);

    loop {
        match STATE.wait().await {
            GnssReceiverCommand::Enable => {}
            GnssReceiverCommand::Disable => {
                gnss.stop();
                STATE.publish(GnssSnapshot::Disabled);
                continue;
            }
        }

        STATE.publish(GnssSnapshot::Starting);
        gnss.start().await;
        let mut parser = NmeaParser::new();
        STATE.publish(GnssSnapshot::Searching { satellites: 0 });
        let mut enabled = true;

        while enabled {
            let mut bytes = [0u8; READ_BYTES];
            match select(gnss.uart.read(&mut bytes), STATE.wait()).await {
                Either::First(Ok(())) => {
                    for byte in bytes {
                        if let Some(snapshot) = parser.feed(byte) {
                            STATE.publish(snapshot);
                        }
                    }
                }
                Either::First(Err(_)) => {
                    STATE.publish(GnssSnapshot::Error);
                    match select(Timer::after(ERROR_RETRY), STATE.wait()).await {
                        Either::First(()) => {}
                        Either::Second(command) => {
                            enabled = command == GnssReceiverCommand::Enable;
                        }
                    }
                }
                Either::Second(command) => {
                    enabled = command == GnssReceiverCommand::Enable;
                }
            }
        }

        gnss.stop();
        STATE.publish(GnssSnapshot::Disabled);
    }
}
