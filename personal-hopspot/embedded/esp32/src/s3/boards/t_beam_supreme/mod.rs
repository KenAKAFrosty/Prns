use core::cell::RefCell;

use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::uart::{Config as UartConfig, Uart};
use esp_println::println;

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Delay, Duration, Timer};
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{OriginDimensions, Point, Size};
use embedded_graphics::Pixel;
use embedded_hal_bus::spi::ExclusiveDevice;

use personal_rns::interfaces::InterfaceId;
use personal_rns::radios::sx126x::{BoardConfig, Sx126x, TcxoVoltage};

use personal_hopspot_core as screen;
use static_cell::StaticCell;

use crate::s3::{
    self, BoardDisplay, BoardFace, Esp32S3Board, S3BoardHardware, S3InterfaceHardware,
    S3ManifoldHardware,
};

mod gnss;

use gnss::TBeamSupremeGnss;

/// This board's USB-auto interface id (the always-present top-level wire on pool slot 0).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"tbeamsup");

/// This node's `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])`
/// = `fixarray(2)` ‖ `bin8("Personal Hopspot T-Beam Supreme")` ‖ `nil`, the shape LXMF apps parse.
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x1fPersonal Hopspot T-Beam Supreme\xc0";
const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot T-Beam Supreme";

/// The AXP2101 PMU (I2C1 on SDA 42 / SCL 41). On the T-Beam S3 Supreme the SX1262, GPS, and
/// OLED+sensor bus all sit behind PMU LDO rails that boot OFF: ALDO3 → LoRa, ALDO4 → GNSS, and
/// ALDO1 + ALDO2 → OLED/sensor bus. Registers per the AXP2101 datasheet (matching XPowersLib):
/// 0x90 is the LDO on/off control, while 0x92..0x95 hold the ALDO1..4 voltages (0.5–3.5 V,
/// 100 mV steps, so 3.3 V = (3300-500)/100).
const AXP2101_ADDR: u8 = 0x34;
const AXP2101_LDO_ONOFF0: u8 = 0x90;
const AXP2101_ALDO1_VOL: u8 = 0x92;
const AXP2101_ALDO2_VOL: u8 = 0x93;
const AXP2101_ALDO3_VOL: u8 = 0x94;
const AXP2101_ALDO4_VOL: u8 = 0x95;
const AXP2101_VOL_3V3: u8 = 0x1c;
const AXP2101_ALDO123_EN: u8 = 0b0000_0111;
const AXP2101_ALDO4_EN: u8 = 0b0000_1000;
/// ADC channel-enable register; bit0 turns on the battery-voltage ADC the fuel gauge reads.
const AXP2101_ADC_EN: u8 = 0x30;
const AXP2101_ADC_BATV: u8 = 0x01;
/// Battery-voltage ADC result, high byte at 0x34 (6 valid bits) + low byte at 0x35 = mV.
const AXP2101_BATV_H: u8 = 0x34;
/// PMU status register 1; bit5 ("VBUS good") is set when external USB power is present.
const AXP2101_STATUS1: u8 = 0x00;
const AXP2101_VBUS_GOOD: u8 = 0x20;

enum Axp2101AccessError {
    Bus,
}

enum GnssPower {
    On,
    Off,
}

