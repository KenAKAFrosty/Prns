use core::cell::RefCell;

use embassy_nrf::config::{HfclkSource, LfclkSource};
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::mode::Blocking;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::rng::Rng;
use embassy_nrf::saadc::{self, ChannelConfig, Config as SaadcConfig, Gain, Reference, Saadc};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::twim::{self, Twim};
use embassy_nrf::uarte::{self, Uarte};
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Delay, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use personal_rns::lora::LoRaInterface;
#[cfg(feature = "wio-tracker-l1-pro-1w")]
use personal_rns::radios::sx126x::ExternalPowerAmplifier;
use personal_rns::radios::sx126x::{BoardConfig, FrontendControl, Sx126x, TcxoVoltage};
use static_cell::{ConstStaticCell, StaticCell};

use crate::boards::status_led::StatusLed;
use crate::immediate_display::BoardDisplay;

use super::input::WioInputs;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
    TWISPI1 => twim::InterruptHandler<peripherals::TWISPI1>;
    SAADC => saadc::InterruptHandler;
    UARTE1 => uarte::InterruptHandler<peripherals::UARTE1>;
});

type WioSpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

type WioRadio = Sx126x<WioSpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type WioLoraInterface = LoRaInterface<'static, 'static, WioRadio>;

pub(crate) type WioDisplayBringup = BoardDisplay<super::DisplayDriver>;

type WioUsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

pub(crate) struct WioHardware {
    pub(crate) usb: WioUsbDriver,
    pub(crate) vbus: &'static SoftwareVbusDetect,
    pub(crate) radio: WioRadio,
    pub(crate) display: WioDisplayBringup,
    pub(crate) battery: WioBattery,
    pub(crate) button: WioInputs,
    pub(crate) status_led: StatusLed,
    pub(crate) gnss: super::Gnss,
}

pub(crate) struct WioBattery {
    adc: Saadc<'static, 1>,
    divider_enable: Output<'static>,
}

impl WioBattery {
    pub(crate) async fn sample_millivolts(&mut self) -> u32 {
        // BAT_CTL gates the 1:2 VBAT divider; power it only for the settling and conversion window.
        self.divider_enable.set_high();
        Timer::after_millis(2).await;
        let mut sample = [0i16; 1];
        self.adc.sample(&mut sample).await;
        self.divider_enable.set_low();
        battery_millivolts(sample[0])
    }
}

struct HeldIo {
    _buzzer: Output<'static>,
    _qspi_cs: Output<'static>,
    #[cfg(not(feature = "wio-tracker-l1-pro-1w"))]
    radio_rx_enable: Output<'static>,
    #[cfg(feature = "wio-tracker-l1-pro-1w")]
    _radio_power: Output<'static>,
    #[cfg(feature = "wio-tracker-l1-pro-1w")]
    _grove_boost: Output<'static>,
}

static HELD_IO: Mutex<CriticalSectionRawMutex, RefCell<Option<HeldIo>>> =
    Mutex::new(RefCell::new(None));

pub(crate) struct WioBoard;

