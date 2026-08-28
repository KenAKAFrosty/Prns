use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, AdcPin, Attenuation};
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::uart::{Config as UartConfig, Uart};
use esp_hal::Async;

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_time::{Delay, Duration, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use ssd1306::mode::BufferedGraphicsMode;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use personal_rns::interfaces::InterfaceId;
use personal_rns::radios::sx126x::{BoardConfig, ExternalPowerAmplifier, Sx126x, TcxoVoltage};

use personal_hopspot_core as screen;

use super::heltec_frontend;
use crate::s3::{
    self, BoardFace, Esp32S3Board, GnssProvider, GnssShared, ImmediateBoardDisplay,
    ImmediateDisplayDevice, S3BoardHardware, S3InterfaceHardware, S3ManifoldHardware,
};

/// This board's USB-auto interface id (the always-present top-level wire on pool slot 0).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"heltecv4");

/// This node's `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])`
/// = `fixarray(2)` ‖ `bin8("Personal Hopspot HeltecV4")` ‖ `nil`, the shape LXMF apps parse.
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x19Personal Hopspot HeltecV4\xc0";
const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot HeltecV4";

const VBAT_DIVIDER_NUM: u32 = 49;
const VBAT_DIVIDER_DEN: u32 = 10;
const GNSS_RESET_HOLD: Duration = Duration::from_millis(100);
const GNSS_WARMUP: Duration = Duration::from_secs(1);
const GNSS_ERROR_RETRY: Duration = Duration::from_millis(100);
const GNSS_READ_BYTES: usize = 64;

static GNSS_SHARED: GnssShared = GnssShared::new();

/// How far the fast voltage average must lead the slow one to read as "charging". The Heltec V4 has
/// no charge/VBUS pin, so charging is inferred from the cell's voltage trend: plugging in steps the
/// terminal voltage up and charging trends it up. Below this the cell is idle, discharging, or full
/// (flat). Tuned above ADC/load noise (load dips pull the fast average *down*, never up).
const CHARGE_RISE_MV: u32 = 16;

/// The Heltec V4's battery sense: VBAT on a 49:10 divider into ADC1 (GPIO1), gated by GPIO37. The
/// shared [`BatteryGauge`](screen::BatteryGauge) owns the percentage curve; this reads the divided
/// millivolts and keeps two EMAs (fast + slow) so [`external_power`](Self::external_power) can infer the
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

/// The standard V4's L76K transport and electrical controls. NMEA parsing and public positioning
/// types stay in `prns-core`; this adapter owns only the Heltec pin policy and ESP-HAL UART.
pub struct HeltecV4Gnss {
    uart: Uart<'static, Async>,
    enable: Output<'static>,
    reset: Output<'static>,
    standby: Output<'static>,
    _pulse_per_second: Input<'static>,
}

impl HeltecV4Gnss {
    fn stop(&mut self) {
        // VGNSS_Ctrl and reset are active-low. Letting standby fall before removing power keeps the
        // receiver quiescent, and holding reset asserted prevents stale UART output while disabled.
        self.standby.set_low();
        self.reset.set_low();
        self.enable.set_high();
    }

    async fn start(&mut self) {
        self.reset.set_low();
        self.standby.set_low();
        self.enable.set_low();
        Timer::after(GNSS_RESET_HOLD).await;
        self.reset.set_high();
        self.standby.set_high();
        Timer::after(GNSS_WARMUP).await;
    }
}

impl GnssProvider for HeltecV4Gnss {
    const AVAILABILITY: screen::GnssAvailability = screen::GnssAvailability::Available;

    fn control(command: screen::GnssReceiverCommand) {
        GNSS_SHARED.control(command);
    }

    fn snapshot() -> Option<screen::GnssSnapshot> {
        Some(GNSS_SHARED.snapshot())
    }

    async fn drive(mut self) {
        self.stop();
        GNSS_SHARED.publish(screen::GnssSnapshot::Disabled);

        loop {
            match GNSS_SHARED.wait_command().await {
                screen::GnssReceiverCommand::Enable => {}
                screen::GnssReceiverCommand::Disable => {
                    self.stop();
                    GNSS_SHARED.publish(screen::GnssSnapshot::Disabled);
                    continue;
                }
            }

            GNSS_SHARED.publish(screen::GnssSnapshot::Starting);
            self.start().await;
            let mut parser = screen::NmeaParser::new();
            GNSS_SHARED.publish(screen::GnssSnapshot::Searching { satellites: 0 });
            let mut enabled = true;

            while enabled {
                let mut bytes = [0u8; GNSS_READ_BYTES];
                match select(self.uart.read_async(&mut bytes), GNSS_SHARED.wait_command()).await {
                    Either::First(Ok(read)) => {
                        for &byte in &bytes[..read] {
                            if let Some(snapshot) = parser.feed(byte) {
                                GNSS_SHARED.publish(snapshot);
                            }
                        }
                    }
                    Either::First(Err(error)) => {
                        log::warn!("GNSS UART receive failed: {error:?}");
                        GNSS_SHARED.publish(screen::GnssSnapshot::Error);
                        match select(Timer::after(GNSS_ERROR_RETRY), GNSS_SHARED.wait_command())
                            .await
                        {
                            Either::First(()) => {}
                            Either::Second(command) => {
                                enabled = command == screen::GnssReceiverCommand::Enable;
                            }
                        }
                    }
                    Either::Second(command) => {
                        enabled = command == screen::GnssReceiverCommand::Enable;
                    }
                }
            }

            self.stop();
            GNSS_SHARED.publish(screen::GnssSnapshot::Disabled);
        }
    }
}

/// The Heltec V4's board half (OLED/battery/radio bring-up); everything past it is the shared
/// [`s3`] core.
pub struct HeltecBoard;

impl Esp32S3Board for HeltecBoard {
    const ANNOUNCE_APP_DATA: &'static [u8] = ANNOUNCE_APP_DATA;
    const NODE_ANNOUNCE_APP_DATA: &'static [u8] = NODE_ANNOUNCE_APP_DATA;
    const BOOT_BANNER: &'static str = "HOPSPOT_HELTECV4";
    const USB_INTERFACE_ID: InterfaceId = USB_INTERFACE_ID;
    const FLASH_LAYOUT: screen::HopspotS3FlashLayout = screen::S3_16_MIB_FLASH_LAYOUT;
    #[cfg(feature = "lora")]
    const MAX_TX_POWER_DBM: i8 = 28;
    type Display = ImmediateBoardDisplay<HeltecDisplay>;
    type Battery = HeltecBattery;
    type Gnss = HeltecV4Gnss;

    async fn bringup(
        p: esp_hal::peripherals::Peripherals,
    ) -> S3BoardHardware<Self::Display, Self::Battery, Self::Gnss> {
        let (sw_int1, timebase, rtc) = s3::boot_common!(p, Self::BOOT_BANNER);

        // GPIO35 drives the V4's bright white user LED active-high. Claim it immediately so the
        // reset/default pin state cannot leave the LED lit while the Hopspot is running.
        let _user_led = Output::new(p.GPIO35, Level::Low, OutputConfig::default());

        // Heltec's standard V4 routes its optional L76K through the 8-pin GNSS connector. Establish
        // the disabled electrical state before slower display/radio bring-up: VGNSS_Ctrl (34) and
        // reset (42) are active-low, while standby (40) must be high to force the receiver awake.
        let gnss_enable = Output::new(p.GPIO34, Level::High, OutputConfig::default());
        let gnss_reset = Output::new(p.GPIO42, Level::Low, OutputConfig::default());
        let gnss_standby = Output::new(p.GPIO40, Level::Low, OutputConfig::default());
        let gnss_pps = Input::new(p.GPIO41, InputConfig::default());
        let gnss_uart = Uart::new(p.UART1, UartConfig::default().with_baudrate(9_600))
            .expect("L76K UART configuration is valid")
            // Heltec names these connector nets from the receiver's perspective: GPIO39 carries
            // GNSS TX toward the ESP RX, while GPIO38 carries ESP TX toward GNSS RX.
            .with_rx(p.GPIO39)
            .with_tx(p.GPIO38)
            .into_async();
        let gnss = HeltecV4Gnss {
            uart: gnss_uart,
            enable: gnss_enable,
            reset: gnss_reset,
            standby: gnss_standby,
            _pulse_per_second: gnss_pps,
        };

        s3::boot_stage(s3::BootPhase::DisplayHardwareBegin);
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
                    external_power_amplifier: Some(ExternalPowerAmplifier {
                        minimum_output_power_dbm: 5,
                        maximum_output_power_dbm: 28,
                        chip_power_dbm: heltec_frontend::heltec_fem_chip_power_dbm,
                    }),
                    frontend_control: lora_frontend.control(),
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
            gnss,
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
    s3::run::<HeltecBoard>(spawner).await
}
