use core::cell::RefCell;

use embassy_nrf::config::{HfclkSource, LfclkSource};
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

type Rak10724SpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

type Rak10724Radio =
    Sx126x<Rak10724SpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type Rak10724LoraInterface = LoRaInterface<'static, Rak10724Radio>;

type Rak10724UsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

pub(crate) struct Rak10724Hardware {
    pub(crate) usb: Rak10724UsbDriver,
    pub(crate) vbus: &'static SoftwareVbusDetect,
    pub(crate) radio: Rak10724Radio,
    pub(crate) status_led: StatusLed,
    pub(crate) button: Input<'static>,
}

struct HeldIo {
    _fem_enable: Output<'static>,
    _peripheral_power: Output<'static>,
    _blue_led: Output<'static>,
}

static HELD_IO: Mutex<CriticalSectionRawMutex, RefCell<Option<HeldIo>>> =
    Mutex::new(RefCell::new(None));

pub(crate) struct Rak10724Board;

impl Rak10724Board {
    pub(crate) async fn initialize<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, Rng<'static, Blocking>) -> R,
    ) -> (R, Rak10724Hardware) {
        let mut nrf_config = config::Config::default();
        nrf_config.hfclk_source = HfclkSource::ExternalXtal;
        nrf_config.lfclk_source = LfclkSource::ExternalXtal;
        nrf_config.gpiote_interrupt_priority = Priority::P2;
        nrf_config.time_interrupt_priority = Priority::P2;
        let peripherals = embassy_nrf::init(nrf_config);

        pet_bootloader_watchdog();
        disable_leftover_softdevice();
        pet_bootloader_watchdog();
        // UF2 / leftover Meshtastic may have S140 on USB; let POWER settle before NVMC/USBD.
        Timer::after_millis(100).await;
        pet_bootloader_watchdog();

        let identity = {
            let mut nvmc = Nvmc::new(peripherals.NVMC);
            let rng = Rng::new_blocking(peripherals.RNG);
            pet_bootloader_watchdog();
            let identity = bootstrap(&mut nvmc, rng);
            pet_bootloader_watchdog();
            identity
        };

        interrupt::USBD.set_priority(Priority::P2);
        interrupt::TWISPI0.set_priority(Priority::P3);
        static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
        let vbus = crate::runtime::software_vbus::initialize(&SOFTWARE_VBUS);
        let usb = Driver::new(peripherals.USBD, Irqs, vbus);

        // RAK13302: WB_IO2 / P1.02 holds the 3V3_S rail and the 5 V boost for the SKY66122.
        // WisBlock IO3 / P0.21 is CSD+CPS tied together; high keeps the FEM in LNA/PA mode.
        // SX1262 DIO2 owns CTX, so there is no separate TX/RX GPIO.
        let peripheral_power = Output::new(peripherals.P1_02, Level::High, OutputDrive::Standard);
        let fem_enable = Output::new(peripherals.P0_21, Level::High, OutputDrive::Standard);
        let blue_led = Output::new(peripherals.P1_04, Level::Low, OutputDrive::Standard);
        HELD_IO.lock(|held| {
            *held.borrow_mut() = Some(HeldIo {
                _fem_enable: fem_enable,
                _peripheral_power: peripheral_power,
                _blue_led: blue_led,
            });
        });

        let mut radio_spim_config = spim::Config::default();
        radio_spim_config.frequency = spim::Frequency::M4;
        // WisBlock IO-slot SPI on the RAK3401: SCK P0.03, MISO P0.29, MOSI P0.30, NSS P0.26.
        let radio_bus = Spim::new(
            peripherals.TWISPI0,
            Irqs,
            peripherals.P0_03,
            peripherals.P0_29,
            peripherals.P0_30,
            radio_spim_config,
        );
        let radio_cs = Output::new(peripherals.P0_26, Level::High, OutputDrive::Standard);
        let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
        // BUSY is WisBlock IO5 / P0.09 and DIO1 is IO6 / P0.10. Both are NFC pins.
        let radio_busy = Input::new(peripherals.P0_09, Pull::None);
        let radio_dio1 = Input::new(peripherals.P0_10, Pull::None);
        let mut radio_reset = Output::new(peripherals.P0_04, Level::Low, OutputDrive::Standard);
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
                external_rx_gain_db: SKY66122_RX_GAIN_DB,
                external_power_amplifier: Some(ExternalPowerAmplifier {
                    minimum_output_power_dbm: 7,
                    maximum_output_power_dbm: 30,
                    chip_power_dbm: rak13302_chip_power_dbm,
                }),
                frontend_control: FrontendControl::NoDynamicControl,
            },
        );

        let status_led = StatusLed::active_high(Output::new(
            peripherals.P1_03,
            Level::Low,
            OutputDrive::Standard,
        ));
        let button = Input::new(peripherals.P0_08, Pull::Up);

        (
            identity,
            Rak10724Hardware {
                usb,
                vbus,
                radio,
                status_led,
                button,
            },
        )
    }
}

/// SKY66122-11 typical receive-path gain. The RAK13302 SAW is not subtracted separately.
const SKY66122_RX_GAIN_DB: u8 = 16;

/// Meshtastic `TX_GAIN_LORA` for `RAK13302`, indexed by SX1262 chip power 0..=21 dBm.
/// Antenna power is chip power plus this gain. The same indexing is used for the T096 PA.
const fn rak13302_chip_power_dbm(requested_output_dbm: i8) -> i8 {
    const GAIN_DB_BY_CHIP_POWER: [i8; 22] = [
        7, 8, 8, 8, 8, 8, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 8,
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
    assert!(rak13302_chip_power_dbm(22) == 13);
    assert!(rak13302_chip_power_dbm(30) == 22);
};

fn pet_bootloader_watchdog() {
    let wdt = embassy_nrf::pac::WDT;
    if wdt.runstatus().read().runstatus() {
        for index in 0..8 {
            wdt.rr(index)
                .write(|register| register.set_rr(embassy_nrf::pac::wdt::vals::Rr::RELOAD));
        }
    }
}

fn disable_leftover_softdevice() {
    let mut enabled = 0_u8;
    // SAFETY: The Adafruit MBR implements this SVC whether S140 is on or off.
    let _ = unsafe { nrf_softdevice::raw::sd_softdevice_is_enabled(&mut enabled) };
    if enabled != 0 {
        // SAFETY: S140 is enabled; disable returns the RNG/NVMC peripherals to the application.
        let _ = unsafe { nrf_softdevice::raw::sd_softdevice_disable() };
    }
}
