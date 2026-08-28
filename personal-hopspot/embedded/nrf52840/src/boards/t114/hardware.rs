use embassy_nrf::config::HfclkSource;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::mode::Blocking;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::rng::Rng;
use embassy_nrf::saadc::{self, ChannelConfig, Config as SaadcConfig, Gain, Reference, Saadc};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_time::{Delay, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use personal_rns::lora::LoRaInterface;
use personal_rns::radios::sx126x::{BoardConfig, FrontendControl, Sx126x, TcxoVoltage};
use static_cell::StaticCell;

use crate::boards::status_led::StatusLed;

use crate::boards::DisplayIoError;
use crate::immediate_display::BoardDisplay;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
    TWISPI1 => spim::InterruptHandler<peripherals::TWISPI1>;
    SAADC => saadc::InterruptHandler;
});

type T114SpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

type T114Radio = Sx126x<T114SpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type T114LoraInterface = LoRaInterface<'static, T114Radio>;

pub(crate) type T114Display = super::DisplayDriver<T114SpiDevice>;
pub(crate) type T114DisplayBringup = BoardDisplay<T114Display>;

type T114UsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

pub(crate) struct T114Hardware {
    pub(crate) usb: T114UsbDriver,
    pub(crate) vbus: &'static SoftwareVbusDetect,
    pub(crate) radio: T114Radio,
    pub(crate) display: T114DisplayBringup,
    pub(crate) battery: T114Battery,
    pub(crate) button: Input<'static>,
    pub(crate) status_led: StatusLed,
}

pub(crate) struct T114Battery {
    adc: Saadc<'static, 1>,
    divider_enable: Output<'static>,
}

impl T114Battery {
    pub(crate) async fn sample_millivolts(&mut self) -> u32 {
        self.divider_enable.set_high();
        Timer::after_millis(2).await;
        let mut sample = [0i16; 1];
        self.adc.sample(&mut sample).await;
        self.divider_enable.set_low();
        battery_millivolts(sample[0])
    }
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
        interrupt::TWISPI0.set_priority(Priority::P3);
        interrupt::TWISPI1.set_priority(Priority::P3);
        interrupt::SAADC.set_priority(Priority::P3);
        static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
        let vbus = crate::runtime::software_vbus::initialize(&SOFTWARE_VBUS);
        let usb = Driver::new(peripherals.USBD, Irqs, vbus);

        let mut display_spim_config = spim::Config::default();
        display_spim_config.frequency = spim::Frequency::M8;
        let display_bus = Spim::new_txonly(
            peripherals.TWISPI1,
            Irqs,
            peripherals.P1_08,
            peripherals.P1_09,
            display_spim_config,
        );
        let display_cs = Output::new(peripherals.P0_11, Level::High, OutputDrive::Standard);
        let display_spi = ExclusiveDevice::new(display_bus, display_cs, Delay).unwrap();
        let display_dc = Output::new(peripherals.P0_12, Level::Low, OutputDrive::Standard);
        let display_reset = Output::new(peripherals.P0_02, Level::High, OutputDrive::Standard);
        let display_power = Output::new(peripherals.P0_03, Level::Low, OutputDrive::Standard);
        let display_backlight = Output::new(peripherals.P0_15, Level::High, OutputDrive::Standard);
        let mut display = super::DisplayDriver::new(
            display_spi,
            display_dc,
            display_reset,
            display_power,
            display_backlight,
        );
        let display = match display.initialize().await {
            Ok(()) => BoardDisplay::initialized(display),
            Err(DisplayIoError::Spi | DisplayIoError::NotInitialized) => {
                display.force_dark();
                BoardDisplay::initialization_failed(display)
            }
        };

        let mut battery_channel = ChannelConfig::single_ended(peripherals.P0_04);
        battery_channel.reference = Reference::INTERNAL;
        battery_channel.gain = Gain::GAIN1_5;
        let battery_adc = Saadc::new(
            peripherals.SAADC,
            Irqs,
            SaadcConfig::default(),
            [battery_channel],
        );
        let battery = T114Battery {
            adc: battery_adc,
            divider_enable: Output::new(peripherals.P0_06, Level::Low, OutputDrive::Standard),
        };
        let button = Input::new(peripherals.P1_10, Pull::Up);

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
                frontend_control: FrontendControl::NoDynamicControl,
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
                vbus,
                radio,
                display,
                battery,
                button,
                status_led,
            },
        )
    }
}

const fn battery_millivolts(raw: i16) -> u32 {
    let raw = if raw < 0 { 0 } else { raw as u32 };
    raw * 14_748 / 4_096
}

const _: () = {
    assert!(battery_millivolts(0) == 0);
    assert!(battery_millivolts(1_166) >= 4_195);
    assert!(battery_millivolts(1_166) <= 4_205);
};