/// Power the SX1262 (ALDO3) and the OLED + sensor bus (ALDO1, ALDO2) on by setting each rail to
/// 3.3 V and flipping its on/off bit. Read-modify-write so the rest of each register (the voltage
/// regs' high 3 bits, the on/off reg's other rails) is preserved. Returns false if the PMU never
/// ACKs — i.e. it is absent or the I2C bus is mis-wired, which the boot log surfaces.
fn axp2101_bringup<I: embedded_hal::i2c::I2c>(i2c: &mut I) -> Result<(), Axp2101AccessError> {
    for reg in [
        AXP2101_ALDO1_VOL,
        AXP2101_ALDO2_VOL,
        AXP2101_ALDO3_VOL,
        AXP2101_ALDO4_VOL,
    ] {
        let mut cur = [0u8];
        i2c.write_read(AXP2101_ADDR, &[reg], &mut cur)
            .map_err(|_| Axp2101AccessError::Bus)?;
        let val = (cur[0] & 0xe0) | AXP2101_VOL_3V3;
        i2c.write(AXP2101_ADDR, &[reg, val])
            .map_err(|_| Axp2101AccessError::Bus)?;
    }
    let mut onoff = [0u8];
    i2c.write_read(AXP2101_ADDR, &[AXP2101_LDO_ONOFF0], &mut onoff)
        .map_err(|_| Axp2101AccessError::Bus)?;
    let val = onoff[0] | AXP2101_ALDO123_EN;
    i2c.write(AXP2101_ADDR, &[AXP2101_LDO_ONOFF0, val])
        .map_err(|_| Axp2101AccessError::Bus)?;
    let mut adc = [0u8];
    if i2c
        .write_read(AXP2101_ADDR, &[AXP2101_ADC_EN], &mut adc)
        .is_ok()
    {
        let _ = i2c.write(AXP2101_ADDR, &[AXP2101_ADC_EN, adc[0] | AXP2101_ADC_BATV]);
    }
    Ok(())
}

/// Switch the Supreme's GNSS supply (ALDO4, bit3 of the LDO on/off register). The receiver backup
/// domain remains supplied by the board's battery circuit, preserving hot-start state when present.
fn axp2101_set_gnss_power<I: embedded_hal::i2c::I2c>(
    i2c: &mut I,
    power: GnssPower,
) -> Result<(), Axp2101AccessError> {
    let mut r = [0u8];
    i2c.write_read(AXP2101_ADDR, &[AXP2101_LDO_ONOFF0], &mut r)
        .map_err(|_| Axp2101AccessError::Bus)?;
    let value = match power {
        GnssPower::On => r[0] | AXP2101_ALDO4_EN,
        GnssPower::Off => r[0] & !AXP2101_ALDO4_EN,
    };
    i2c.write(AXP2101_ADDR, &[AXP2101_LDO_ONOFF0, value])
        .map_err(|_| Axp2101AccessError::Bus)
}

/// Switch the OLED's power rail (ALDO1, bit0 of the on/off register) on or off. A watchdog reset
/// reboots the ESP but leaves the AXP rails latched, so the SH1106 never sees a clean power-on and
/// can wedge; toggling ALDO1 off→on at boot guarantees the panel resets with the firmware.
fn axp2101_set_aldo1<I: embedded_hal::i2c::I2c>(i2c: &mut I, on: bool) {
    let mut r = [0u8];
    if i2c
        .write_read(AXP2101_ADDR, &[AXP2101_LDO_ONOFF0], &mut r)
        .is_err()
    {
        return;
    }
    let v = if on { r[0] | 0x01 } else { r[0] & !0x01 };
    let _ = i2c.write(AXP2101_ADDR, &[AXP2101_LDO_ONOFF0, v]);
}

type TBeamI2c = I2c<'static, esp_hal::Blocking>;

struct TBeamPmu {
    i2c: Mutex<CriticalSectionRawMutex, RefCell<TBeamI2c>>,
}

impl TBeamPmu {
    fn new(i2c: TBeamI2c) -> Self {
        Self {
            i2c: Mutex::new(RefCell::new(i2c)),
        }
    }

    fn with_i2c<R>(&self, f: impl FnOnce(&mut TBeamI2c) -> R) -> R {
        self.i2c.lock(|i2c| f(&mut i2c.borrow_mut()))
    }

    fn set_gnss_power(&self, power: GnssPower) -> Result<(), Axp2101AccessError> {
        self.with_i2c(|i2c| axp2101_set_gnss_power(i2c, power))
    }
}

static PMU: StaticCell<TBeamPmu> = StaticCell::new();