impl WioBoard {
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn initialize<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, Rng<'static, Blocking>) -> R,
    ) -> (R, WioHardware) {
        let mut nrf_config = config::Config::default();
        nrf_config.hfclk_source = HfclkSource::ExternalXtal;
        nrf_config.lfclk_source = LfclkSource::ExternalXtal;
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
        interrupt::TWISPI1.set_priority(Priority::P3);
        interrupt::SAADC.set_priority(Priority::P3);
        interrupt::UARTE1.set_priority(Priority::P3);
        static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
        let vbus = crate::runtime::software_vbus::initialize(&SOFTWARE_VBUS);
        let usb = Driver::new(peripherals.USBD, Irqs, vbus);

        // A floating buzzer gate can whine, and the unused P25Q16H QSPI flash must stay deselected.
        let buzzer = Output::new(peripherals.P1_00, Level::Low, OutputDrive::Standard);
        let qspi_cs = Output::new(peripherals.P0_25, Level::High, OutputDrive::Standard);
        // The RF switch's receive path is selected by RXEN (P1.08); SX1262 DIO2 drives transmit.
        #[cfg(not(feature = "wio-tracker-l1-pro-1w"))]
        let radio_rx_enable = Output::new(peripherals.P1_08, Level::Low, OutputDrive::Standard);
        // Pro 1W: an LDO enabled by P0.14 feeds the SX1262 and its 1 W PA, and DIO2 alone switches
        // the antenna path. The Grove 5 V boost (P0.13) stays off as in Seeed's shipping state.
        #[cfg(feature = "wio-tracker-l1-pro-1w")]
        let radio_power = Output::new(peripherals.P0_14, Level::High, OutputDrive::Standard);
        #[cfg(feature = "wio-tracker-l1-pro-1w")]
        let grove_boost = Output::new(peripherals.P0_13, Level::Low, OutputDrive::Standard);
        HELD_IO.lock(|held| {
            *held.borrow_mut() = Some(HeldIo {
                _buzzer: buzzer,
                _qspi_cs: qspi_cs,
                #[cfg(not(feature = "wio-tracker-l1-pro-1w"))]
                radio_rx_enable,
                #[cfg(feature = "wio-tracker-l1-pro-1w")]
                _radio_power: radio_power,
                #[cfg(feature = "wio-tracker-l1-pro-1w")]
                _grove_boost: grove_boost,
            });
        });

        // Quectel L76K at 9600 baud: MCU TX P0.27, MCU RX P0.26, standby/wake P1.09.
        let gnss = {
            let mut uart_config = uarte::Config::default();
            uart_config.baudrate = uarte::Baudrate::BAUD9600;
            let uart = Uarte::new(
                peripherals.UARTE1,
                peripherals.P0_26,
                peripherals.P0_27,
                Irqs,
                uart_config,
            );
            super::Gnss::new(
                uart,
                Output::new(peripherals.P1_09, Level::Low, OutputDrive::Standard),
            )
        };

        let mut twim_config = twim::Config::default();
        twim_config.frequency = twim::Frequency::K400;
        static TWIM_TX_BUFFER: ConstStaticCell<[u8; 4]> = ConstStaticCell::new([0; 4]);
        let display_bus = Twim::new(
            peripherals.TWISPI1,
            Irqs,
            peripherals.P0_06,
            peripherals.P0_05,
            twim_config,
            TWIM_TX_BUFFER.take(),
        );
        static DISPLAYED_PAGES: ConstStaticCell<super::display::PageCache> =
            ConstStaticCell::new(super::display::PageCache::new());
        let mut display = super::DisplayDriver::new(display_bus, DISPLAYED_PAGES.take());
        let display = match display.initialize().await {
            Ok(()) => BoardDisplay::initialized(display),
            Err(_) => {
                display.force_dark();
                BoardDisplay::initialization_failed(display)
            }
        };

        // VBAT reaches AIN7 (P0.31) through a 1:2 divider gated by BAT_CTL (P0.04). Gain 1/6
        // against the 0.6 V internal reference is the 3.6 V range Seeed's firmware uses.
        let mut battery_channel = ChannelConfig::single_ended(peripherals.P0_31);
        battery_channel.reference = Reference::INTERNAL;
        battery_channel.gain = Gain::GAIN1_6;
        let battery_adc = Saadc::new(
            peripherals.SAADC,
            Irqs,
            SaadcConfig::default(),
            [battery_channel],
        );
        let battery = WioBattery {
            adc: battery_adc,
            divider_enable: Output::new(peripherals.P0_04, Level::Low, OutputDrive::Standard),
        };

        let button = WioInputs {
            button: Input::new(peripherals.P0_08, Pull::Up),
            press: Input::new(peripherals.P1_05, Pull::Up),
            up: Input::new(peripherals.P1_04, Pull::Up),
            down: Input::new(peripherals.P0_12, Pull::Up),
            left: Input::new(peripherals.P0_11, Pull::Up),
            right: Input::new(peripherals.P1_03, Pull::Up),
        };

        let mut radio_spim_config = spim::Config::default();
        radio_spim_config.frequency = spim::Frequency::M8;
        let radio_bus = Spim::new(
            peripherals.TWISPI0,
            Irqs,
            peripherals.P0_30,
            peripherals.P0_03,
            peripherals.P0_28,
            radio_spim_config,
        );
        let radio_cs = Output::new(peripherals.P1_14, Level::High, OutputDrive::Standard);
        let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
        let radio_busy = Input::new(peripherals.P1_10, Pull::None);
        let radio_dio1 = Input::new(peripherals.P0_07, Pull::None);
        let mut radio_reset = Output::new(peripherals.P1_07, Level::Low, OutputDrive::Standard);
        // The radio LDO switched on above has had the display bring-up to settle; on the stock L1
        // the SX1262 sits on the always-on 3V3 rail.
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
                #[cfg(not(feature = "wio-tracker-l1-pro-1w"))]
                external_power_amplifier: None,
                #[cfg(feature = "wio-tracker-l1-pro-1w")]
                external_power_amplifier: Some(ExternalPowerAmplifier {
                    minimum_output_power_dbm: 1,
                    maximum_output_power_dbm: 30,
                    chip_power_dbm: pro_1w_chip_power_dbm,
                }),
                #[cfg(not(feature = "wio-tracker-l1-pro-1w"))]
                frontend_control: FrontendControl::TxRx {
                    enter_transmit,
                    enter_receive,
                },
                #[cfg(feature = "wio-tracker-l1-pro-1w")]
                frontend_control: FrontendControl::NoDynamicControl,
            },
        );

        // The single user LED on P1.01 is active-high.
        let status_led = StatusLed::active_high(Output::new(
            peripherals.P1_01,
            Level::Low,
            OutputDrive::Standard,
        ));

        (
            identity,
            WioHardware {
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

/// Convert the 12-bit, 3.6 V SAADC result through the board's 2x VBAT divider.
const fn battery_millivolts(raw: i16) -> u32 {
    let raw = if raw < 0 { 0 } else { raw as u32 };
    raw * 7_200 / 4_096
}

const _: () = {
    assert!(battery_millivolts(0) == 0);
    assert!(battery_millivolts(2_389) >= 4_195);
    assert!(battery_millivolts(2_389) <= 4_205);
};

/// Convert an antenna-referred request through the Pro 1W PA gain curve Meshtastic ships for this
/// board (indexed by SX1262 output power). The driver separately clamps to the SX1262 range.
#[cfg(feature = "wio-tracker-l1-pro-1w")]
const fn pro_1w_chip_power_dbm(requested_output_dbm: i8) -> i8 {
    const GAIN_DB_BY_CHIP_POWER: [i8; 22] = [
        10, 10, 10, 10, 10, 10, 10, 10, 10, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 10, 10,
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

#[cfg(feature = "wio-tracker-l1-pro-1w")]
const _: () = {
    assert!(pro_1w_chip_power_dbm(1) == -9);
    assert!(pro_1w_chip_power_dbm(14) == 4);
    assert!(pro_1w_chip_power_dbm(30) == 20);
};

#[cfg(not(feature = "wio-tracker-l1-pro-1w"))]
fn enter_transmit() {
    HELD_IO.lock(|held| {
        if let Some(io) = held.borrow_mut().as_mut() {
            io.radio_rx_enable.set_low();
        }
    });
}

#[cfg(not(feature = "wio-tracker-l1-pro-1w"))]
fn enter_receive() {
    HELD_IO.lock(|held| {
        if let Some(io) = held.borrow_mut().as_mut() {
            io.radio_rx_enable.set_high();
        }
    });
}
