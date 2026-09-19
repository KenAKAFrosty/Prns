use embassy_executor::Spawner;
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;

use personal_hopspot_core as screen;
use personal_rns::interfaces::lora::{RadioProfile, Region, DEFAULT_915_PROFILE};
use personal_rns::interfaces::InterfaceId;
use personal_rns::radios::sx126x::{BoardConfig, Sx126x, TcxoVoltage};

use super::wio_sx1262_frontend;
use crate::s3::{
    self, BoardFace, Esp32S3Board, HeadlessBoardDisplay, NoGnss, S3BoardHardware,
    S3InterfaceHardware, S3ManifoldHardware,
};

const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"xiaos3wi");

/// `msgpack([display_name, stamp_cost])` for the built-in `lxmf.delivery` destination.
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x18Personal Hopspot XIAO S3\xc0";
const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot XIAO S3";

const EU868_PROFILE: RadioProfile = RadioProfile {
    frequency: Region::Eu868.default_frequency(),
    tx_power: Region::Eu868.max_tx_power(),
    region: Region::Eu868,
    ..DEFAULT_915_PROFILE
};

/// Seeed Studio XIAO ESP32S3 mounted on the Wio-SX1262 B2B baseboard.
pub struct XiaoEsp32S3WioSx1262Board;

impl Esp32S3Board for XiaoEsp32S3WioSx1262Board {
    const ANNOUNCE_APP_DATA: &'static [u8] = ANNOUNCE_APP_DATA;
    const NODE_ANNOUNCE_APP_DATA: &'static [u8] = NODE_ANNOUNCE_APP_DATA;
    const BOOT_BANNER: &'static str = "HOPSPOT_XIAO_ESP32S3_WIO_SX1262";
    const USB_INTERFACE_ID: InterfaceId = USB_INTERFACE_ID;
    const FLASH_LAYOUT: screen::HopspotS3FlashLayout = screen::S3_8_MIB_FLASH_LAYOUT;
    const DEFAULT_LORA_PROFILE: RadioProfile = EU868_PROFILE;
    type Display = HeadlessBoardDisplay;
    type Battery = screen::NoBattery;
    type Gnss = NoGnss;

    async fn bringup(
        mut p: esp_hal::peripherals::Peripherals,
    ) -> S3BoardHardware<Self::Display, Self::Battery, Self::Gnss> {
        // XIAO ESP32S3 uses the ESP32-S3R8 SiP: 8 MiB Octal PSRAM and 8 MiB flash.
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
        let runtime_bootstrap =
            s3::bootstrap_s3_runtime(&mut p.RNG, &mut p.ADC1, Self::FLASH_LAYOUT).await;

        log::info!("headless board: display, battery gauge, and GNSS unavailable");

        let lora_radio = {
            // Seeed's B2B mapping: SCK 7, MISO 8, MOSI 9, NSS 41, RESET 42,
            // BUSY 40, DIO1 39, and the external receive-enable path on GPIO38.
            let lora_spi = Spi::new(
                p.SPI2,
                SpiConfig::default().with_frequency(Rate::from_mhz(8)),
            )
            .expect("Wio-SX1262 SPI2 configuration is valid")
            .with_sck(p.GPIO7)
            .with_mosi(p.GPIO9)
            .with_miso(p.GPIO8)
            .into_async();
            let lora_cs = Output::new(p.GPIO41, Level::High, OutputConfig::default());
            let lora_spi_device =
                ExclusiveDevice::new(lora_spi, lora_cs, Delay).expect("Wio-SX1262 SPI device");
            let lora_reset = Output::new(p.GPIO42, Level::High, OutputConfig::default());
            let lora_busy = Input::new(p.GPIO40, InputConfig::default());
            let lora_dio1 = Input::new(p.GPIO39, InputConfig::default());
            let frontend_control = wio_sx1262_frontend::initialize(p.GPIO38);

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
                    frontend_control,
                },
            )
        };

        S3BoardHardware {
            runtime_bootstrap,
            face: BoardFace {
                display: HeadlessBoardDisplay,
                battery: screen::NoBattery,
                button: Input::new(
                    p.GPIO21,
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
    s3::run::<XiaoEsp32S3WioSx1262Board>(spawner).await
}
