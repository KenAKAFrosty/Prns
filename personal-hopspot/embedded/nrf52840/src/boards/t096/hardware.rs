use core::cell::RefCell;

use embassy_nrf::config::{HfclkSource, LfclkSource};
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::mode::Blocking;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::rng::Rng;
use embassy_nrf::saadc::{self, ChannelConfig, Config as SaadcConfig, Gain, Reference, Saadc};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::uarte::{self, Uarte};
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Delay, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use personal_rns::lora::LoRaInterface;
use personal_rns::radios::sx126x::{
    BoardConfig, ExternalPowerAmplifier, FrontendControl, Sx126x, TcxoVoltage,
};
use static_cell::StaticCell;

use crate::boards::status_led::StatusLed;

use crate::boards::{DisplayBringup, DisplayIoError};

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
    TWISPI1 => spim::InterruptHandler<peripherals::TWISPI1>;
    SAADC => saadc::InterruptHandler;
    UARTE1 => uarte::InterruptHandler<peripherals::UARTE1>;
});

type T096SpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

type T096Radio = Sx126x<T096SpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type T096LoraInterface = LoRaInterface<'static, T096Radio>;

pub(crate) type T096Display = super::DisplayDriver<T096SpiDevice>;
pub(crate) type T096DisplayBringup = DisplayBringup<T096Display, DisplayIoError>;

type T096UsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

pub(crate) struct T096Hardware {
    pub(crate) usb: T096UsbDriver,
    pub(crate) vbus: &'static SoftwareVbusDetect,
    pub(crate) radio: T096Radio,
    pub(crate) display: T096DisplayBringup,
    pub(crate) battery: T096Battery,
    pub(crate) button: Input<'static>,
    pub(crate) status_led: StatusLed,
    pub(crate) gnss: super::Gnss,
}

pub(crate) struct T096Battery {
    adc: Saadc<'static, 1>,
    divider_enable: Output<'static>,
}

impl T096Battery {
    pub(crate) async fn sample_millivolts(&mut self) -> u32 {
        // The 390k/100k divider is gated; power it only for the settling and conversion window.
        self.divider_enable.set_high();
        Timer::after_millis(2).await;
        let mut sample = [0i16; 1];
        self.adc.sample(&mut sample).await;
        self.divider_enable.set_low();
        battery_millivolts(sample[0])
    }
}

struct HeldIo {
    _peripheral_power: Output<'static>,
    fem_power: Output<'static>,
    fem_enable: Output<'static>,
    fem_ctx: Output<'static>,
}

static HELD_IO: Mutex<CriticalSectionRawMutex, RefCell<Option<HeldIo>>> =
    Mutex::new(RefCell::new(None));

pub(crate) struct T096Board;

