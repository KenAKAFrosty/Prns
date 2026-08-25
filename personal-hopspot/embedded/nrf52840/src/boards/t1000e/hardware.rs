use embassy_nrf::config::HfclkSource;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::mode::Blocking;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::rng::Rng;
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::uarte::{self, Uarte};
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_time::{Delay, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use personal_rns::lora::LoRaInterface;
use personal_rns::radios::lr1110::Lr1110;

use crate::boards::status_led::StatusLed;

use super::radio::board_config;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
    SPI2 => spim::InterruptHandler<peripherals::SPI2>;
    UARTE0 => uarte::InterruptHandler<peripherals::UARTE0>;
});

type T1000eSpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

type T1000eRadio = Lr1110<T1000eSpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type T1000eLoraInterface = LoRaInterface<'static, T1000eRadio>;

type T1000eUsbDriver = Driver<'static, HardwareVbusDetect>;

pub(crate) struct T1000eHardware {
    pub(crate) flash: Nvmc<'static>,
    pub(crate) usb: T1000eUsbDriver,
    pub(crate) radio: T1000eRadio,
    pub(crate) status_led: StatusLed,
    pub(crate) gnss: super::Gnss,
}

pub(crate) struct T1000eBoard;

impl T1000eBoard {
    pub(crate) async fn initialize<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, &mut Rng<'static, Blocking>) -> R,
    ) -> (R, T1000eHardware) {
        let mut nrf_config = config::Config::default();
        nrf_config.hfclk_source = HfclkSource::ExternalXtal;
        nrf_config.gpiote_interrupt_priority = Priority::P2;
        nrf_config.time_interrupt_priority = Priority::P2;
        let peripherals = embassy_nrf::init(nrf_config);

        let (identity, flash) = {
            let mut nvmc = Nvmc::new(peripherals.NVMC);
            let mut rng = Rng::new_blocking(peripherals.RNG);
            let identity = bootstrap(&mut nvmc, &mut rng);
            (identity, nvmc)
        };

        interrupt::USBD.set_priority(Priority::P2);
        interrupt::UARTE0.set_priority(Priority::P3);
        let usb = Driver::new(peripherals.USBD, Irqs, HardwareVbusDetect::new(Irqs));

        let gnss = {
            let uart = Uarte::new(
                peripherals.UARTE0,
                peripherals.P0_14,
                peripherals.P0_13,
                Irqs,
                uarte::Config::default(),
            );
            super::Gnss::new(
                uart,
                Output::new(peripherals.P1_11, Level::Low, OutputDrive::Standard),
                Output::new(peripherals.P1_15, Level::Low, OutputDrive::Standard),
                Output::new(peripherals.P0_08, Level::High, OutputDrive::Standard),
                Output::new(peripherals.P1_12, Level::High, OutputDrive::Standard),
                Output::new(peripherals.P0_15, Level::Low, OutputDrive::Standard),
                Input::new(peripherals.P1_14, Pull::Up),
            )
        };

        let mut radio_reset = Output::new(peripherals.P1_10, Level::Low, OutputDrive::Standard);
        Timer::after_millis(500).await;

        let mut radio_spim_config = spim::Config::default();
        radio_spim_config.frequency = spim::Frequency::M4;
        let radio_bus = Spim::new(
            peripherals.SPI2,
            Irqs,
            peripherals.P0_11,
            peripherals.P1_08,
            peripherals.P1_09,
            radio_spim_config,
        );
        let radio_cs = Output::new(peripherals.P0_12, Level::High, OutputDrive::Standard);
        let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
        let radio_busy = Input::new(peripherals.P0_07, Pull::None);
        let radio_dio1 = Input::new(peripherals.P1_01, Pull::Down);
        radio_reset.set_high();
        let radio = Lr1110::new(
            radio_spi,
            radio_busy,
            radio_dio1,
            radio_reset,
            Delay,
            board_config(),
        );

        let status_led = StatusLed::active_high(Output::new(
            peripherals.P0_24,
            Level::Low,
            OutputDrive::Standard,
        ));

        (
            identity,
            T1000eHardware {
                flash,
                usb,
                radio,
                status_led,
                gnss,
            },
        )
    }
}
