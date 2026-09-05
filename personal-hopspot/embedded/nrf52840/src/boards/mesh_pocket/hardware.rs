use embassy_nrf::config::{HfclkSource, LfclkSource};
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
use embassy_time::{Delay, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use personal_rns::radios::sx126x::{BoardConfig, FrontendControl, Sx126x, TcxoVoltage};
use static_cell::StaticCell;

use crate::retained_display::BoardDisplay;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    SPI2 => spim::InterruptHandler<peripherals::SPI2>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
    SAADC => saadc::InterruptHandler;
});

type MeshPocketSpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

pub(crate) type MeshPocketRadio =
    Sx126x<MeshPocketSpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type MeshPocketEink =
    super::ssd1680::Ssd1680<MeshPocketSpiDevice, Input<'static>, Output<'static>, Output<'static>>;

pub(crate) struct MeshPocketUsbHardware {
    pub(crate) driver: Driver<'static, &'static SoftwareVbusDetect>,
    pub(crate) vbus: &'static SoftwareVbusDetect,
}

pub(crate) struct MeshPocketFaceHardware {
    pub(crate) battery: MeshPocketBattery,
    pub(crate) status_led: Output<'static>,
}

pub(crate) struct MeshPocketEarlyHardware {
    pub(crate) usb: MeshPocketUsbHardware,
    pub(crate) face: MeshPocketFaceHardware,
    pub(crate) deferred: MeshPocketDeferredHardware,
}

pub(crate) struct MeshPocketControls {
    pub(crate) button: Input<'static>,
}

pub(crate) struct MeshPocketDisplayHold;

pub(crate) struct MeshPocketDisplayHardware {
    pub(crate) device: BoardDisplay<super::display::MeshPocketDisplayDevice>,
    pub(crate) _rail: MeshPocketDisplayHold,
}

pub(crate) struct MeshPocketRuntimeHardware {
    pub(crate) radio: MeshPocketRadio,
    pub(crate) display: MeshPocketDisplayHardware,
    pub(crate) controls: MeshPocketControls,
}

pub(crate) struct MeshPocketBattery {
    adc: Saadc<'static, 1>,
    divider_enable: Output<'static>,
}

impl MeshPocketBattery {
    pub(crate) async fn sample_millivolts(&mut self) -> u32 {
        self.divider_enable.set_high();
        Timer::after_millis(10).await;
        let mut sample = [0i16; 1];
        self.adc.sample(&mut sample).await;
        self.divider_enable.set_low();
        battery_millivolts(sample[0])
    }
}

pub(crate) struct MeshPocketBoard;

impl MeshPocketBoard {
    pub(crate) fn initialize_identities<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, Rng<'static, Blocking>) -> R,
    ) -> (R, MeshPocketEarlyHardware) {
        let mut nrf_config = config::Config::default();
        nrf_config.hfclk_source = HfclkSource::ExternalXtal;
        nrf_config.lfclk_source = LfclkSource::ExternalXtal;
        nrf_config.gpiote_interrupt_priority = Priority::P2;
        nrf_config.time_interrupt_priority = Priority::P2;
        let peripherals = embassy_nrf::init(nrf_config);

        let identities = {
            let mut nvmc = Nvmc::new(peripherals.NVMC);
            let rng = Rng::new_blocking(peripherals.RNG);
            bootstrap(&mut nvmc, rng)
        };

        interrupt::USBD.set_priority(Priority::P2);
        interrupt::SPI2.set_priority(Priority::P3);
        interrupt::TWISPI0.set_priority(Priority::P3);
        interrupt::SAADC.set_priority(Priority::P3);

        static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
        let vbus = crate::runtime::software_vbus::initialize(&SOFTWARE_VBUS);
        let usb_driver = Driver::new(peripherals.USBD, Irqs, vbus);

        let mut battery_channel = ChannelConfig::single_ended(peripherals.P0_29);
        battery_channel.reference = Reference::INTERNAL;
        battery_channel.gain = Gain::GAIN1_5;
        let battery = MeshPocketBattery {
            adc: Saadc::new(
                peripherals.SAADC,
                Irqs,
                SaadcConfig::default(),
                [battery_channel],
            ),
            divider_enable: Output::new(peripherals.P1_02, Level::Low, OutputDrive::Standard),
        };

        let hardware = MeshPocketEarlyHardware {
            usb: MeshPocketUsbHardware {
                driver: usb_driver,
                vbus,
            },
            face: MeshPocketFaceHardware {
                battery,
                status_led: Output::new(peripherals.P0_13, Level::High, OutputDrive::Standard),
            },
            deferred: MeshPocketDeferredHardware {
                radio_bus: peripherals.TWISPI0,
                radio_sck: peripherals.P0_04,
                radio_miso: peripherals.P1_09,
                radio_mosi: peripherals.P0_05,
                radio_cs: peripherals.P0_26,
                radio_busy: peripherals.P0_15,
                radio_dio1: peripherals.P0_16,
                radio_reset: peripherals.P0_12,
                eink_bus: peripherals.SPI2,
                eink_sck: peripherals.P0_22,
                eink_mosi: peripherals.P0_20,
                eink_cs: peripherals.P0_24,
                eink_dc: peripherals.P0_31,
                eink_reset: peripherals.P1_04,
                eink_busy: peripherals.P1_06,
                button: peripherals.P1_10,
            },
        };
        (identities, hardware)
    }
}

