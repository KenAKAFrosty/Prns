//! T1000-E (Seeed SenseCAP Wio Tracker) hardware wiring: nRF52840 + Semtech
//! LR1110, no display.
//!
//! The radio SPI + control pin map is encoded here verbatim. The T1000-E has no
//! e-ink panel, button, or frontlight, so `DisplayHardware::driver` is always
//! `None` and the runtime's no-display path (`firmware.rs` matches `eink: None` to
//! `core::future::pending().await`) makes the render loop inert. The
//! `T1000eEink` driver stub exists only so that inert `match eink { Some(epd) => epd, .. }`
//! typechecks; it is never constructed.
//!
//! Non-radio GPIO (status LED, battery AIN, e-ink rail, button, frontlight) are
//! bring-up placeholders pinned to unused GPIO until the full T1000-E peripheral
//! map is confirmed. The LR1110 is wired into `LoRaInterface` via the generic
//! `Radio` trait (`personal_rns::radios::Radio`); the board links into a runnable
//! image modulo flash transport (the Nordic serial DFU bootloader carries the
//! firmware; on-device persistence is not yet wired for this variant).

use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::mode::Blocking;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::rng::Rng;
use embassy_nrf::saadc::{self, ChannelConfig, Config as SaadcConfig, Gain, Reference, Saadc};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb, Peri};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use epd_waveshare::epd1in54_v2::Display1in54;
use personal_rns::radios::lr1110::{BoardConfig, Lr1110, TcxoVoltage};
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
    SAADC => saadc::InterruptHandler;
});

type T1000eSpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

pub(crate) type T1000eRadio =
    Lr1110<T1000eSpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

/// No-physical-display driver stub. The runtime's render loop match-arms on
/// `Option<T1000eEink>`; `DisplayHardware::driver` is always `None`, so the
/// `Some(epd) => epd` arm is never taken and these methods are unreachable. They
/// exist solely so `epd.full_update(panel.buffer()).is_ok()` typechecks against
/// the Ssd1681-shaped call site the shared runtime (`runtime/firmware.rs`) uses.
pub(crate) struct T1000eEink;

impl T1000eEink {
    pub(crate) fn full_update(&mut self, _frame: &[u8]) -> Result<(), core::convert::Infallible> {
        Ok(())
    }
    pub(crate) fn partial_update(&mut self, _frame: &[u8]) -> Result<(), core::convert::Infallible> {
        Ok(())
    }
}

pub(crate) struct T1000eUsbHardware {
    pub(crate) driver: Driver<'static, &'static SoftwareVbusDetect>,
    pub(crate) vbus: &'static SoftwareVbusDetect,
}

pub(crate) struct T1000eFaceHardware {
    pub(crate) battery: Saadc<'static, 1>,
    pub(crate) status_led: Output<'static>,
}

pub(crate) struct T1000eEarlyHardware {
    pub(crate) usb: T1000eUsbHardware,
    pub(crate) face: T1000eFaceHardware,
    pub(crate) deferred: T1000eDeferredHardware,
}

pub(crate) struct T1000eControls {
    pub(crate) button: Input<'static>,
    pub(crate) frontlight: Output<'static>,
}

pub(crate) struct T1000eDisplayHardware {
    pub(crate) driver: Option<T1000eEink>,
    pub(crate) panel: Display1in54,
    pub(crate) _rail: Output<'static>,
}

pub(crate) struct T1000eRuntimeHardware {
    pub(crate) radio: T1000eRadio,
    pub(crate) display: T1000eDisplayHardware,
    pub(crate) controls: T1000eControls,
}

pub(crate) struct T1000eDeferredHardware {
    radio_bus: Peri<'static, peripherals::TWISPI0>,
    radio_sck: Peri<'static, peripherals::P0_11>,
    radio_mosi: Peri<'static, peripherals::P1_09>,
    radio_miso: Peri<'static, peripherals::P1_08>,
    radio_cs: Peri<'static, peripherals::P0_12>,
    radio_busy: Peri<'static, peripherals::P0_07>,
    radio_dio1: Peri<'static, peripherals::P1_01>,
    radio_reset: Peri<'static, peripherals::P1_10>,
    // Bring-up placeholders (T1000-E has no e-ink/button/frontlight). Pinned to
    // unused GPIO until the full peripheral map is confirmed.
    eink_rail: Output<'static>,
    button: Peri<'static, peripherals::P0_14>,
    frontlight: Peri<'static, peripherals::P0_15>,
}

pub(crate) struct T1000eBoard;

