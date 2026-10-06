use core::cell::RefCell;

use embassy_nrf::config::HfclkSource;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_nrf::mode::Blocking;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::rng::Rng;
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::uarte::{self, Baudrate, Uarte};
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Delay, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use personal_rns::lora::LoRaInterface;
use personal_rns::radios::sx126x::{BoardConfig, FrontendControl, Sx126x, TcxoVoltage};

use crate::boards::status_led::StatusLed;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
    UARTE0 => uarte::InterruptHandler<peripherals::UARTE0>;
});

type SolarNodeSpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

type SolarNodeRadio =
    Sx126x<SolarNodeSpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type SolarNodeLoraInterface = LoRaInterface<'static, SolarNodeRadio>;

type SolarNodeUsbDriver = Driver<'static, HardwareVbusDetect>;

pub(crate) struct SolarNodeHardware {
    pub(crate) flash: Nvmc<'static>,
    pub(crate) usb: SolarNodeUsbDriver,
    pub(crate) radio: SolarNodeRadio,
    pub(crate) status_led: StatusLed,
    pub(crate) gnss: super::Gnss,
}

/// The Wio-SX1262's receive path is gated by a GPIO while DIO2 drives the transmit path.
/// `FrontendControl` hands the driver bare `fn()` pointers with no captured state, so the pin is
/// parked here for those callbacks to reach — the same arrangement the MeshTower V2 uses.
struct HeldIo {
    radio_rx_enable: Output<'static>,
}

static HELD_IO: Mutex<CriticalSectionRawMutex, RefCell<Option<HeldIo>>> =
    Mutex::new(RefCell::new(None));

pub(crate) struct SolarNodeBoard;

impl SolarNodeBoard {
    pub(crate) async fn initialize<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, Rng<'static, Blocking>) -> R,
    ) -> (R, SolarNodeHardware) {
        let mut nrf_config = config::Config::default();
        nrf_config.hfclk_source = HfclkSource::ExternalXtal;
        // The carrier populates a 32.768 kHz crystal (both reference firmwares declare USE_LFXO),
        // but the internal RC is kept here for now: selecting an absent LFXO stalls at boot, and
        // this board has no SWD attached to tell that apart from any other early fault.
        nrf_config.gpiote_interrupt_priority = Priority::P2;
        nrf_config.time_interrupt_priority = Priority::P2;
        let peripherals = embassy_nrf::init(nrf_config);

        let (identity, flash) = {
            let mut nvmc = Nvmc::new(peripherals.NVMC);
            let rng = Rng::new_blocking(peripherals.RNG);
            let identity = bootstrap(&mut nvmc, rng);
            (identity, nvmc)
        };

        interrupt::USBD.set_priority(Priority::P2);
        interrupt::TWISPI0.set_priority(Priority::P3);
        interrupt::UARTE0.set_priority(Priority::P3);
        let usb = Driver::new(peripherals.USBD, Irqs, HardwareVbusDetect::new(Irqs));

        let gnss = {
            let mut gnss_uart_config = uarte::Config::default();
            // The XIAO L76K speaks NMEA at 9600, not the 115200 this HAL defaults to.
            gnss_uart_config.baudrate = Baudrate::BAUD9600;
            let uart = Uarte::new(
                peripherals.UARTE0,
                // RXD first: the MCU receives on P1.11, which is the receiver's TX.
                peripherals.P1_11,
                peripherals.P1_12,
                Irqs,
                gnss_uart_config,
            );
            super::Gnss::new(
                uart,
                Output::new(peripherals.P1_05, Level::Low, OutputDrive::Standard),
                Output::new(peripherals.P1_03, Level::Low, OutputDrive::Standard),
                Output::new(peripherals.P0_02, Level::Low, OutputDrive::Standard),
            )
        };

        let mut radio_spim_config = spim::Config::default();
        radio_spim_config.frequency = spim::Frequency::M4;
        let radio_bus = Spim::new(
            peripherals.TWISPI0,
            Irqs,
            peripherals.P1_13,
            peripherals.P1_14,
            peripherals.P1_15,
            radio_spim_config,
        );
        let radio_cs = Output::new(peripherals.P0_04, Level::High, OutputDrive::Standard);
        let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
        let radio_busy = Input::new(peripherals.P0_29, Pull::None);
        let radio_dio1 = Input::new(peripherals.P0_03, Pull::None);

        HELD_IO.lock(|held| {
            *held.borrow_mut() = Some(HeldIo {
                radio_rx_enable: Output::new(peripherals.P0_05, Level::Low, OutputDrive::Standard),
            });
        });

        let mut radio_reset = Output::new(peripherals.P0_28, Level::Low, OutputDrive::Standard);
        Timer::after_millis(10).await;
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
                frontend_control: FrontendControl::TxRx {
                    enter_transmit,
                    enter_receive,
                },
            },
        );

        let status_led = StatusLed::active_high(Output::new(
            peripherals.P0_19,
            Level::Low,
            OutputDrive::Standard,
        ));

        (
            identity,
            SolarNodeHardware {
                flash,
                usb,
                radio,
                status_led,
                gnss,
            },
        )
    }
}

fn enter_transmit() {
    HELD_IO.lock(|held| {
        if let Some(io) = held.borrow_mut().as_mut() {
            io.radio_rx_enable.set_low();
        }
    });
}

fn enter_receive() {
    HELD_IO.lock(|held| {
        if let Some(io) = held.borrow_mut().as_mut() {
            io.radio_rx_enable.set_high();
        }
    });
}