/// Reads battery voltage from the AXP2101's fuel-gauge ADC over I2C — the T-Beam S3 Supreme's only
/// battery telemetry (there is no GPIO divider). Returns `None` when no cell is attached (the ADC
/// reads ~0) so the gauge shows `Unknown` on USB-only power.
pub struct Axp2101Battery {
    pmu: &'static TBeamPmu,
}

impl Axp2101Battery {
    fn new(pmu: &'static TBeamPmu) -> Self {
        Self { pmu }
    }
}

impl screen::BatterySource for Axp2101Battery {
    fn read_millivolts(&mut self) -> Option<u32> {
        let mut buf = [0u8; 2];
        self.pmu
            .with_i2c(|i2c| i2c.write_read(AXP2101_ADDR, &[AXP2101_BATV_H], &mut buf))
            .ok()?;
        let mv = (((buf[0] & 0x3f) as u32) << 8) | buf[1] as u32;
        (mv >= 100).then_some(mv)
    }

    fn external_power(&mut self) -> screen::ExternalPowerState {
        let mut s = [0u8];
        if self
            .pmu
            .with_i2c(|i2c| i2c.write_read(AXP2101_ADDR, &[AXP2101_STATUS1], &mut s))
            .is_err()
        {
            return screen::ExternalPowerState::Unknown;
        }
        screen::ExternalPowerState::from_presence(s[0] & AXP2101_VBUS_GOOD != 0)
    }
}

const SH1106_ADDR: u8 = 0x3d;
/// The SH1106's RAM is 132 columns wide but the glass shows columns 2..130, so every page write
/// starts at column 2. This is the one detail that makes the SH1106 incompatible with the SSD1306
/// driver's full-width horizontal-addressing flush.
const SH1106_COL_OFFSET: u8 = 2;
/// SH1106 panel: 128 columns × 64 rows = 8 pages of 128 bytes. The UI draws into a 64×128 portrait
/// canvas, so the `DrawTarget` reports 64×128 and rotates each pixel 90° into this physical buffer.
const SH1106_W: u32 = 128;
const SH1106_H: u32 = 64;

/// A minimal SH1106 OLED driver over I2C: a 1 KiB framebuffer that implements the same
/// `embedded_graphics` `DrawTarget<Color = BinaryColor>` the shared UI renders into, flushed page by
/// page (the SH1106 only supports page addressing). No external crate — the published `sh1106` crate
/// is stuck on embedded-hal 0.2, which this esp-hal stack does not implement.
pub struct Sh1106I2c<I> {
    i2c: I,
    buf: [u8; (SH1106_W * SH1106_H / 8) as usize],
}

impl<I: embedded_hal::i2c::I2c> Sh1106I2c<I> {
    fn new(i2c: I) -> Self {
        Self {
            i2c,
            buf: [0u8; (SH1106_W * SH1106_H / 8) as usize],
        }
    }

    fn cmd(&mut self, byte: u8) -> Result<(), ()> {
        self.i2c.write(SH1106_ADDR, &[0x00, byte]).map_err(|_| ())
    }

    fn init(&mut self) -> Result<(), ()> {
        for byte in [
            0xae, 0xd5, 0x80, 0xa8, 0x3f, 0xd3, 0x00, 0x40, 0xad, 0x8b, 0xa1, 0xc8, 0xda, 0x12,
            0x81, 0xcf, 0xd9, 0x1f, 0xdb, 0x40, 0xa4, 0xa6, 0xaf,
        ] {
            self.cmd(byte)?;
        }
        self.flush()
    }

    fn flush(&mut self) -> Result<(), ()> {
        for page in 0..8u8 {
            self.cmd(0xb0 | page)?;
            self.cmd(SH1106_COL_OFFSET & 0x0f)?;
            self.cmd(0x10 | (SH1106_COL_OFFSET >> 4))?;
            let start = page as usize * SH1106_W as usize;
            let mut chunk = [0u8; SH1106_W as usize + 1];
            chunk[0] = 0x40;
            chunk[1..].copy_from_slice(&self.buf[start..start + SH1106_W as usize]);
            self.i2c.write(SH1106_ADDR, &chunk).map_err(|_| ())?;
        }
        Ok(())
    }

