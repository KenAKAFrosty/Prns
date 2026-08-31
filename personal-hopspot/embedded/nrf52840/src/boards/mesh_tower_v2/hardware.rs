use core::cell::RefCell;

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

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
});

type MeshTowerV2SpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

type MeshTowerV2Radio =
    Sx126x<MeshTowerV2SpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type MeshTowerV2LoraInterface = LoRaInterface<'static, MeshTowerV2Radio>;

type MeshTowerV2UsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

pub(crate) struct MeshTowerV2Hardware {
    pub(crate) usb: MeshTowerV2UsbDriver,
    pub(crate) vbus: &'static SoftwareVbusDetect,
    pub(crate) radio: MeshTowerV2Radio,
    pub(crate) status_led: StatusLed,
    pub(crate) button: Input<'static>,
}

struct HeldIo {
    fem_enable: Output<'static>,
    fem_tx_rx: Output<'static>,
    watchdog_done: Output<'static>,
    _watchdog_wake: Input<'static>,
    _gps_en: Output<'static>,
    _pa_detect: Input<'static>,
}

static HELD_IO: Mutex<CriticalSectionRawMutex, RefCell<Option<HeldIo>>> =
    Mutex::new(RefCell::new(None));

pub(crate) struct MeshTowerV2Board;

impl MeshTowerV2Board {
    pub(crate) async fn initialize<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, &mut Rng<'static, Blocking>) -> R,
    ) -> (R, MeshTowerV2Hardware) {
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

        // SoftDevice reserves P0/P1; keep app interrupts off those. USB at P2, SPI at P3 so a BLE
        // radio event can preempt LoRa SPI.
        interrupt::USBD.set_priority(Priority::P2);
        interrupt::TWISPI0.set_priority(Priority::P3);
        static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
        let vbus = crate::runtime::software_vbus::initialize(&SOFTWARE_VBUS);
        let usb = Driver::new(peripherals.USBD, Irqs, vbus);

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

        let pa_detect = Input::new(peripherals.P0_13, Pull::None);
        let mut fem_enable = Output::new(peripherals.P0_15, Level::High, OutputDrive::Standard);
        fem_enable.set_high();
        Timer::after_millis(1).await;
        let mut fem_tx_rx = Output::new(peripherals.P0_16, Level::Low, OutputDrive::Standard);
        fem_tx_rx.set_low();

        // P0.09/P0.10 are NFC by default; board-mesh-tower-v2 enables nfc-pins-as-gpio.
        let mut watchdog_done = Output::new(peripherals.P0_09, Level::Low, OutputDrive::Standard);
        let watchdog_wake = Input::new(peripherals.P0_10, Pull::None);
        watchdog_done.set_low();
        Timer::after_millis(1).await;
        watchdog_done.set_high();
        Timer::after_millis(1).await;
        watchdog_done.set_low();

        // GPS_EN is active-low; hold it high so the L76K stays off until a GPS face exists.
        let gps_en = Output::new(peripherals.P0_07, Level::High, OutputDrive::Standard);

        HELD_IO.lock(|held| {
            *held.borrow_mut() = Some(HeldIo {
                fem_enable,
                fem_tx_rx,
                watchdog_done,
                _watchdog_wake: watchdog_wake,
                _gps_en: gps_en,
                _pa_detect: pa_detect,
            });
        });

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
                external_power_amplifier: Some(ExternalPowerAmplifier {
                    minimum_output_power_dbm: 5,
                    maximum_output_power_dbm: 30,
                    chip_power_dbm: fem_8db_chip_power_dbm,
                }),
                frontend_control: FrontendControl::TxRx {
                    enter_transmit,
                    enter_receive,
                },
            },
        );

        let status_led = StatusLed::active_low(Output::new(
            peripherals.P1_15,
            Level::High,
            OutputDrive::Standard,
        ));
        let button = Input::new(peripherals.P1_10, Pull::Up);

        (
            identity,
            MeshTowerV2Hardware {
                usb,
                vbus,
                radio,
                status_led,
                button,
            },
        )
    }
}

fn enter_transmit() {
    HELD_IO.lock(|held| {
        if let Some(io) = held.borrow_mut().as_mut() {
            io.fem_enable.set_high();
            io.fem_tx_rx.set_high();
        }
    });
}

fn enter_receive() {
    HELD_IO.lock(|held| {
        if let Some(io) = held.borrow_mut().as_mut() {
            io.fem_enable.set_high();
            io.fem_tx_rx.set_low();
        }
    });
}

pub(crate) fn pet_watchdog() {
    HELD_IO.lock(|held| {
        if let Some(io) = held.borrow_mut().as_mut() {
            io.watchdog_done.set_high();
        }
    });
}

pub(crate) fn release_watchdog() {
    HELD_IO.lock(|held| {
        if let Some(io) = held.borrow_mut().as_mut() {
            io.watchdog_done.set_low();
        }
    });
}

/// Map antenna-referred power through the Mesh Tower FEM (~8 dB gain) into SX1262 chip power.
fn fem_8db_chip_power_dbm(requested_output_dbm: i8) -> i8 {
    requested_output_dbm.saturating_sub(8).clamp(-9, 22)
}
