use embassy_nrf::config::HfclkSource;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::mode::Blocking;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::rng::Rng;
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_time::{Delay, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use personal_rns::lora::LoRaInterface;
use personal_rns::radios::sx126x::{BoardConfig, Sx126x, TcxoVoltage};

use crate::boards::status_led::StatusLed;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
});

type T114SpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

type T114Radio = Sx126x<T114SpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type T114LoraInterface = LoRaInterface<'static, T114Radio>;

type T114UsbDriver = Driver<'static, HardwareVbusDetect>;

pub(crate) struct T114Hardware {
    pub(crate) usb: T114UsbDriver,
    pub(crate) radio: T114Radio,
    pub(crate) status_led: StatusLed,
}

pub(crate) struct T114Board;

impl T114Board {
    pub(crate) async fn initialize<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, &mut Rng<'static, Blocking>) -> R,
    ) -> (R, T114Hardware) {
        let mut nrf_config = config::Config::default();
        nrf_config.hfclk_source = HfclkSource::ExternalXtal;
        nrf_config.gpiote_interrupt_priority = Priority::P2;
        nrf_config.time_interrupt_priority = Priority::P2;
        let peripherals = embassy_nrf::init(nrf_config);

        let identity = {
            let mut nvmc = Nvmc::new(peripherals.NVMC);
            let mut rng = Rng::new_blocking(peripherals.RNG);
            bootstrap(&mut nvmc, &mut rng)
        };

        interrupt::USBD.set_priority(Priority::P2);
        let usb = Driver::new(peripherals.USBD, Irqs, HardwareVbusDetect::new(Irqs));

        let mut radio_spim_config = spim::Config::default();
        radio_spim_config.frequency = spim::Frequency::M4;
        let radio_sck = peripherals.P0_19;
        let radio_miso = peripherals.P0_23;
        let radio_mosi = peripherals.P0_22;
        let radio_bus = Spim::new(
            peripherals.TWISPI0,
            Irqs,
            radio_sck,
            radio_miso,
            radio_mosi,
            radio_spim_config,
        );
        let radio_cs = Output::new(peripherals.P0_24, Level::High, OutputDrive::Standard);
        let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
        let radio_busy = Input::new(peripherals.P0_17, Pull::None);
        let radio_dio1 = Input::new(peripherals.P0_20, Pull::None);
        let mut radio_reset = Output::new(peripherals.P0_25, Level::Low, OutputDrive::Standard);
        Timer::after_millis(2).await;
        radio_reset.set_high();
        let radio = Sx126x::new(
            radio_spi,
            radio_busy,
            radio_dio1,
            radio_reset,
            Delay,
            BoardConfig {
                tcxo_voltage: Some(TcxoVoltage::V1_8),
                use_dcdc: true,
                rx_boost: true,
                dio2_as_rf_switch: true,
                external_rx_gain_db: 0,
                external_power_amplifier: None,
                enter_transmit: None,
                enter_receive: None,
            },
        );

        let status_led = StatusLed::active_low(Output::new(
            peripherals.P1_03,
            Level::High,
            OutputDrive::Standard,
        ));

        (
            identity,
            T114Hardware {
                usb,
                radio,
                status_led,
            },
        )
    }
}
