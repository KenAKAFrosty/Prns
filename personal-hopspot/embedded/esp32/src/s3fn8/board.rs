use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::TrngSource;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart::{Config as UartConfig, Uart};
use personal_rns::engine::InstantMillis;
use personal_rns::manifold::embassy::EmbassyTimebase;
use personal_rns::radios::sx126x::{BoardConfig, FrontendControl, Sx126x, TcxoVoltage};

use super::{LoraRadio, S3Fn8Hardware, USB_UART_BAUD};

const HEAP_BYTES: usize = 64 * 1024;

pub(super) fn bringup() -> S3Fn8Hardware {
    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(size: HEAP_BYTES);
    esp_println::println!(
        "HOPSPOT_HELTEC_WSL_V3 boot {} commit={}",
        env!("HOPSPOT_BUILD_IDENTITY"),
        env!("HOPSPOT_BUILD_COMMIT_SHORT")
    );

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let vext = Output::new(peripherals.GPIO36, Level::Low, OutputConfig::default());
    let adc_control = Output::new(peripherals.GPIO37, Level::High, OutputConfig::default());

    let uart = Uart::new(
        peripherals.UART0,
        UartConfig::default().with_baudrate(USB_UART_BAUD),
    )
    .expect("Wireless Stick Lite UART0 configuration is valid")
    .with_rx(peripherals.GPIO44)
    .with_tx(peripherals.GPIO43)
    .into_async();
    let (usb_rx, usb_tx) = uart.split();

    let lora_spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default().with_frequency(Rate::from_mhz(8)),
    )
    .expect("Wireless Stick Lite LoRa SPI2 configuration is valid")
    .with_sck(peripherals.GPIO9)
    .with_mosi(peripherals.GPIO10)
    .with_miso(peripherals.GPIO11)
    .into_async();
    let lora_cs = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());
    let lora_spi = ExclusiveDevice::new(lora_spi, lora_cs, Delay)
        .expect("Wireless Stick Lite LoRa chip select is valid");
    let lora_reset = Output::new(peripherals.GPIO12, Level::High, OutputConfig::default());
    let lora_busy = Input::new(peripherals.GPIO13, InputConfig::default());
    let lora_dio1 = Input::new(peripherals.GPIO14, InputConfig::default());
    let lora_radio: LoraRadio = Sx126x::new(
        lora_spi,
        lora_busy,
        lora_dio1,
        lora_reset,
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

    let timer_group = TimerGroup::new(peripherals.TIMG0);
    let software_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timer_group.timer0, software_interrupt.software_interrupt0);

    let mut rtc = Rtc::new(peripherals.LPWR);
    rtc.rwdt.disable();
    rtc.swd.disable();
    let timebase = EmbassyTimebase::start_at(InstantMillis(rtc.current_time_us() / 1000));
    let identity_entropy = TrngSource::new(peripherals.RNG, peripherals.ADC1);

    let base_mac = base_mac_address();
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&base_mac.as_bytes()[..6]);

    S3Fn8Hardware {
        usb_rx,
        usb_tx,
        lora_radio,
        bluetooth: peripherals.BT,
        identity_entropy,
        mac,
        timebase,
        _rtc: rtc,
        _vext: vext,
        _adc_control: adc_control,
    }
}