pub(crate) struct MeshPocketDeferredHardware {
    radio_bus: Peri<'static, peripherals::TWISPI0>,
    radio_sck: Peri<'static, peripherals::P0_04>,
    radio_miso: Peri<'static, peripherals::P1_09>,
    radio_mosi: Peri<'static, peripherals::P0_05>,
    radio_cs: Peri<'static, peripherals::P0_26>,
    radio_busy: Peri<'static, peripherals::P0_15>,
    radio_dio1: Peri<'static, peripherals::P0_16>,
    radio_reset: Peri<'static, peripherals::P0_12>,
    eink_bus: Peri<'static, peripherals::SPI2>,
    eink_sck: Peri<'static, peripherals::P0_22>,
    eink_mosi: Peri<'static, peripherals::P0_20>,
    eink_cs: Peri<'static, peripherals::P0_24>,
    eink_dc: Peri<'static, peripherals::P0_31>,
    eink_reset: Peri<'static, peripherals::P1_04>,
    eink_busy: Peri<'static, peripherals::P1_06>,
    button: Peri<'static, peripherals::P1_10>,
}

impl MeshPocketDeferredHardware {
    pub(crate) async fn finish(self) -> MeshPocketRuntimeHardware {
        let mut radio_config = spim::Config::default();
        radio_config.frequency = spim::Frequency::M4;
        let radio_bus = Spim::new(
            self.radio_bus,
            Irqs,
            self.radio_sck,
            self.radio_miso,
            self.radio_mosi,
            radio_config,
        );
        let radio_spi = ExclusiveDevice::new(
            radio_bus,
            Output::new(self.radio_cs, Level::High, OutputDrive::Standard),
            Delay,
        )
        .unwrap();
        let radio = Sx126x::new(
            radio_spi,
            Input::new(self.radio_busy, Pull::None),
            Input::new(self.radio_dio1, Pull::None),
            Output::new(self.radio_reset, Level::High, OutputDrive::Standard),
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

        let mut eink_config = spim::Config::default();
        eink_config.frequency = spim::Frequency::M4;
        let eink_bus = Spim::new_txonly(
            self.eink_bus,
            Irqs,
            self.eink_sck,
            self.eink_mosi,
            eink_config,
        );
        let eink_spi = ExclusiveDevice::new(
            eink_bus,
            Output::new(self.eink_cs, Level::High, OutputDrive::Standard),
            Delay,
        )
        .unwrap();
        let mut eink = super::ssd1680::Ssd1680::new(
            eink_spi,
            Input::new(self.eink_busy, Pull::None),
            Output::new(self.eink_dc, Level::Low, OutputDrive::Standard),
            Output::new(self.eink_reset, Level::High, OutputDrive::Standard),
        );
        let initialized = eink.initialize().await.is_ok();
        let device = super::display::MeshPocketDisplayDevice::new(eink);
        let device = if initialized {
            BoardDisplay::initialized(device)
        } else {
            BoardDisplay::initialization_failed(device)
        };

        MeshPocketRuntimeHardware {
            radio,
            display: MeshPocketDisplayHardware {
                device,
                _rail: MeshPocketDisplayHold,
            },
            controls: MeshPocketControls {
                button: Input::new(self.button, Pull::Up),
            },
        }
    }
}

const fn battery_millivolts(raw: i16) -> u32 {
    let raw = if raw < 0 { 0 } else { raw as u32 };
    raw * 55_710 / 16_384
}

const _: () = {
    assert!(battery_millivolts(0) == 0);
    assert!(battery_millivolts(1_238) >= 4_205);
    assert!(battery_millivolts(1_238) <= 4_210);
};
