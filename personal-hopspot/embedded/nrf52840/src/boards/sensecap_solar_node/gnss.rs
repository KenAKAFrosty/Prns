use embassy_futures::select::{select, Either};
use embassy_nrf::gpio::Output;
use embassy_nrf::uarte::Uarte;
use embassy_time::{Duration, Timer};
use prns_core::capabilities::positioning::gnss::{GnssReceiverCommand, GnssSnapshot, NmeaParser};

use crate::runtime::gnss::GnssShared;

/// Time for the TPS22916 load switch to bring the receiver's 3V3 rail up before it is addressed.
const POWER_SETTLE: Duration = Duration::from_millis(50);
const RESET_HOLD: Duration = Duration::from_millis(10);
const RESET_SETTLE: Duration = Duration::from_millis(100);
const READ_BYTES: usize = 64;
const ERROR_RETRY: Duration = Duration::from_millis(100);

static STATE: GnssShared = GnssShared::new();

/// The Solar Node's XIAO L76K transport and power controls. NMEA interpretation and the public
/// observation contract live in `prns-core`; this module owns only the board adapter.
///
/// Unlike the T1000-E's AG3335, the L76K needs no vendor wake command: it emits NMEA at 9600 baud
/// as soon as its rail is up and reset is released.
pub(crate) struct SolarNodeGnss {
    uart: Uarte<'static>,
    /// Active-high gate on the receiver's dedicated TPS22916 load switch, so a disabled receiver
    /// is genuinely unpowered rather than merely idle. That matters on a solar budget.
    enable: Output<'static>,
    /// Active-low RESET on the L76K.
    reset: Output<'static>,
    /// Held high to keep the receiver out of standby while it is enabled.
    wakeup: Output<'static>,
}

impl SolarNodeGnss {
    pub(crate) fn new(
        uart: Uarte<'static>,
        enable: Output<'static>,
        reset: Output<'static>,
        wakeup: Output<'static>,
    ) -> Self {
        Self {
            uart,
            enable,
            reset,
            wakeup,
        }
    }

    fn stop(&mut self) {
        // Drop the control lines before removing the rail so no pin back-powers an unpowered part.
        self.wakeup.set_low();
        self.reset.set_low();
        self.enable.set_low();
    }

    async fn start(&mut self) {
        self.reset.set_low();
        self.wakeup.set_low();
        self.enable.set_high();
        Timer::after(POWER_SETTLE).await;

        Timer::after(RESET_HOLD).await;
        self.reset.set_high();
        self.wakeup.set_high();
        Timer::after(RESET_SETTLE).await;
    }
}

pub(crate) fn control(command: GnssReceiverCommand) {
    STATE.control(command);
}

pub(crate) fn snapshot() -> GnssSnapshot {
    STATE.snapshot()
}

pub(crate) async fn drive(mut gnss: SolarNodeGnss) -> ! {
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