impl T096Board {
    pub(crate) async fn initialize<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, &mut Rng<'static, Blocking>) -> R,
    ) -> (R, T096Hardware) {
        let mut nrf_config = config::Config::default();
        nrf_config.hfclk_source = HfclkSource::ExternalXtal;
        nrf_config.lfclk_source = LfclkSource::ExternalXtal;
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
        interrupt::UARTE1.set_priority(Priority::P3);
        static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
        let vbus = crate::runtime::software_vbus::initialize(&SOFTWARE_VBUS);
        let usb = Driver::new(peripherals.USBD, Irqs, vbus);

        // VEXT feeds the radio, display, GNSS, and external headers. Keep GNSS disabled and held in
        // reset until a consumer explicitly claims it; the display is brought up below with its
        // active-low backlight held off.
        let peripheral_power = Output::new(peripherals.P0_26, Level::High, OutputDrive::Standard);
        let gnss_enable = Output::new(peripherals.P0_06, Level::High, OutputDrive::Standard);
        let gnss_reset = Output::new(peripherals.P1_14, Level::Low, OutputDrive::Standard);

        // KCT8103L: power and CSD high enable the FEM. CTX low selects its 21 dB receive-LNA
        // path; CTX high selects transmit while SX1262 DIO2 drives the CPS path switch.
        let mut fem_power = Output::new(peripherals.P0_30, Level::High, OutputDrive::Standard);
        fem_power.set_high();
        let mut fem_enable = Output::new(peripherals.P0_12, Level::High, OutputDrive::Standard);
        fem_enable.set_high();
        let mut fem_ctx = Output::new(peripherals.P1_09, Level::Low, OutputDrive::Standard);
        fem_ctx.set_low();

        HELD_IO.lock(|held| {
            *held.borrow_mut() = Some(HeldIo {
                _peripheral_power: peripheral_power,
                fem_power,
                fem_enable,
                fem_ctx,
            });
        });
        Timer::after_millis(10).await;

        let gnss = {
            let uart = Uarte::new(
                peripherals.UARTE1,
                peripherals.P0_25,
                peripherals.P0_23,
                Irqs,
                uarte::Config::default(),
            );
            super::Gnss::new(
                uart,
                gnss_enable,
                gnss_reset,
                Input::new(peripherals.P1_11, Pull::None),
            )
        };

        let mut display_spim_config = spim::Config::default();
        display_spim_config.frequency = spim::Frequency::M8;
        let display_bus = Spim::new_txonly(
            peripherals.TWISPI1,
            Irqs,
            peripherals.P0_20,
            peripherals.P0_17,
            display_spim_config,
        );
        let display_cs = Output::new(peripherals.P0_22, Level::High, OutputDrive::Standard);
        let display_spi = ExclusiveDevice::new(display_bus, display_cs, Delay).unwrap();
        let display_dc = Output::new(peripherals.P0_15, Level::Low, OutputDrive::Standard);
        let display_reset = Output::new(peripherals.P0_13, Level::High, OutputDrive::Standard);
        let display_backlight = Output::new(peripherals.P1_12, Level::High, OutputDrive::Standard);
        let mut display =
            super::DisplayDriver::new(display_spi, display_dc, display_reset, display_backlight);
        let display = match display.initialize().await {
            Ok(()) => DisplayBringup::Ready(display),
            Err(error) => {
                display.force_dark();
                DisplayBringup::Unavailable(error)
            }
        };

        // AIN1 sees VBAT through Heltec's gated 390k/100k divider. Gain 1/5 against the 0.6 V
        // internal reference yields the 3.0 V ADC range used by Heltec's reference firmware.
        let mut battery_channel = ChannelConfig::single_ended(peripherals.P0_03);
        battery_channel.reference = Reference::INTERNAL;
        battery_channel.gain = Gain::GAIN1_5;
        let battery_adc = Saadc::new(
            peripherals.SAADC,
            Irqs,
            SaadcConfig::default(),
            [battery_channel],
        );
        let battery = T096Battery {
            adc: battery_adc,
            divider_enable: Output::new(peripherals.P1_15, Level::Low, OutputDrive::Standard),
        };
        let button = Input::new(peripherals.P1_10, Pull::Up);

        let mut radio_spim_config = spim::Config::default();
        radio_spim_config.frequency = spim::Frequency::M4;
        let radio_bus = Spim::new(
            peripherals.TWISPI0,
            Irqs,
            peripherals.P1_08,
            peripherals.P0_14,
            peripherals.P0_11,
            radio_spim_config,
        );
        let radio_cs = Output::new(peripherals.P0_05, Level::High, OutputDrive::Standard);
        let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
        let radio_busy = Input::new(peripherals.P0_19, Pull::None);
        let radio_dio1 = Input::new(peripherals.P0_21, Pull::None);
        let mut radio_reset = Output::new(peripherals.P0_16, Level::Low, OutputDrive::Standard);
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
                external_rx_gain_db: 21,
                external_power_amplifier: Some(ExternalPowerAmplifier {
                    minimum_output_power_dbm: 5,
                    maximum_output_power_dbm: 22,
                    chip_power_dbm: t096_chip_power_dbm,
                }),
                frontend_control: FrontendControl::TxRx {
                    enter_transmit,
                    enter_receive,
                },
            },
        );

        let status_led = StatusLed::active_high(Output::new(
            peripherals.P0_28,
            Level::Low,
            OutputDrive::Standard,
        ));

        (
            identity,
            T096Hardware {
                usb,
                vbus,
                radio,
                display,
                battery,
                button,
                status_led,
                gnss,
            },
        )
    }
}

/// Convert the 12-bit, 3.0 V SAADC result through Heltec's measured 4.916x divider multiplier.
const fn battery_millivolts(raw: i16) -> u32 {
    let raw = if raw < 0 { 0 } else { raw as u32 };
    raw * 14_748 / 4_096
}

const _: () = {
    assert!(battery_millivolts(0) == 0);
    assert!(battery_millivolts(1_166) >= 4_195);
    assert!(battery_millivolts(1_166) <= 4_205);
};

/// Convert an antenna-referred request through the measured KCT8103L gain curve used by the
/// current T096 reference firmware. The driver separately clamps the result to the SX1262 range.
const fn t096_chip_power_dbm(requested_output_dbm: i8) -> i8 {
    const GAIN_DB_BY_CHIP_POWER: [i8; 22] = [
        14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 13, 13, 13, 12, 11, 10, 9, 8, 7,
    ];

    let mut chip_power_dbm = 0;
    while chip_power_dbm < GAIN_DB_BY_CHIP_POWER.len() {
        let gain_db = GAIN_DB_BY_CHIP_POWER[chip_power_dbm];
        let is_last = chip_power_dbm == GAIN_DB_BY_CHIP_POWER.len() - 1;
        if chip_power_dbm as i8 + gain_db > requested_output_dbm || is_last {
            return requested_output_dbm - gain_db;
        }
        chip_power_dbm += 1;
    }

    requested_output_dbm
}

const _: () = {
    assert!(t096_chip_power_dbm(5) == -9);
    assert!(t096_chip_power_dbm(22) == 8);
};

fn enter_transmit() {
    HELD_IO.lock(|held| {
        if let Some(io) = held.borrow_mut().as_mut() {
            io.fem_power.set_high();
            io.fem_enable.set_high();
            io.fem_ctx.set_high();
        }
    });
}

fn enter_receive() {
    HELD_IO.lock(|held| {
        if let Some(io) = held.borrow_mut().as_mut() {
            io.fem_power.set_high();
            io.fem_enable.set_high();
            io.fem_ctx.set_low();
        }
    });
}
