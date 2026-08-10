use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, AdcPin, Attenuation};
use esp_hal::gpio::{Flex, Input, InputConfig, Level, Output, OutputConfig};
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

use crate::s3::{
    self, BoardDisplay, BoardFace, Esp32S3Board, S3BoardHardware, S3InterfaceHardware,
    S3ManifoldHardware,
};

const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltecv3");
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x19Personal Hopspot HeltecV3\xc0";
const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot HeltecV3";
const VBAT_DIVIDER_NUM: u32 = 49;
const VBAT_DIVIDER_DEN: u32 = 10;
const CHARGE_RISE_MV: u32 = 16;

/// The V3 senses VBAT through GPIO1 and gates its divider with GPIO37.
pub struct HeltecV3Battery {
    adc: Adc<'static, esp_hal::peripherals::ADC1<'static>, esp_hal::Blocking>,
    pin: AdcPin<
        esp_hal::peripherals::GPIO1<'static>,
        esp_hal::peripherals::ADC1<'static>,
        AdcCalCurve<esp_hal::peripherals::ADC1<'static>>,
    >,
    _ctrl: Flex<'static>,
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

    fn is_charging(&mut self) -> bool {
        self.fast_ema_mv > self.slow_ema_mv.saturating_add(CHARGE_RISE_MV)
    }
}

type HeltecV3Display = Ssd1306<
    I2CInterface<I2c<'static, esp_hal::Blocking>>,
    DisplaySize128x64,
    BufferedGraphicsMode<DisplaySize128x64>,
>;

/// Heltec WiFi LoRa 32 V3: an 8 MiB, PSRAM-free ESP32-S3 board with an SX1262.
pub struct HeltecV3Board;

impl Esp32S3Board for HeltecV3Board {
    const ANNOUNCE_APP_DATA: &'static [u8] = ANNOUNCE_APP_DATA;
    const NODE_ANNOUNCE_APP_DATA: &'static [u8] = NODE_ANNOUNCE_APP_DATA;
    const BOOT_BANNER: &'static str = "HOPSPOT_HELTECV3";
    const USB_INTERFACE_ID: InterfaceId = USB_INTERFACE_ID;
    const FLASH_LAYOUT: screen::HopspotS3FlashLayout = screen::S3_8_MIB_FLASH_LAYOUT;
    type Display = HeltecV3Display;
    type Battery = HeltecV3Battery;

    fn flush(display: &mut Self::Display) {
        if let Err(error) = display.flush() {
            log::error!("OLED render failed: {error:?}");
        }
    }

    fn set_display_awake(display: &mut Self::Display, awake: bool) {
        let _ = display.set_display_on(awake);
    }

    async fn bringup(
        p: esp_hal::peripherals::Peripherals,
    ) -> S3BoardHardware<Self::Display, Self::Battery> {
        let (sw_int1, timebase, rtc) = s3::boot_common!(p, Self::BOOT_BANNER, no_psram);

        s3::boot_stage(s3::BootPhase::OledBegin);
        let mut _vext = Output::new(p.GPIO36, Level::Low, OutputConfig::default());
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
                s3::boot_stage(s3::BootPhase::OledReady);
                true
            }
            Err(error) => {
                s3::boot_stage(s3::BootPhase::OledFailed);
                log::error!("OLED initialization failed: {error:?}");
                false
            }
        };
        if oled_ok {
            screen::splash(&mut display, screen::SplashContent::Brand);
            if let Err(error) = display.flush() {
                log::error!("OLED splash failed: {error:?}");
            }
        }

        #[cfg(feature = "lora")]
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
                    external_rx_gain_db: 0,
                },
            )
        };

        // V3 and V3.1 pull GPIO37 high and enable the divider low; V3.2 uses an active-high
        // transistor. Sampling before taking output control distinguishes the revisions.
        let mut adc_ctrl = Flex::new(p.GPIO37);
        adc_ctrl.apply_input_config(&InputConfig::default());
        adc_ctrl.set_input_enable(true);
        let adc_ctrl_active = if adc_ctrl.is_high() {
            Level::Low
        } else {
            Level::High
        };
        adc_ctrl.set_output_enable(true);
        match adc_ctrl_active {
            Level::Low => adc_ctrl.set_low(),
            Level::High => adc_ctrl.set_high(),
        }
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

        S3BoardHardware {
            face: BoardFace {
                display: BoardDisplay {
                    device: display,
                    initialized: oled_ok,
                },
                battery,
                button: Input::new(
                    p.GPIO0,
                    InputConfig::default().with_pull(esp_hal::gpio::Pull::Up),
                ),
            },
            interface_hardware: S3InterfaceHardware {
                usb_device: p.USB_DEVICE,
                #[cfg(feature = "lora")]
                lora_radio,
                #[cfg(feature = "wifi-auto")]
                wifi: p.WIFI,
                #[cfg(feature = "bluetooth-auto")]
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
    s3::run::<HeltecV3Board>(spawner).await
}
