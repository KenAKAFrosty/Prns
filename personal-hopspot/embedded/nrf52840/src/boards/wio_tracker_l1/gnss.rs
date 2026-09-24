use embassy_futures::select::{select, Either};
use embassy_nrf::gpio::Output;
use embassy_nrf::uarte::Uarte;
use embassy_time::{Duration, Timer};
use prns_core::capabilities::positioning::gnss::{GnssReceiverCommand, GnssSnapshot, NmeaParser};

use crate::runtime::gnss::GnssShared;

const WAKE_SETTLE: Duration = Duration::from_millis(10);
const READ_BYTES: usize = 64;
const ERROR_RETRY: Duration = Duration::from_millis(100);

static STATE: GnssShared = GnssShared::new();

/// The Wio Tracker L1's Quectel L76K transport and standby control. The receiver has no switched
/// supply on this board; its active-low standby input (P1.09) parks it between uses. NMEA
/// interpretation and the public observation contract live in `prns-core`.
pub(crate) struct WioGnss {
    uart: Uarte<'static>,
    wake: Output<'static>,
}

impl WioGnss {
    pub(crate) fn new(uart: Uarte<'static>, wake: Output<'static>) -> Self {
        Self { uart, wake }
    }

    fn stop(&mut self) {
        self.wake.set_low();
    }

    async fn start(&mut self) {
        self.wake.set_high();
        Timer::after(WAKE_SETTLE).await;
    }
}

pub(crate) fn control(command: GnssReceiverCommand) {
    STATE.control(command);
}

pub(crate) fn snapshot() -> GnssSnapshot {
    STATE.snapshot()
}

pub(crate) async fn drive(mut gnss: WioGnss) -> ! {
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
