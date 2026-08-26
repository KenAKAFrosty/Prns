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
use embassy_time::{Delay, Duration, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use epd_waveshare::epd1in54_v2::Display1in54;
use personal_rns::radios::sx126x::{BoardConfig, FrontendControl, Sx126x, TcxoVoltage};
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    SPI2 => spim::InterruptHandler<peripherals::SPI2>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
    SAADC => saadc::InterruptHandler;
});

type TechoSpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

pub(crate) type TechoRadio =
    Sx126x<TechoSpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type TechoEink = super::ssd1681::Ssd1681<
    TechoSpiDevice,
    Input<'static>,
    Output<'static>,
    Output<'static>,
    Delay,
>;

pub(crate) struct TechoUsbHardware {
    pub(crate) driver: Driver<'static, &'static SoftwareVbusDetect>,
    pub(crate) vbus: &'static SoftwareVbusDetect,
}

pub(crate) struct TechoFaceHardware {
    pub(crate) battery: Saadc<'static, 1>,
    pub(crate) status_led: Output<'static>,
}

pub(crate) struct TechoEarlyHardware {
    pub(crate) usb: TechoUsbHardware,
    pub(crate) face: TechoFaceHardware,
    pub(crate) deferred: TechoDeferredHardware,
}

pub(crate) struct TechoControls {
    pub(crate) button: Input<'static>,
    pub(crate) frontlight: Output<'static>,
}

pub(crate) struct TechoDisplayHardware {
    pub(crate) driver: Option<TechoEink>,
    pub(crate) panel: Display1in54,
    pub(crate) _rail: Output<'static>,
}

pub(crate) struct TechoRuntimeHardware {
    pub(crate) radio: TechoRadio,
    pub(crate) display: TechoDisplayHardware,
    pub(crate) controls: TechoControls,
}

pub(crate) struct TechoDeferredHardware {
    radio_bus: Peri<'static, peripherals::TWISPI0>,
    radio_sck: Peri<'static, peripherals::P0_19>,
    radio_mosi: Peri<'static, peripherals::P0_23>,
    radio_miso: Peri<'static, peripherals::P0_22>,
    radio_cs: Peri<'static, peripherals::P0_24>,
    radio_busy: Peri<'static, peripherals::P0_17>,
    radio_dio1: Peri<'static, peripherals::P0_20>,
    radio_reset: Peri<'static, peripherals::P0_25>,
    eink_bus: Peri<'static, peripherals::SPI2>,
    eink_sck: Peri<'static, peripherals::P0_31>,
    eink_mosi: Peri<'static, peripherals::P1_06>,
    eink_miso: Peri<'static, peripherals::P0_29>,
    eink_cs: Peri<'static, peripherals::P0_30>,
    eink_dc: Peri<'static, peripherals::P0_28>,
    eink_reset: Peri<'static, peripherals::P0_02>,
    eink_busy: Peri<'static, peripherals::P0_03>,
    eink_rail: Output<'static>,
    button: Peri<'static, peripherals::P1_10>,
    frontlight: Peri<'static, peripherals::P1_11>,
}

pub(crate) struct TechoBoard;

