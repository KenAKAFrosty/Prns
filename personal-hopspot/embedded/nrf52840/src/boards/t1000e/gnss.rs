use embassy_futures::select::{select, Either};
use embassy_nrf::gpio::{Input, Output};
use embassy_nrf::uarte::Uarte;
use embassy_time::{Duration, Timer};
use prns_core::capabilities::positioning::gnss::{GnssReceiverCommand, GnssSnapshot, NmeaParser};

use crate::runtime::gnss::GnssShared;

const RESET_HOLD: Duration = Duration::from_millis(10);
const RESET_SETTLE: Duration = Duration::from_millis(100);
const RTC_WAKE_HOLD: Duration = Duration::from_millis(3);
const RTC_WAKE_SETTLE: Duration = Duration::from_millis(50);
const PAIR_WAKE_INTERVAL: Duration = Duration::from_millis(40);
const PAIR_WAKE_ATTEMPTS: usize = 25;
const READ_BYTES: usize = 64;
const ERROR_RETRY: Duration = Duration::from_millis(100);

// The AG3335 can remain silent after power-up until this vendor wake command is repeated for
// roughly one second. It changes no persistent receiver configuration.
const PAIR_WAKE: &[u8] = b"$PAIR382,1*2E\r\n";

static STATE: GnssShared = GnssShared::new();

/// The T1000-E-specific AG3335 transport and power controls. NMEA interpretation and the public
/// observation contract live in `prns-core`; this module owns only the board adapter.
pub(crate) struct T1000eGnss {
    uart: Uarte<'static>,
    enable: Output<'static>,
    reset: Output<'static>,
    vrtc_enable: Output<'static>,
    sleep_interrupt: Output<'static>,
    rtc_interrupt: Output<'static>,
    _resetb_out: Input<'static>,
}

impl T1000eGnss {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        uart: Uarte<'static>,
        enable: Output<'static>,
        reset: Output<'static>,
        vrtc_enable: Output<'static>,
        sleep_interrupt: Output<'static>,
        rtc_interrupt: Output<'static>,
        resetb_out: Input<'static>,
    ) -> Self {
        Self {
            uart,
            enable,
            reset,
            vrtc_enable,
            sleep_interrupt,
            rtc_interrupt,
            _resetb_out: resetb_out,
        }
    }

    fn stop(&mut self) {
        self.enable.set_low();
        self.reset.set_low();
        self.sleep_interrupt.set_high();
        self.rtc_interrupt.set_low();
        // Keep the backup domain supplied so ephemeris and RTC state can survive a receiver stop.
        self.vrtc_enable.set_high();
    }

    async fn start(&mut self) -> Result<(), embassy_nrf::uarte::Error> {
        self.vrtc_enable.set_high();
        self.sleep_interrupt.set_high();
        self.rtc_interrupt.set_low();
        self.enable.set_high();

        // RESET is active-high on the T1000-E carrier.
        self.reset.set_high();
        Timer::after(RESET_HOLD).await;
        self.reset.set_low();
        Timer::after(RESET_SETTLE).await;

        // Pulse the AG3335 RTC interrupt, then repeat its volatile wake command. Repetition is
        // intentional: the receiver may not accept UART input immediately after the wake edge.
        self.rtc_interrupt.set_high();
        Timer::after(RTC_WAKE_HOLD).await;
        self.rtc_interrupt.set_low();
        Timer::after(RTC_WAKE_SETTLE).await;
        for _ in 0..PAIR_WAKE_ATTEMPTS {
            self.uart.write(PAIR_WAKE).await?;
            Timer::after(PAIR_WAKE_INTERVAL).await;
        }
        Ok(())
    }
}

pub(crate) fn control(command: GnssReceiverCommand) {
    STATE.control(command);
}

pub(crate) fn snapshot() -> GnssSnapshot {
    STATE.snapshot()
}

pub(crate) async fn drive(mut gnss: T1000eGnss) -> ! {
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
        let mut enabled = true;
        while gnss.start().await.is_err() {
            gnss.stop();
            STATE.publish(GnssSnapshot::Error);
            match select(Timer::after(ERROR_RETRY), STATE.wait()).await {
                Either::First(()) => {
                    STATE.publish(GnssSnapshot::Starting);
                }
                Either::Second(command) => {
                    enabled = command == GnssReceiverCommand::Enable;
                    if !enabled {
                        break;
                    }
                    STATE.publish(GnssSnapshot::Starting);
                }
            }
        }
        if !enabled {
            gnss.stop();
            STATE.publish(GnssSnapshot::Disabled);
            continue;
        }
        let mut parser = NmeaParser::new();
        STATE.publish(GnssSnapshot::Searching { satellites: 0 });

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
