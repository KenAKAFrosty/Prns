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

type XiaoSpiDevice = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

type XiaoRadio = Sx126x<XiaoSpiDevice, Input<'static>, Input<'static>, Output<'static>, Delay>;

pub(crate) type XiaoLoraInterface = LoRaInterface<'static, 'static, XiaoRadio>;

type XiaoUsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

pub(crate) struct XiaoHardware {
    pub(crate) usb: XiaoUsbDriver,
    pub(crate) vbus: &'static SoftwareVbusDetect,
    pub(crate) radio: XiaoRadio,
    pub(crate) status_led: StatusLed,
    pub(crate) button: Input<'static>,
}

struct HeldIo {
    radio_rx_enable: Output<'static>,
    _red_led: Output<'static>,
    _blue_led: Output<'static>,
    _battery_divider_enable: Output<'static>,
}

static HELD_IO: Mutex<CriticalSectionRawMutex, RefCell<Option<HeldIo>>> =
    Mutex::new(RefCell::new(None));

pub(crate) struct XiaoBoard;

impl XiaoBoard {
    pub(crate) async fn initialize<R>(
        bootstrap: impl FnOnce(&mut Nvmc<'static>, Rng<'static, Blocking>) -> R,
    ) -> (R, XiaoHardware) {
        let mut nrf_config = config::Config::default();
        nrf_config.hfclk_source = HfclkSource::ExternalXtal;
        // S140 runs its LF clock from the RC oscillator, so the runtime does not depend on the
        // XIAO's optional 32.768 kHz crystal.
        nrf_config.lfclk_source = LfclkSource::InternalRC;
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

        // The Wio-SX1262 RF switch's receive path is selected by RXEN (D5 / P0.05); SX1262 DIO2
        // drives transmit, as on the Wio Tracker L1 that carries the same module.
        let radio_rx_enable = Output::new(peripherals.P0_05, Level::Low, OutputDrive::Standard);
        // The RGB LED is active-low; green is the status LED, red and blue are held dark.
        let red_led = Output::new(peripherals.P0_26, Level::High, OutputDrive::Standard);
        let blue_led = Output::new(peripherals.P0_06, Level::High, OutputDrive::Standard);
        // P0.14 low connects the VBAT divider to P0.31. Driving it high with a battery attached
        // exposes P0.31 to the full cell voltage, so it is held low for the life of the firmware.
        let battery_divider_enable =
            Output::new(peripherals.P0_14, Level::Low, OutputDrive::Standard);
        HELD_IO.lock(|held| {
            *held.borrow_mut() = Some(HeldIo {
                radio_rx_enable,
                _red_led: red_led,
                _blue_led: blue_led,
                _battery_divider_enable: battery_divider_enable,
            });
        });

        // XIAO D8 / D9 / D10 are SCK / MISO / MOSI.
        let mut radio_spim_config = spim::Config::default();
        radio_spim_config.frequency = spim::Frequency::M8;
        let radio_bus = Spim::new(
            peripherals.TWISPI0,
            Irqs,
            peripherals.P1_13,
            peripherals.P1_14,
            peripherals.P1_15,
            radio_spim_config,
        );
        // D4 / D1 / D3 / D2 are NSS / DIO1 / BUSY / NRESET on Seeed's XIAO + Wio-SX1262 kit.
        let radio_cs = Output::new(peripherals.P0_04, Level::High, OutputDrive::Standard);
        let radio_spi = ExclusiveDevice::new(radio_bus, radio_cs, Delay).unwrap();
        let radio_busy = Input::new(peripherals.P0_29, Pull::None);
        let radio_dio1 = Input::new(peripherals.P0_03, Pull::None);
        let mut radio_reset = Output::new(peripherals.P0_28, Level::Low, OutputDrive::Standard);
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
                frontend_control: FrontendControl::TxRx {
                    enter_transmit,
                    enter_receive,
                },
            },
        );

        let status_led = StatusLed::active_low(Output::new(
            peripherals.P0_30,
            Level::High,
            OutputDrive::Standard,
        ));
        let button = Input::new(peripherals.P0_02, Pull::Up);

        (
            identity,
            XiaoHardware {
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
