use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, AdcPin, Attenuation};
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;

use embassy_executor::Spawner;
use embassy_time::{Delay, Duration, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use ssd1306::mode::BufferedGraphicsMode;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use personal_rns::interfaces::InterfaceId;
use personal_rns::radios::sx126x::{BoardConfig, Sx126x, TcxoVoltage};

use personal_hopspot_core as screen;

use super::heltec_frontend;
use crate::s3::{
    self, BoardFace, Esp32S3Board, ImmediateBoardDisplay, ImmediateDisplayDevice, NoGnss,
    S3BoardHardware, S3InterfaceHardware, S3ManifoldHardware,
};

/// This board's USB-auto interface id (the always-present top-level wire on pool slot 0).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltecr8");

/// This node's `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])`
/// = `fixarray(2)` ‖ `bin8("Personal Hopspot HeltecV4-R8")` ‖ `nil`, the shape LXMF apps parse.
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x1cPersonal Hopspot HeltecV4-R8\xc0";
const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot HeltecV4-R8";

const VBAT_DIVIDER_NUM: u32 = 49;
const VBAT_DIVIDER_DEN: u32 = 10;

/// How far the fast voltage average must lead the slow one to read as "charging". The Heltec V4-R8
/// has no charge/VBUS pin, so charging is inferred from the cell's voltage trend: plugging in steps
/// the terminal voltage up and charging trends it up. Below this the cell is idle, discharging, or
/// full (flat). Tuned above ADC/load noise (load dips pull the fast average *down*, never up).
const CHARGE_RISE_MV: u32 = 16;

/// The Heltec V4-R8's battery sense: VBAT on a 49:10 divider into ADC1 (GPIO1). Unlike the S3R2 V4,
/// ADC_Ctrl is not broken out — do not claim GPIO37 (that pad is SPIDQS on the Octal SiP). The shared
/// [`BatteryGauge`](screen::BatteryGauge) owns the percentage curve; this reads the divided
/// millivolts and keeps two EMAs (fast + slow) so [`external_power`](Self::external_power) can infer the
/// plugged/charging state this board gives no direct signal for. ADC oneshots can report
/// `WouldBlock`, so a read is retried briefly.
pub struct HeltecR8Battery {
    adc: Adc<'static, esp_hal::peripherals::ADC1<'static>, esp_hal::Blocking>,
    pin: AdcPin<
        esp_hal::peripherals::GPIO1<'static>,
        esp_hal::peripherals::ADC1<'static>,
        AdcCalCurve<esp_hal::peripherals::ADC1<'static>>,
    >,
    fast_ema_mv: u32,
    slow_ema_mv: u32,
}

impl screen::BatterySource for HeltecR8Battery {
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

    /// Inferred charging: the fast voltage average leading the slow one by [`CHARGE_RISE_MV`] means
    /// the terminal voltage is stepping/trending up (plug-in or active charge). Fades when the cell
    /// is full (flat) or on unplug (step down) — an approximation that answers "did plugging in
    /// actually start charging?", which is the signal that matters on a board with no charge pin.
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

pub struct HeltecDisplay(HeltecController);

impl ImmediateDisplayDevice for HeltecDisplay {
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

/// The Heltec V4-R8's board half (OLED/battery/radio bring-up); everything past it is the shared
/// [`s3`] core. Pinout differs from the S3R2 V4 for Vext and battery gating; PSRAM is Octal 8MB.
pub struct HeltecV4R8Board;

impl Esp32S3Board for HeltecV4R8Board {
    const ANNOUNCE_APP_DATA: &'static [u8] = ANNOUNCE_APP_DATA;
    const NODE_ANNOUNCE_APP_DATA: &'static [u8] = NODE_ANNOUNCE_APP_DATA;
    const BOOT_BANNER: &'static str = "HOPSPOT_HELTECV4_R8";
    const USB_INTERFACE_ID: InterfaceId = USB_INTERFACE_ID;
    const FLASH_LAYOUT: screen::HopspotS3FlashLayout = screen::S3_16_MIB_FLASH_LAYOUT;
    type Display = ImmediateBoardDisplay<HeltecDisplay>;
    type Battery = HeltecR8Battery;
    type Gnss = NoGnss;

    async fn bringup(
        p: esp_hal::peripherals::Peripherals,
    ) -> S3BoardHardware<Self::Display, Self::Battery, Self::Gnss> {
        // Octal 8 MiB at 40 MHz, split between a private low engine window and a global high `esp_alloc` window.
        // Vext is GPIO40; GPIO36 is FSPICLK/SPIIO7 on the R8 SiP, and driving it as GPIO disrupts PSRAM access after early probes.
        let (sw_int1, timebase, rtc) = s3::boot_common!(
            p,
            Self::BOOT_BANNER,
            ::esp_hal::psram::PsramConfig {
                mode: ::esp_hal::psram::PsramMode::OctalSpi,
                size: ::esp_hal::psram::PsramSize::Size(8 * 1024 * 1024),
                ram_frequency: ::esp_hal::psram::SpiRamFreq::Freq40m,
                ..::core::default::Default::default()
            }
        );

        s3::boot_stage(s3::BootPhase::DisplayHardwareBegin);
        // OLED (V4-R8: Vext on GPIO40 active-low; pulse RST; I2C0 on 17/18).
        let mut _vext = Output::new(p.GPIO40, Level::Low, OutputConfig::default());
        let mut rst = Output::new(p.GPIO21, Level::High, OutputConfig::default());
        rst.set_low();
        Timer::after(Duration::from_millis(20)).await;
        rst.set_high();
        Timer::after(Duration::from_millis(20)).await;
        let i2c = I2c::new(
            p.I2C0,
            I2cConfig::default().with_frequency(Rate::from_khz(400)),
        )
        .expect("i2c0")
        .with_sda(p.GPIO17)
        .with_scl(p.GPIO18);
        let mut display = Ssd1306::new(
            I2CDisplayInterface::new(i2c),
            DisplaySize128x64,
            DisplayRotation::Rotate90,
        )
        .into_buffered_graphics_mode();
        let oled_ok = match display.init() {
            Ok(()) => {
                s3::boot_stage(s3::BootPhase::DisplayHardwareReady);
                log::info!("OLED initialized");
                true
            }
            Err(error) => {
                s3::boot_stage(s3::BootPhase::DisplayHardwareFailed);
                log::error!("OLED initialization failed: {error:?}");
                false
            }
        };
        let mut display = HeltecDisplay(display);
        if oled_ok {
            let mut frame = screen::face_64x128::Frame::new();
            screen::face_64x128::splash(&mut frame, screen::face_64x128::SplashContent::Brand);
            let _ = display.present(&frame);
        }

        let lora_radio = {
            let lora_spi = Spi::new(
                p.SPI2,
                SpiConfig::default().with_frequency(Rate::from_mhz(8)),
            )
            .expect("lora spi2")
            .with_sck(p.GPIO9)
            .with_mosi(p.GPIO10)
            .with_miso(p.GPIO11)
            .into_async();
            let lora_cs = Output::new(p.GPIO8, Level::High, OutputConfig::default());
            let lora_spi_device =
                ExclusiveDevice::new(lora_spi, lora_cs, Delay).expect("lora spi device");
            let lora_reset = Output::new(p.GPIO12, Level::High, OutputConfig::default());
            let lora_busy = Input::new(p.GPIO13, InputConfig::default());
            let lora_dio1 = Input::new(p.GPIO14, InputConfig::default());
            let lora_frontend = heltec_frontend::initialize(p.GPIO7, p.GPIO2, p.GPIO46, p.GPIO5);
            Sx126x::new(
                lora_spi_device,
                lora_busy,
                lora_dio1,
                lora_reset,
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
            )
        };

        // Battery sense: VBAT on GPIO1, ungated (ADC_Ctrl removed on R8).
        let mut adc_cfg = AdcConfig::new();
        let vbat_pin =
            adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<_>>(p.GPIO1, Attenuation::_11dB);
        let vbat_adc = Adc::new(p.ADC1, adc_cfg);
        let battery = HeltecR8Battery {
            adc: vbat_adc,
            pin: vbat_pin,
            fast_ema_mv: 0,
            slow_ema_mv: 0,
        };

        S3BoardHardware {
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
                usb_device: p.USB_DEVICE,
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
    s3::run::<HeltecV4R8Board>(spawner).await
}
