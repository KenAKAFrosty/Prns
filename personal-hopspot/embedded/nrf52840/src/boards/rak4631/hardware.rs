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
use personal_rns::radios::sx126x::{BoardConfig, FrontendControl, Sx126x, TcxoVoltage};
use static_cell::StaticCell;

use crate::boards::status_led::StatusLed;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    TWISPI0 => spim::InterruptHandler<peripherals::TWISPI0>;
});

type Rak4631SpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

type Rak4631Radio =
    Sx126x<Rak4631SpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type Rak4631LoraInterface = LoRaInterface<'static, Rak4631Radio>;

type Rak4631UsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

pub(crate) struct Rak4631Hardware {
    pub(crate) usb: Rak4631UsbDriver,
    pub(crate) vbus: &'static SoftwareVbusDetect,
    pub(crate) radio: Rak4631Radio,
    pub(crate) status_led: StatusLed,
    pub(crate) button: Input<'static>,
}

struct HeldIo {
    _radio_power_en: Output<'static>,
    _peripheral_power: Output<'static>,
    _blue_led: Output<'static>,
}

static HELD_IO: Mutex<CriticalSectionRawMutex, RefCell<Option<HeldIo>>> =
    Mutex::new(RefCell::new(None));

pub(crate) struct Rak4631Board;

impl Rak4631Board {
    pub(crate) async fn initialize<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, Rng<'static, Blocking>) -> R,
    ) -> (R, Rak4631Hardware) {
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

        // SX1262 POWER_EN (Meshtastic GPIO 37 / P1.05) must stay high. DIO2 owns the antenna
        // switch, so P1.07/TXEN is left uninitialized. WisBlock 3V3_S (P1.02) stays on.
        let radio_power_en = Output::new(peripherals.P1_05, Level::High, OutputDrive::Standard);
        let peripheral_power = Output::new(peripherals.P1_02, Level::High, OutputDrive::Standard);
        let blue_led = Output::new(peripherals.P1_04, Level::Low, OutputDrive::Standard);
        HELD_IO.lock(|held| {
            *held.borrow_mut() = Some(HeldIo {
                _radio_power_en: radio_power_en,
                _peripheral_power: peripheral_power,
                _blue_led: blue_led,
            });
        });

        let mut radio_spim_config = spim::Config::default();
        radio_spim_config.frequency = spim::Frequency::M4;
        let radio_bus = Spim::new(
            peripherals.TWISPI0,
            Irqs,
            peripherals.P1_11,
            peripherals.P1_13,
            peripherals.P1_12,
            radio_spim_config,
        );
        let radio_cs = Output::new(peripherals.P1_10, Level::High, OutputDrive::Standard);
        let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
        let radio_busy = Input::new(peripherals.P1_14, Pull::None);
        let radio_dio1 = Input::new(peripherals.P1_15, Pull::None);
        let mut radio_reset = Output::new(peripherals.P1_06, Level::Low, OutputDrive::Standard);
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

        let status_led = StatusLed::active_high(Output::new(
            peripherals.P1_03,
            Level::Low,
            OutputDrive::Standard,
        ));
        // P0.09 is NFC by default; board-rak4631 enables nfc-pins-as-gpio.
        let button = Input::new(peripherals.P0_09, Pull::Up);

        (
            identity,
            Rak4631Hardware {
                usb,
                vbus,
                radio,
                status_led,
                button,
            },
        )
    }
}

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