    fn set_display_on(&mut self, on: bool) -> Result<(), ()> {
        self.cmd(0xae | u8::from(on))
    }
}

impl<I> OriginDimensions for Sh1106I2c<I> {
    fn size(&self) -> Size {
        Size::new(SH1106_H, SH1106_W)
    }
}

impl<I: embedded_hal::i2c::I2c> DrawTarget for Sh1106I2c<I> {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<T>(&mut self, pixels: T) -> Result<(), Self::Error>
    where
        T: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(Point { x, y }, color) in pixels {
            if x < 0 || y < 0 || x >= SH1106_H as i32 || y >= SH1106_W as i32 {
                continue;
            }
            let px = (SH1106_W - 1) - y as u32;
            let py = x as u32;
            let idx = (px + (py / 8) * SH1106_W) as usize;
            let bit = 1u8 << (py % 8);
            if color == BinaryColor::On {
                self.buf[idx] |= bit;
            } else {
                self.buf[idx] &= !bit;
            }
        }
        Ok(())
    }
}

/// The LilyGO T-Beam S3 Supreme: an AXP2101 PMU gates the SX1262 + OLED rails (so they boot dark
/// until [`axp2101_bringup`]), the panel is an SH1106 at 0x3D, and the battery is read over the PMU's
/// fuel-gauge ADC. Everything past bring-up is the shared [`s3`] core.
pub struct TBeamSupremeBoard;

impl Esp32S3Board for TBeamSupremeBoard {
    const ANNOUNCE_APP_DATA: &'static [u8] = ANNOUNCE_APP_DATA;
    const NODE_ANNOUNCE_APP_DATA: &'static [u8] = NODE_ANNOUNCE_APP_DATA;
    const BOOT_BANNER: &'static str = "HOPSPOT_TBEAM_SUPREME";
    const USB_INTERFACE_ID: InterfaceId = USB_INTERFACE_ID;
    const FLASH_LAYOUT: screen::HopspotS3FlashLayout = screen::S3_8_MIB_FLASH_LAYOUT;
    type Display = Sh1106I2c<TBeamI2c>;
    type Battery = Axp2101Battery;
    type Gnss = TBeamSupremeGnss;

    fn flush(display: &mut Self::Display) {
        let _ = display.flush();
    }

    fn set_display_awake(display: &mut Self::Display, awake: bool) {
        let _ = display.set_display_on(awake);
    }

