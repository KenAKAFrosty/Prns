mod display;

use embassy_executor::Spawner;
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{
    gpio::{Input, InputConfig, Level, Output, OutputConfig},
    psram::{PsramConfig, PsramMode, PsramSize, SpiRamFreq},
    spi::{
        master::{Config as SpiConfig, Spi},
        Mode as SpiMode,
    },
    time::Rate,
};
use personal_hopspot_core as screen;
use personal_rns::{
    interfaces::InterfaceId,
    radios::sx126x::{BoardConfig, FrontendControl, Sx126x, TcxoVoltage},
};

use crate::{
    heltec_e290_ssd1680,
    s3::{
        self, BoardDisplay, BoardFace, Esp32S3Board, NoGnss, S3BoardHardware, S3InterfaceHardware,
        S3ManifoldHardware,
    },
};

use self::display::{DisplaySpi, E290Display, E290DisplayError};

const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"e290-usb");
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x15Personal Hopspot E290\xc0";
const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot E290";

/// Heltec Vision Master E290-HF V0.3.1 composition. The fitted LF radio assembly, GPIO7 battery
/// divider, QuickLink pins, and UART expansion remain outside this qualification target.
pub struct HeltecE290Board;

impl Esp32S3Board for HeltecE290Board {
    const ANNOUNCE_APP_DATA: &'static [u8] = ANNOUNCE_APP_DATA;
    const NODE_ANNOUNCE_APP_DATA: &'static [u8] = NODE_ANNOUNCE_APP_DATA;
    const BOOT_BANNER: &'static str = "HOPSPOT_HELTEC_E290";
    const USB_INTERFACE_ID: InterfaceId = USB_INTERFACE_ID;
    const FLASH_LAYOUT: screen::HopspotS3FlashLayout = screen::S3_16_MIB_FLASH_LAYOUT;
    const USER_BLANKING: screen::UserBlanking = screen::UserBlanking::Unavailable;
    type Display = E290Display;
    type DisplayError = E290DisplayError;
    type Presentation = heltec_e290_ssd1680::E290Presentation;
    type Battery = screen::NoBattery;
    type Gnss = NoGnss;

    fn presentation() -> Self::Presentation {
        heltec_e290_ssd1680::presentation_state()
    }

    async fn present(
        display: &mut Self::Display,
        frame: &screen::face_64x128::Frame,
        kind: screen::presentation::RefreshKind,
    ) -> Result<(), Self::DisplayError> {
        display.present(frame, kind).await
    }

    fn set_display_awake(
        _display: &mut Self::Display,
        _awake: bool,
    ) -> Result<(), Self::DisplayError> {
        // The panel retains visible ink after its controller and rail sleep. The shared UI is
        // capability-gated and never calls this for E290 user blanking.
        Ok(())
    }

    async fn bringup(
        p: esp_hal::peripherals::Peripherals,
    ) -> S3BoardHardware<Self::Display, Self::Battery, Self::Gnss> {
        // Establish every externally active safe state before PSRAM, allocators, display, identity,
        // storage, or network initialization can fail.
        let display_power = Output::new(p.GPIO18, Level::Low, OutputConfig::default());
        let display_cs = Output::new(p.GPIO3, Level::High, OutputConfig::default());
        let display_data_command = Output::new(p.GPIO4, Level::Low, OutputConfig::default());
        let display_reset = Output::new(p.GPIO5, Level::Low, OutputConfig::default());
        let radio_cs = Output::new(p.GPIO8, Level::High, OutputConfig::default());
        let radio_reset = Output::new(p.GPIO12, Level::Low, OutputConfig::default());
        let button = Input::new(p.GPIO21, InputConfig::default());

        let (sw_int1, timebase, rtc) = s3::boot_common!(
            p,
            Self::BOOT_BANNER,
            PsramConfig {
                mode: PsramMode::OctalSpi,
                size: PsramSize::Size(8 * 1024 * 1024),
                ram_frequency: SpiRamFreq::Freq40m,
                ..Default::default()
            }
        );

        s3::boot_stage(s3::BootPhase::DisplayHardwareBegin);
        let display_busy = Input::new(p.GPIO6, InputConfig::default());
        let display_spi: Option<DisplaySpi> = Spi::new(
            p.SPI3,
            SpiConfig::default()
                .with_frequency(Rate::from_mhz(6))
                .with_mode(SpiMode::_0),
        )
        .ok()
        .map(|spi| spi.with_sck(p.GPIO2).with_mosi(p.GPIO1).into_async())
        .and_then(|spi| ExclusiveDevice::new(spi, display_cs, Delay).ok());
        let display = E290Display::new(
            display_spi,
            display_data_command,
            display_reset,
            display_busy,
            display_power,
        );
        let display_available = display.is_available();
        if display_available {
            s3::boot_stage(s3::BootPhase::DisplayHardwareReady);
            log::info!("E290 SSD1680 presenter ready; first refresh deferred to display runtime");
        } else {
            s3::boot_stage(s3::BootPhase::DisplayHardwareFailed);
            log::error!("E290 SSD1680 SPI initialization failed; routing will continue");
        }

        let lora_spi = Spi::new(
            p.SPI2,
            SpiConfig::default().with_frequency(Rate::from_mhz(8)),
        )
        .expect("E290 LoRa SPI2 configuration is valid")
        .with_sck(p.GPIO9)
        .with_mosi(p.GPIO10)
        .with_miso(p.GPIO11)
        .into_async();
        let lora_spi =
            ExclusiveDevice::new(lora_spi, radio_cs, Delay).expect("E290 LoRa chip select");
        let lora_busy = Input::new(p.GPIO13, InputConfig::default());
        let lora_dio1 = Input::new(p.GPIO14, InputConfig::default());
        let lora_radio = Sx126x::new(
            lora_spi,
            lora_busy,
            lora_dio1,
            radio_reset,
            Delay,
            BoardConfig {
                tcxo_voltage: Some(TcxoVoltage::V1_8),
                use_dcdc: true,
                rx_boost: false,
                dio2_as_rf_switch: true,
                external_rx_gain_db: 0,
                external_power_amplifier: None,
                frontend_control: FrontendControl::NoDynamicControl,
            },
        );

        S3BoardHardware {
            face: BoardFace {
                display: BoardDisplay {
                    device: display,
                    available: display_available,
                },
                battery: screen::NoBattery,
                button,
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
    s3::run::<HeltecE290Board>(spawner).await
}
