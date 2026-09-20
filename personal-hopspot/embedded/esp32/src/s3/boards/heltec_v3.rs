use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, AdcPin, Attenuation};
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::uart::{Config as UartConfig, Uart};

use embassy_executor::Spawner;
use embassy_time::{Delay, Duration, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use ssd1306::mode::BufferedGraphicsMode;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use personal_hopspot_core as screen;
use personal_rns::interfaces::InterfaceId;
use personal_rns::radios::sx126x::{BoardConfig, Sx126x, TcxoVoltage};

use super::heltec_frontend;
use crate::s3::{
    self, BoardFace, Esp32S3Board, ImmediateBoardDisplay, ImmediateDisplayDevice, NoGnss,
    S3BoardHardware, S3InterfaceHardware, S3ManifoldHardware, S3UsbHardware,
};

const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltecv3");
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x19Personal Hopspot HeltecV3\xc0";
const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot HeltecV3";
const VBAT_DIVIDER_NUM: u32 = 49;
const VBAT_DIVIDER_DEN: u32 = 10;
const CHARGE_RISE_MV: u32 = 16;

/// Heltec V3 exposes VBAT through a 49:10 divider on GPIO1. GPIO37 gates the
/// divider; keeping it high while sampling matches the vendor schematic.
pub struct HeltecV3Battery {
    adc: Adc<'static, esp_hal::peripherals::ADC1<'static>, esp_hal::Blocking>,
    pin: AdcPin<
        esp_hal::peripherals::GPIO1<'static>,
        esp_hal::peripherals::ADC1<'static>,
        AdcCalCurve<esp_hal::peripherals::ADC1<'static>>,
    >,
    _ctrl: Output<'static>,
    fast_ema_mv: u32,
    slow_ema_mv: u32,
}

impl screen::BatterySource for HeltecV3Battery {
    fn read_millivolts(&mut self) -> Option<u32> {
        for _ in 0..1000 {
            if let Ok(raw) = self.adc.read_oneshot(&mut self.pin) {
                let mv = raw as u32 * VBAT_DIVIDER_NUM / VBAT_DIVIDER_DEN;
                if self.slow_ema_mv == 0 {
                    self.fast_ema_mv = mv;
                    self.slow_ema_mv = mv;
                } else {
                    self.fast_ema_mv = (self.fast_ema_mv * 3 + mv) / 4;
                    self.slow_ema_mv = (self.slow_ema_mv * 15 + mv) / 16;
                }
                return Some(mv);
            }
        }
        None
    }

    fn external_power(&mut self) -> screen::ExternalPowerState {
        if self.fast_ema_mv > self.slow_ema_mv.saturating_add(CHARGE_RISE_MV) {
            screen::ExternalPowerState::Present {
                charging: screen::ChargingState::Charging,
            }
        } else {
            screen::ExternalPowerState::Unknown
        }
    }
}

type HeltecController = Ssd1306<
    I2CInterface<I2c<'static, esp_hal::Blocking>>,
    DisplaySize128x64,
    BufferedGraphicsMode<DisplaySize128x64>,
>;

pub struct HeltecV3Display(HeltecController);

impl ImmediateDisplayDevice for HeltecV3Display {
    fn present(
        &mut self,
        frame: &screen::face_64x128::Frame,
    ) -> screen::display::PresentationOutcome {
        if let Err(error) = crate::immediate_display::draw_canonical_frame(&mut self.0, frame) {
            log::error!("OLED frame conversion failed: {error:?}");
            return screen::display::PresentationOutcome::Failed;
        }
        match self.0.flush() {
            Ok(()) => screen::display::PresentationOutcome::Succeeded,
            Err(error) => {
                log::error!("OLED render failed: {error:?}");
                screen::display::PresentationOutcome::Failed
            }
        }
    }

    fn apply_blanking(
        &mut self,
        command: screen::display::BlankingCommand,
    ) -> screen::display::BlankingOutcome {
        let on = command == screen::display::BlankingCommand::Restore;
        let result = match self.0.set_display_on(on) {
            Ok(()) => screen::display::BlankingResult::Succeeded,
            Err(error) => {
                log::error!("OLED blanking failed: {error:?}");
                screen::display::BlankingResult::Failed
            }
        };
        screen::display::BlankingOutcome {
            result,
            buffer_retention: screen::display::BufferRetention::Preserved,
        }
    }
}

/// Heltec WiFi LoRa 32 V3: ESP32-S3, SX1262, 8 MiB flash, and no PSRAM.
pub struct HeltecV3Board;

impl Esp32S3Board for HeltecV3Board {
    const ANNOUNCE_APP_DATA: &'static [u8] = ANNOUNCE_APP_DATA;
    const NODE_ANNOUNCE_APP_DATA: &'static [u8] = NODE_ANNOUNCE_APP_DATA;
    const BOOT_BANNER: &'static str = "HOPSPOT_HELTECV3";
    const USB_INTERFACE_ID: InterfaceId = USB_INTERFACE_ID;
    const FLASH_LAYOUT: screen::HopspotS3FlashLayout = screen::S3_8_MIB_FLASH_LAYOUT;
    const INTERNAL_SRAM_ONLY: bool = true;
    type Display = ImmediateBoardDisplay<HeltecV3Display>;
    type Battery = HeltecV3Battery;
    type Gnss = NoGnss;

    async fn bringup(
        mut p: esp_hal::peripherals::Peripherals,
    ) -> S3BoardHardware<Self::Display, Self::Battery, Self::Gnss> {
        let (sw_int1, timebase, rtc) = s3::boot_common!(p, Self::BOOT_BANNER, no_psram);
        s3::boot_stage(s3::BootPhase::DisplayHardwareBegin);
        // Vext is active-low. The V3 uses OLED reset GPIO21 and I2C0 GPIO17/18.
        let _vext = Output::new(p.GPIO36, Level::Low, OutputConfig::default());
        let mut reset = Output::new(p.GPIO21, Level::High, OutputConfig::default());
        reset.set_low();
        Timer::after(Duration::from_millis(20)).await;
        reset.set_high();
        Timer::after(Duration::from_millis(20)).await;
        let i2c = I2c::new(
            p.I2C0,
            I2cConfig::default().with_frequency(Rate::from_khz(400)),
        )
        .expect("I2C0 configuration is valid")
        .with_sda(p.GPIO17)
        .with_scl(p.GPIO18);
        let mut oled = Ssd1306::new(
            I2CDisplayInterface::new(i2c),
            DisplaySize128x64,
            DisplayRotation::Rotate90,
        )
        .into_buffered_graphics_mode();
        let oled_ok = match oled.init() {
            Ok(()) => {
                s3::boot_stage(s3::BootPhase::DisplayHardwareReady);
                true
            }
            Err(error) => {
                s3::boot_stage(s3::BootPhase::DisplayHardwareFailed);
                log::error!("OLED initialization failed: {error:?}");
                false
            }
        };
        let mut display = HeltecV3Display(oled);
        if oled_ok {
            let mut frame = screen::face_64x128::Frame::new();
            screen::face_64x128::splash(&mut frame, screen::face_64x128::SplashContent::Brand);
            let _ = display.present(&frame);
        }

        // The V3 has no PSRAM. Prove the board's power, reset, and I2C path
        // before the allocation-heavy identity and flash bootstrap runs.
        let runtime_bootstrap =
            s3::bootstrap_s3_runtime(&mut p.RNG, &mut p.ADC1, Self::FLASH_LAYOUT).await;

        // SX1262: SCK/MOSI/MISO 9/10/11, NSS/reset/busy/DIO1 8/12/13/14.
        let lora_spi = Spi::new(
            p.SPI2,
            SpiConfig::default().with_frequency(Rate::from_mhz(8)),
        )
        .expect("LoRa SPI2 configuration is valid")
        .with_sck(p.GPIO9)
        .with_mosi(p.GPIO10)
        .with_miso(p.GPIO11)
        .into_async();
        let lora_spi = ExclusiveDevice::new(
            lora_spi,
            Output::new(p.GPIO8, Level::High, OutputConfig::default()),
            Delay,
        )
        .expect("LoRa SPI device is valid");
        let lora_frontend = heltec_frontend::initialize(p.GPIO7, p.GPIO2, p.GPIO46, p.GPIO5);
        let lora_radio = Sx126x::new(
            lora_spi,
            Input::new(p.GPIO13, InputConfig::default()),
            Input::new(p.GPIO14, InputConfig::default()),
            Output::new(p.GPIO12, Level::High, OutputConfig::default()),
            Delay,
            BoardConfig {
                tcxo_voltage: Some(TcxoVoltage::V1_8),
                use_dcdc: true,
                rx_boost: true,
                dio2_as_rf_switch: true,
                external_rx_gain_db: lora_frontend.rx_gain_db(),
                external_power_amplifier: None,
                frontend_control: lora_frontend.control(),
            },
        );

        let adc_ctrl = Output::new(p.GPIO37, Level::High, OutputConfig::default());
        let mut adc_cfg = AdcConfig::new();
        let vbat_pin =
            adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<_>>(p.GPIO1, Attenuation::_11dB);
        let battery = HeltecV3Battery {
            adc: Adc::new(p.ADC1, adc_cfg),
            pin: vbat_pin,
            _ctrl: adc_ctrl,
            fast_ema_mv: 0,
            slow_ema_mv: 0,
        };
        // The V3 USB-C connector reaches the ESP32-S3 through a CP2102 on
        // UART0. Its native USB Serial/JTAG peripheral is not exposed there.
        let (usb_rx, usb_tx) = Uart::new(p.UART0, UartConfig::default().with_baudrate(115_200))
            .expect("USB Auto UART0 configuration is valid")
            .with_rx(p.GPIO44)
            .with_tx(p.GPIO43)
            .into_async()
            .split();

        S3BoardHardware {
            runtime_bootstrap,
            face: BoardFace {
                display: if oled_ok {
                    ImmediateBoardDisplay::initialized(display)
                } else {
                    ImmediateBoardDisplay::initialization_failed(display)
                },
                battery,
                button: Input::new(
                    p.GPIO0,
                    InputConfig::default().with_pull(esp_hal::gpio::Pull::Up),
                ),
            },
            gnss: NoGnss,
            interface_hardware: S3InterfaceHardware {
                usb: S3UsbHardware::Uart {
                    rx: usb_rx,
                    tx: usb_tx,
                },
                lora_radio,
                wifi: p.WIFI,
                bluetooth: p.BT,
            },
            manifold: S3ManifoldHardware {
                cpu_control: p.CPU_CTRL,
                software_interrupt: sw_int1,
                timebase,
                rtc,
            },
        }
    }
}

pub async fn run(spawner: Spawner) {
    s3::run::<HeltecV3Board>(spawner).await;
}
