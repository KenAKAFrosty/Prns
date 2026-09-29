use embassy_nrf::config::HfclkSource;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::mode::Blocking;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::rng::Rng;
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_time::{Delay, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use personal_rns::lora::LoRaInterface;
use personal_rns::radios::lr1110::Lr1110;
use static_cell::StaticCell;

use crate::boards::status_led::StatusLed;

use super::radio::board_config;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
});

const RADIO_RESET_HOLD_MS: u64 = 2;

type MuziBaseDuoSpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

type MuziBaseDuoRadio =
    Lr1110<MuziBaseDuoSpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type MuziBaseDuoLoraInterface = LoRaInterface<'static, MuziBaseDuoRadio>;

type MuziBaseDuoUsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

pub(crate) struct MuziBaseDuoHardware {
    pub(crate) usb: MuziBaseDuoUsbDriver,
    pub(crate) vbus: &'static SoftwareVbusDetect,
    pub(crate) radio: MuziBaseDuoRadio,
    pub(crate) status_led: StatusLed,
    pub(crate) button: Input<'static>,
}

pub(crate) struct MuziBaseDuoBoard;

impl MuziBaseDuoBoard {
    pub(crate) async fn initialize<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, Rng<'static, Blocking>) -> R,
    ) -> (R, MuziBaseDuoHardware) {
        let mut nrf_config = config::Config::default();
        nrf_config.hfclk_source = HfclkSource::ExternalXtal;
        nrf_config.gpiote_interrupt_priority = Priority::P2;
        nrf_config.time_interrupt_priority = Priority::P2;
        let peripherals = embassy_nrf::init(nrf_config);

        let identity = {
            let mut nvmc = Nvmc::new(peripherals.NVMC);
            let rng = Rng::new_blocking(peripherals.RNG);
            bootstrap(&mut nvmc, rng)
        };

        interrupt::USBD.set_priority(Priority::P2);
        interrupt::TWISPI0.set_priority(Priority::P3);
        static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
        let vbus = crate::runtime::software_vbus::initialize(&SOFTWARE_VBUS);
        let usb = Driver::new(peripherals.USBD, Irqs, vbus);

        let mut radio_reset = Output::new(peripherals.P1_10, Level::Low, OutputDrive::Standard);
        let mut radio_spim_config = spim::Config::default();
        radio_spim_config.frequency = spim::Frequency::M4;
        let radio_bus = Spim::new(
            peripherals.TWISPI0,
            Irqs,
            peripherals.P1_13,
            peripherals.P1_15,
            peripherals.P1_14,
            radio_spim_config,
        );
        let radio_cs = Output::new(peripherals.P1_12, Level::High, OutputDrive::Standard);
        let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
        let radio_busy = Input::new(peripherals.P1_11, Pull::None);
        let radio_irq = Input::new(peripherals.P1_08, Pull::Down);
        Timer::after_millis(RADIO_RESET_HOLD_MS).await;
        radio_reset.set_high();
        let radio = Lr1110::new(
            radio_spi,
            radio_busy,
            radio_irq,
            radio_reset,
            Delay,
            board_config(),
        );

        let status_led = StatusLed::active_low(Output::new(
            peripherals.P1_03,
            Level::High,
            OutputDrive::Standard,
        ));
        let button = Input::new(peripherals.P0_10, Pull::None);

        (
            identity,
            MuziBaseDuoHardware {
                usb,
                vbus,
                radio,
                status_led,
                button,
            },
        )
    }
}