impl TechoBoard {
    pub(crate) fn initialize_identities<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, &mut Rng<'static, Blocking>) -> R,
    ) -> (R, TechoEarlyHardware) {
        let mut nrf_config = config::Config::default();
        nrf_config.gpiote_interrupt_priority = Priority::P2;
        nrf_config.time_interrupt_priority = Priority::P2;
        let peripherals = embassy_nrf::init(nrf_config);

        let identities = {
            let mut nvmc = Nvmc::new(peripherals.NVMC);
            let mut rng = Rng::new_blocking(peripherals.RNG);
            bootstrap(&mut nvmc, &mut rng)
        };

        let eink_rail = Output::new(peripherals.P0_12, Level::High, OutputDrive::Standard);
        let status_led = Output::new(peripherals.P1_01, Level::High, OutputDrive::Standard);

        // The SoftDevice reserves P0/P1/P4; keep every app interrupt off those. USB at P2 (matches the
        // validated bring-up); SPI and SAADC at P3 so a BLE radio event can preempt them.
        interrupt::USBD.set_priority(Priority::P2);
        interrupt::SPI2.set_priority(Priority::P3);
        interrupt::TWISPI0.set_priority(Priority::P3);
        interrupt::SAADC.set_priority(Priority::P3);

        static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
        let vbus = crate::runtime::software_vbus::initialize(&SOFTWARE_VBUS);
        let usb_driver = Driver::new(peripherals.USBD, Irqs, vbus);

        // Battery sense: VBAT on a 2:1 divider into AIN2 (P0.04), sampled by the SAADC against the 3.0 V
        // internal reference, so VBAT_mV = raw * 6000 / 4096.
        let mut battery_channel = ChannelConfig::single_ended(peripherals.P0_04);
        battery_channel.reference = Reference::INTERNAL;
        battery_channel.gain = Gain::GAIN1_5;
        let battery = Saadc::new(
            peripherals.SAADC,
            Irqs,
            SaadcConfig::default(),
            [battery_channel],
        );

        let hardware = TechoEarlyHardware {
            usb: TechoUsbHardware {
                driver: usb_driver,
                vbus,
            },
            face: TechoFaceHardware {
                battery,
                status_led,
            },
            deferred: TechoDeferredHardware {
                radio_bus: peripherals.TWISPI0,
                radio_sck: peripherals.P0_19,
                radio_mosi: peripherals.P0_23,
                radio_miso: peripherals.P0_22,
                radio_cs: peripherals.P0_24,
                radio_busy: peripherals.P0_17,
                radio_dio1: peripherals.P0_20,
                radio_reset: peripherals.P0_25,
                eink_bus: peripherals.SPI2,
                eink_sck: peripherals.P0_31,
                eink_mosi: peripherals.P1_06,
                eink_miso: peripherals.P0_29,
                eink_cs: peripherals.P0_30,
                eink_dc: peripherals.P0_28,
                eink_reset: peripherals.P0_02,
                eink_busy: peripherals.P0_03,
                eink_rail,
                button: peripherals.P1_10,
                frontlight: peripherals.P1_11,
            },
        };
        (identities, hardware)
    }
}

impl TechoDeferredHardware {
    pub(crate) async fn finish(self) -> TechoRuntimeHardware {
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
        let radio = Sx126x::new(
            radio_spi,
            radio_busy,
            radio_dio1,
            radio_reset,
            Delay,
            BoardConfig {
                // LilyGo's factory firmware initializes this HPD16A through RadioLib's 1.6 V
                // TCXO default.
                tcxo_voltage: Some(TcxoVoltage::V1_6),
                use_dcdc: true,
                rx_boost: true,
                dio2_as_rf_switch: true,
                external_rx_gain_db: 0,
                external_power_amplifier: None,
                frontend_control: FrontendControl::NoDynamicControl,
            },
        );

        let mut eink_spim_config = spim::Config::default();
        eink_spim_config.frequency = spim::Frequency::M4;
        let eink_bus = Spim::new(
            self.eink_bus,
            Irqs,
            self.eink_sck,
            self.eink_mosi,
            self.eink_miso,
            eink_spim_config,
        );
        let eink_cs = Output::new(self.eink_cs, Level::High, OutputDrive::Standard);
        let eink_dc = Output::new(self.eink_dc, Level::Low, OutputDrive::Standard);
        let eink_reset = Output::new(self.eink_reset, Level::High, OutputDrive::Standard);
        let eink_busy = Input::new(self.eink_busy, Pull::None);
        Timer::after(Duration::from_millis(150)).await;
        let eink_spi = ExclusiveDevice::new(eink_bus, eink_cs, Delay).unwrap();
        let panel = Display1in54::default();
        let eink =
            super::ssd1681::Ssd1681::new(eink_spi, eink_busy, eink_dc, eink_reset, Delay).ok();

        TechoRuntimeHardware {
            radio,
            display: TechoDisplayHardware {
                driver: eink,
                panel,
                _rail: self.eink_rail,
            },
            controls: TechoControls {
                button: Input::new(self.button, Pull::Up),
                frontlight: Output::new(self.frontlight, Level::Low, OutputDrive::Standard),
            },
        }
    }
}
