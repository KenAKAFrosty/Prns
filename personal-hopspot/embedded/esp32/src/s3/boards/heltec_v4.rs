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
    S3ReactorHardware,
};

/// This board's USB-auto interface id (the always-present top-level wire on pool slot 0).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltecv4");

/// This node's `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])`
/// = `fixarray(2)` ‖ `bin8("Personal Hopspot HeltecV4")` ‖ `nil`, the shape LXMF apps parse.
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x19Personal Hopspot HeltecV4\xc0";
const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot HeltecV4";

const VBAT_DIVIDER_NUM: u32 = 49;
const VBAT_DIVIDER_DEN: u32 = 10;

/// How far the fast voltage average must lead the slow one to read as "charging". The Heltec V4 has
/// no charge/VBUS pin, so charging is inferred from the cell's voltage trend: plugging in steps the
/// terminal voltage up and charging trends it up. Below this the cell is idle, discharging, or full
/// (flat). Tuned above ADC/load noise (load dips pull the fast average *down*, never up).
const CHARGE_RISE_MV: u32 = 16;

/// The Heltec V4's battery sense: VBAT on a 49:10 divider into ADC1 (GPIO1), gated by GPIO37. The
/// shared [`BatteryGauge`](screen::BatteryGauge) owns the percentage curve; this reads the divided
/// millivolts and keeps two EMAs (fast + slow) so [`is_charging`](Self::is_charging) can infer the
/// plugged/charging state this board gives no direct signal for. ADC oneshots can report
/// `WouldBlock`, so a read is retried briefly.
pub struct HeltecBattery {
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

impl screen::BatterySource for HeltecBattery {
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
    fn is_charging(&mut self) -> bool {
        self.fast_ema_mv > self.slow_ema_mv.saturating_add(CHARGE_RISE_MV)
    }
}

type HeltecDisplay = Ssd1306<
    I2CInterface<I2c<'static, esp_hal::Blocking>>,
    DisplaySize128x64,
    BufferedGraphicsMode<DisplaySize128x64>,
>;

/// The Heltec V4's board half (OLED/battery/radio bring-up); everything past it is the shared
/// [`s3`] core.
pub struct HeltecBoard;

impl Esp32S3Board for HeltecBoard {
    const ANNOUNCE_APP_DATA: &'static [u8] = ANNOUNCE_APP_DATA;
    const NODE_ANNOUNCE_APP_DATA: &'static [u8] = NODE_ANNOUNCE_APP_DATA;
    const BOOT_BANNER: &'static str = "HOPSPOT_HELTECV4";
    const USB_INTERFACE_ID: InterfaceId = USB_INTERFACE_ID;
    type Display = HeltecDisplay;
    type Battery = HeltecBattery;

    fn flush(display: &mut Self::Display) {
        let _ = display.flush();
    }

    fn set_display_awake(display: &mut Self::Display, awake: bool) {
        let _ = display.set_display_on(awake);
    }

    async fn bringup(
        p: esp_hal::peripherals::Peripherals,
    ) -> S3BoardHardware<Self::Display, Self::Battery> {
        let (sw_int1, timebase, rtc) = s3::boot_common!(p, Self::BOOT_BANNER);

        // OLED (Heltec V4: Vext active-low gates panel power; pulse RST; I2C0 on 17/18).
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
        let oled_ok = display.init().is_ok();
        if oled_ok {
            screen::splash(&mut display, screen::SplashContent::Brand);
            let _ = display.flush();
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
            let _lora_pa_pwr = Output::new(p.GPIO7, Level::High, OutputConfig::default());
            let mut lora_csd = Flex::new(p.GPIO2);
            lora_csd.apply_input_config(&InputConfig::default());
            lora_csd.set_input_enable(true);
            let lora_is_kct8103l = lora_csd.is_high();
            lora_csd.set_output_enable(true);
            lora_csd.set_high();
            let _lora_fem_switch = if lora_is_kct8103l {
                Output::new(p.GPIO5, Level::High, OutputConfig::default())
            } else {
                Output::new(p.GPIO46, Level::High, OutputConfig::default())
            };
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
                },
            )
        };

        // Battery sense (Heltec V4): VBAT divider on GPIO1 (ADC1_CH0), gated by ADC_Ctrl on GPIO37.
        let mut adc_ctrl = Output::new(p.GPIO37, Level::High, OutputConfig::default());
        adc_ctrl.set_high();
        let mut adc_cfg = AdcConfig::new();
        let vbat_pin =
            adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<_>>(p.GPIO1, Attenuation::_11dB);
        let vbat_adc = Adc::new(p.ADC1, adc_cfg);
        let battery = HeltecBattery {
            adc: vbat_adc,
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
            reactor: S3ReactorHardware {
                cpu_control: p.CPU_CTRL,
                software_interrupt: sw_int1,
                timebase,
                _rtc: rtc,
            },
        }
    }
}

pub async fn run(spawner: Spawner) {
    s3::run::<HeltecBoard>(spawner).await
}