    async fn bringup(
        p: esp_hal::peripherals::Peripherals,
    ) -> S3BoardHardware<Self::Display, Self::Battery, Self::Gnss> {
        let (sw_int1, timebase, rtc) = s3::boot_common!(p, Self::BOOT_BANNER);

        s3::boot_stage(s3::BootPhase::OledBegin);
        // AXP2101 PMU first (I2C1 on SDA 42 / SCL 41): the LoRa + OLED rails boot OFF, so nothing else
        // on those rails responds until this enables them. The handle is kept as the battery sense.
        let mut pmu_i2c = I2c::new(
            p.I2C1,
            I2cConfig::default().with_frequency(Rate::from_khz(400)),
        )
        .expect("i2c1")
        .with_sda(p.GPIO42)
        .with_scl(p.GPIO41);
        let pmu_status = match axp2101_bringup(&mut pmu_i2c) {
            Ok(()) => "ok (ALDO1/2/3 @ 3.3V)",
            Err(Axp2101AccessError::Bus) => "FAILED (no ACK — radio + OLED stay dark)",
        };
        let gnss_rail_status = match axp2101_set_gnss_power(&mut pmu_i2c, GnssPower::Off) {
            Ok(()) => "off",
            Err(Axp2101AccessError::Bus) => "unknown",
        };
        println!(
            "AXP2101 bring-up: {}; GNSS rail: {}",
            pmu_status, gnss_rail_status
        );
        Timer::after(Duration::from_millis(50)).await;
        // Power-cycle the OLED rail so the SH1106 resets cleanly even when a watchdog reset left it
        // powered (otherwise its display-on/charge-pump state can wedge and re-init silently no-ops).
        axp2101_set_aldo1(&mut pmu_i2c, false);
        Timer::after(Duration::from_millis(80)).await;
        axp2101_set_aldo1(&mut pmu_i2c, true);
        Timer::after(Duration::from_millis(120)).await;
        // Battery telemetry and GNSS power changes share the blocking PMU bus. The critical-section
        // mutex keeps their short register transactions coherent across the display and GNSS tasks.
        let pmu = PMU.init(TBeamPmu::new(pmu_i2c));
        let mut battery = Axp2101Battery::new(pmu);
        {
            use screen::BatterySource as _;
            println!("battery: {:?} mV", battery.read_millivolts());
        }

        // OLED (T-Beam S3 Supreme: powered via the PMU above, no Vext/RST GPIO; I2C0 on 17/18). The
        // panel is an SH1106 (132-col page-addressed RAM, 2-px offset), so it uses the sh1106 driver
        // rather than the Heltec's ssd1306 — both render through the same embedded-graphics DrawTarget.
        let i2c = I2c::new(
            p.I2C0,
            I2cConfig::default().with_frequency(Rate::from_khz(400)),
        )
        .expect("i2c0")
        .with_sda(p.GPIO17)
        .with_scl(p.GPIO18);
        let mut display = Sh1106I2c::new(i2c);
        let oled_ok = display.init().is_ok();
        s3::boot_stage(if oled_ok {
            s3::BootPhase::OledReady
        } else {
            s3::BootPhase::OledFailed
        });
        if oled_ok {
            screen::splash(&mut display, screen::SplashContent::Brand);
            let _ = display.flush();
        }

        let lora_radio = {
            let lora_spi = Spi::new(
                p.SPI2,
                SpiConfig::default().with_frequency(Rate::from_mhz(8)),
            )
            .expect("lora spi2")
            .with_sck(p.GPIO12)
            .with_mosi(p.GPIO11)
            .with_miso(p.GPIO13)
            .into_async();
            let lora_cs = Output::new(p.GPIO10, Level::High, OutputConfig::default());
            let lora_spi_device =
                ExclusiveDevice::new(lora_spi, lora_cs, Delay).expect("lora spi device");
            let lora_reset = Output::new(p.GPIO5, Level::High, OutputConfig::default());
            let lora_busy = Input::new(p.GPIO4, InputConfig::default());
            let lora_dio1 = Input::new(p.GPIO1, InputConfig::default());
            // No external FEM/PA GPIOs on the T-Beam S3 Supreme: the SX1262's DIO2 drives the TX/RX
            // switch internally (dio2_as_rf_switch below) and the radio rail is the PMU's ALDO3.
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
                    external_power_amplifier: None,
                    enter_transmit: None,
                    enter_receive: None,
                },
            )
        };

        // LilyGO names these pins from the ESP side: GPIO9 receives the receiver's NMEA output,
        // while GPIO8 transmits toward it. GPIO7 wakes L76K boards and is unconnected on MAX-M10S;
        // the shared ALDO4 control above powers either receiver option.
        let gnss_uart = Uart::new(p.UART1, UartConfig::default().with_baudrate(9_600))
            .expect("T-Beam GNSS UART configuration is valid")
            .with_rx(p.GPIO9)
            .with_tx(p.GPIO8)
            .into_async();
        let gnss = TBeamSupremeGnss::new(
            gnss_uart,
            Output::new(p.GPIO7, Level::Low, OutputConfig::default()),
            Input::new(p.GPIO6, InputConfig::default()),
            pmu,
        );

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
    s3::run::<TBeamSupremeBoard>(spawner).await
}
