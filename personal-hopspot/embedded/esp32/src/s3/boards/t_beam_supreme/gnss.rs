use esp_hal::gpio::{Input, Output};
use esp_hal::uart::Uart;
use esp_hal::Async;

use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Timer};

use personal_hopspot_core as screen;

use crate::s3::{GnssProvider, GnssShared};

use super::{Axp2101AccessError, GnssPower, TBeamPmu};

const POWER_SETTLE: Duration = Duration::from_millis(50);
const ERROR_RETRY: Duration = Duration::from_millis(100);
const READ_BYTES: usize = 64;

static STATE: GnssShared = GnssShared::new();

/// The Supreme's L76K/MAX-M10S UART transport and PMU power control. NMEA interpretation and the
/// public positioning types remain in `prns-core`; this adapter owns only the board wiring.
pub struct TBeamSupremeGnss {
    uart: Uart<'static, Async>,
    wake: Output<'static>,
    _pulse_per_second: Input<'static>,
    pmu: &'static TBeamPmu,
}

impl TBeamSupremeGnss {
    pub(super) fn new(
        uart: Uart<'static, Async>,
        wake: Output<'static>,
        pulse_per_second: Input<'static>,
        pmu: &'static TBeamPmu,
    ) -> Self {
        Self {
            uart,
            wake,
            _pulse_per_second: pulse_per_second,
            pmu,
        }
    }

    fn stop(&mut self) {
        // GPIO7 is the L76K wake input and is unconnected on the MAX-M10S variant. ALDO4 is the
        // authoritative power control shared by both receiver options.
        self.wake.set_low();
        if self.pmu.set_gnss_power(GnssPower::Off).is_err() {
            log::warn!("failed to disable T-Beam GNSS power");
        }
    }

    async fn start(&mut self) -> Result<(), Axp2101AccessError> {
        self.wake.set_low();
        self.pmu.set_gnss_power(GnssPower::On)?;
        self.wake.set_high();
        // Begin draining NMEA well before a 9600-baud stream can fill the 128-byte hardware FIFO.
        Timer::after(POWER_SETTLE).await;
        Ok(())
    }
}

impl GnssProvider for TBeamSupremeGnss {
    const AVAILABILITY: screen::GnssAvailability = screen::GnssAvailability::Available;

    fn control(command: screen::GnssReceiverCommand) {
        STATE.control(command);
    }

    fn snapshot() -> Option<screen::GnssSnapshot> {
        Some(STATE.snapshot())
    }

    async fn drive(mut self) {
        self.stop();
        STATE.publish(screen::GnssSnapshot::Disabled);

        loop {
            match STATE.wait_command().await {
                screen::GnssReceiverCommand::Enable => {}
                screen::GnssReceiverCommand::Disable => {
                    self.stop();
                    STATE.publish(screen::GnssSnapshot::Disabled);
                    continue;
                }
            }

            STATE.publish(screen::GnssSnapshot::Starting);
            let mut enabled = true;
            while self.start().await.is_err() {
                self.stop();
                STATE.publish(screen::GnssSnapshot::Error);
                match select(Timer::after(ERROR_RETRY), STATE.wait_command()).await {
                    Either::First(()) => {
                        STATE.publish(screen::GnssSnapshot::Starting);
                    }
                    Either::Second(command) => {
                        enabled = command == screen::GnssReceiverCommand::Enable;
                        if !enabled {
                            break;
                        }
                        STATE.publish(screen::GnssSnapshot::Starting);
                    }
                }
            }
            if !enabled {
                self.stop();
                STATE.publish(screen::GnssSnapshot::Disabled);
                continue;
            }

            let mut parser = screen::NmeaParser::new();
            STATE.publish(screen::GnssSnapshot::Searching { satellites: 0 });

            while enabled {
                let mut bytes = [0u8; READ_BYTES];
                match select(self.uart.read_async(&mut bytes), STATE.wait_command()).await {
                    Either::First(Ok(read)) => {
                        for &byte in &bytes[..read] {
                            if let Some(snapshot) = parser.feed(byte) {
                                STATE.publish(snapshot);
                            }
                        }
                    }
                    Either::First(Err(error)) => {
                        log::warn!("GNSS UART receive failed: {error:?}");
                        STATE.publish(screen::GnssSnapshot::Error);
                        match select(Timer::after(ERROR_RETRY), STATE.wait_command()).await {
                            Either::First(()) => {}
                            Either::Second(command) => {
                                enabled = command == screen::GnssReceiverCommand::Enable;
                            }
                        }
                    }
                    Either::Second(command) => {
                        enabled = command == screen::GnssReceiverCommand::Enable;
                    }
                }
            }

            self.stop();
            STATE.publish(screen::GnssSnapshot::Disabled);
        }
    }
}