impl T1000eBoard {
    pub(crate) fn initialize_identities<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, &mut Rng<'static, Blocking>) -> R,
    ) -> (R, T1000eEarlyHardware) {
        let mut nrf_config = config::Config::default();
        nrf_config.gpiote_interrupt_priority = Priority::P2;
        nrf_config.time_interrupt_priority = Priority::P2;
        let peripherals = embassy_nrf::init(nrf_config);

        let identities = {
            let mut nvmc = Nvmc::new(peripherals.NVMC);
            let mut rng = Rng::new_blocking(peripherals.RNG);
            bootstrap(&mut nvmc, &mut rng)
        };

        // Bring-up placeholders for non-radio GPIO (no e-ink rail, no button, no
        // frontlight on the T1000-E; status LED pin TBD).
        let eink_rail = Output::new(peripherals.P0_13, Level::High, OutputDrive::Standard);
        let status_led = Output::new(peripherals.P0_16, Level::High, OutputDrive::Standard);

        // The SoftDevice reserves P0/P1/P4; keep every app interrupt off those. USB at P2
        // (matches the validated T-Echo bring-up); SPI and SAADC at P3 so a BLE radio event
        // can preempt them.
        interrupt::USBD.set_priority(Priority::P2);
        interrupt::TWISPI0.set_priority(Priority::P3);
        interrupt::SAADC.set_priority(Priority::P3);

        static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
        let vbus: &'static SoftwareVbusDetect =
            &*SOFTWARE_VBUS.init(SoftwareVbusDetect::new(true, true));
        let usb_driver = Driver::new(peripherals.USBD, Irqs, vbus);

        // Battery sense: placeholder AIN2 (P0.04), mirroring the T-Echo until the
        // T1000-E divider/AIN is confirmed. VBAT_mV = raw * 6000 / 4096.
        let mut battery_channel = ChannelConfig::single_ended(peripherals.P0_04);
        battery_channel.reference = Reference::INTERNAL;
        battery_channel.gain = Gain::GAIN1_5;
        let battery = Saadc::new(
            peripherals.SAADC,
            Irqs,
            SaadcConfig::default(),
            [battery_channel],
        );

        let hardware = T1000eEarlyHardware {
            usb: T1000eUsbHardware {
                driver: usb_driver,
                vbus,
            },
            face: T1000eFaceHardware {
                battery,
                status_led,
            },
            deferred: T1000eDeferredHardware {
                radio_bus: peripherals.TWISPI0,
                radio_sck: peripherals.P0_11,
                radio_mosi: peripherals.P1_09,
                radio_miso: peripherals.P1_08,
                radio_cs: peripherals.P0_12,
                radio_busy: peripherals.P0_07,
                radio_dio1: peripherals.P1_01,
                radio_reset: peripherals.P1_10,
                eink_rail,
                button: peripherals.P0_14,
                frontlight: peripherals.P0_15,
            },
        };
        (identities, hardware)
    }
}

impl T1000eDeferredHardware {
    pub(crate) async fn finish(self) -> T1000eRuntimeHardware {
        let mut radio_spim_config = spim::Config::default();
        radio_spim_config.frequency = spim::Frequency::M4;
        let radio_bus = Spim::new(
            self.radio_bus,
            Irqs,
            self.radio_sck,
            self.radio_mosi,
            self.radio_miso,
            radio_spim_config,
        );
        let radio_cs = Output::new(self.radio_cs, Level::High, OutputDrive::Standard);
        let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
        let radio_busy = Input::new(self.radio_busy, Pull::None);
        let radio_dio1 = Input::new(self.radio_dio1, Pull::None);
        let radio_reset = Output::new(self.radio_reset, Level::High, OutputDrive::Standard);
        let radio = Lr1110::new(
            radio_spi,
            radio_busy,
            radio_dio1,
            radio_reset,
            Delay,
            BoardConfig {
                // The LR1110's RF switch and TCXO are internal (driven via radio
                // DIOs), so unlike the SX1262 there is no `dio2_as_rf_switch` /
                // external TCXO GPIO here.
                tcxo_voltage: Some(TcxoVoltage::V1_8),
                use_dcdc: true,
                rx_boost: true,
                external_rx_gain_db: 0,
            },
        );

        T1000eRuntimeHardware {
            radio,
            display: T1000eDisplayHardware {
                driver: None,
                panel: Display1in54::default(),
                _rail: self.eink_rail,
            },
            controls: T1000eControls {
                button: Input::new(self.button, Pull::Up),
                frontlight: Output::new(self.frontlight, Level::Low, OutputDrive::Standard),
            },
        }
    }
}