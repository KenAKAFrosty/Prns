#![no_std]
#![no_main]

use core::convert::Infallible;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicU32, Ordering};

use panic_halt as _;

use embassy_executor::Spawner;
use embassy_futures::join::join4;
use embassy_futures::select::{select, Either};
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Delay, Duration, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, Config as UsbConfig};
use heapless::String;
use static_cell::StaticCell;

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_hal_bus::spi::ExclusiveDevice;
use epd_waveshare::color::Color as EpdColor;
use epd_waveshare::epd1in54_v2::Display1in54;

use personal_hopspot_ui as hopspot;
use personal_rns::engine::EngineState;
use personal_rns::storage::Nrf52840;

mod ssd1681;
use ssd1681::Ssd1681;

type TEchoEngineState = EngineState<Nrf52840>;

const PANEL_SIZE: i32 = 200;
const SCREEN_WIDTH: i32 = 64;
const SCREEN_HEIGHT: i32 = 128;
const SCALE_NUM: i32 = 3;
const SCALE_DEN: i32 = 2;
const SCALED_SHORT: i32 = SCREEN_WIDTH * SCALE_NUM / SCALE_DEN;
const SCALED_LONG: i32 = SCREEN_HEIGHT * SCALE_NUM / SCALE_DEN;
const SCALED_ORIGIN_X: i32 = (PANEL_SIZE - SCALED_LONG) / 2;
const SCALED_ORIGIN_Y: i32 = (PANEL_SIZE - SCALED_SHORT) / 2;

const BUTTON_LONG_PRESS: Duration = Duration::from_millis(650);
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);
const FULL_REFRESH_INTERVAL: u32 = 20;
const FRONTLIGHT_HOLD: Duration = Duration::from_secs(8);

static BUTTON_EVENTS: Channel<CriticalSectionRawMutex, hopspot::InputEvent, 4> = Channel::new();
static BUTTON_COUNT: AtomicU32 = AtomicU32::new(0);
static FRONTLIGHT_WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
    SPI2 => spim::InterruptHandler<peripherals::SPI2>;
});

// Bridges the shared Hopspot renderer (a 64x128 portrait `DrawTarget<BinaryColor>`)
// onto the 200x200 SSD1681: foreground `On` becomes black ink on a white field
// (white is e-ink's stable state under partial refresh), rotated -90 degrees, and
// scaled 1.5x to fill the square panel.
struct EinkScreen<'a> {
    panel: &'a mut Display1in54,
}

impl OriginDimensions for EinkScreen<'_> {
    fn size(&self) -> Size {
        Size::new(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32)
    }
}

impl DrawTarget for EinkScreen<'_> {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            let panel_color = match color {
                BinaryColor::On => EpdColor::Black,
                BinaryColor::Off => EpdColor::White,
            };
            let sx0 = point.x * SCALE_NUM / SCALE_DEN;
            let sx1 = (point.x + 1) * SCALE_NUM / SCALE_DEN;
            let sy0 = point.y * SCALE_NUM / SCALE_DEN;
            let sy1 = (point.y + 1) * SCALE_NUM / SCALE_DEN;
            let top_left = Point::new(
                SCALED_ORIGIN_X + sy0,
                SCALED_ORIGIN_Y + (SCALED_SHORT - sx1),
            );
            let size = Size::new((sy1 - sy0) as u32, (sx1 - sx0) as u32);
            let _ = self.panel.fill_solid(&Rectangle::new(top_left, size), panel_color);
        }
        Ok(())
    }
}

fn placeholder_cards() -> [hopspot::Card; 3] {
    [
        hopspot::Card {
            kind: hopspot::CardKind::Usb,
            label: "USB",
            selected: false,
            liveness: hopspot::Liveness::Live,
            tx_bytes: 12_400,
            rx_bytes: 68_100,
            links: 1,
            destinations: 3,
            rate_bytes_per_sec: 0,
            last_activity_secs: Some(4),
        },
        hopspot::Card {
            kind: hopspot::CardKind::LoRa,
            label: "LoRa",
            selected: false,
            liveness: hopspot::Liveness::Live,
            tx_bytes: 2_048,
            rx_bytes: 4_096,
            links: 0,
            destinations: 7,
            rate_bytes_per_sec: 0,
            last_activity_secs: Some(31),
        },
        hopspot::Card {
            kind: hopspot::CardKind::Ble,
            label: "BLE",
            selected: false,
            liveness: hopspot::Liveness::Offline,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        },
    ]
}

#[embassy_executor::task]
async fn frontlight_task(mut frontlight: Output<'static>) {
    loop {
        FRONTLIGHT_WAKE.wait().await;
        frontlight.set_high();
        while let Either::First(()) =
            select(FRONTLIGHT_WAKE.wait(), Timer::after(FRONTLIGHT_HOLD)).await
        {}
        frontlight.set_low();
    }
}

#[embassy_executor::task]
async fn button_task(mut button: Input<'static>) {
    loop {
        button.wait_for_falling_edge().await;
        FRONTLIGHT_WAKE.signal(());
        match select(
            button.wait_for_rising_edge(),
            Timer::after(BUTTON_LONG_PRESS),
        )
        .await
        {
            Either::First(()) => {
                BUTTON_COUNT.fetch_add(1, Ordering::Relaxed);
                BUTTON_EVENTS.send(hopspot::InputEvent::ShortPress).await;
            }
            Either::Second(()) => {
                BUTTON_COUNT.fetch_add(1, Ordering::Relaxed);
                BUTTON_EVENTS.send(hopspot::InputEvent::LongPress).await;
                button.wait_for_rising_edge().await;
            }
        }
        Timer::after(BUTTON_DEBOUNCE).await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) -> ! {
    let mut nrf_config = config::Config::default();
    nrf_config.hfclk_source = config::HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(nrf_config);

    let _eink_rail = Output::new(p.P0_12, Level::High, OutputDrive::Standard);

    static ENGINE: StaticCell<TEchoEngineState> = StaticCell::new();
    let state = ENGINE.init(TEchoEngineState::default());

    let mut led_green_active_low = Output::new(p.P1_01, Level::High, OutputDrive::Standard);

    let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
    let mut usb_config = UsbConfig::new(0x1209, 0x0001);
    usb_config.manufacturer = Some("Stay Personal");
    usb_config.product = Some("Personal Hopspot (T-Echo)");
    usb_config.serial_number = Some("PERSONAL-RNS-TECHO-001");
    usb_config.max_packet_size_0 = 64;

    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static USB_STATE: StaticCell<State> = StaticCell::new();

    let mut builder = Builder::new(
        driver,
        usb_config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 256]),
        CONTROL_BUF.init([0; 64]),
    );
    let mut class = CdcAcmClass::new(&mut builder, USB_STATE.init(State::new()), 64);
    let mut usb = builder.build();

    // SPIM2, not SPIM3: embassy-nrf's SPIM3 silently fails to drive these pins
    // (the SSD1681 never asserts BUSY and never refreshes); SPIM2 works.
    let mut spim_config = spim::Config::default();
    spim_config.frequency = spim::Frequency::M4;
    let eink_bus = Spim::new(p.SPI2, Irqs, p.P0_31, p.P1_06, p.P0_29, spim_config);
    let eink_cs = Output::new(p.P0_30, Level::High, OutputDrive::Standard);
    let eink_dc = Output::new(p.P0_28, Level::Low, OutputDrive::Standard);
    let eink_rst = Output::new(p.P0_02, Level::High, OutputDrive::Standard);
    let eink_busy = Input::new(p.P0_03, Pull::None);

    Timer::after(Duration::from_millis(150)).await;

    let eink_spi = ExclusiveDevice::new(eink_bus, eink_cs, Delay).unwrap();
    let mut panel = Display1in54::default();
    let eink = Ssd1681::new(eink_spi, eink_busy, eink_dc, eink_rst, Delay).ok();
    let eink_ok = eink.is_some();

    if let Ok(token) = button_task(Input::new(p.P1_10, Pull::Up)) {
        spawner.spawn(token);
    }

    if let Ok(token) =
        frontlight_task(Output::new(p.P1_11, Level::Low, OutputDrive::Standard))
    {
        spawner.spawn(token);
    }

    let usb_fut = usb.run();

    let heartbeat_fut = async {
        loop {
            class.wait_connection().await;
            loop {
                let mut line: String<96> = String::new();
                let _ = write!(
                    line,
                    "techo eink={} btn={} pkts={} route={}\r\n",
                    if eink_ok { "ok" } else { "err" },
                    BUTTON_COUNT.load(Ordering::Relaxed),
                    state.ingested_packet_count(),
                    state.route_count()
                );
                if class.write_packet(line.as_bytes()).await.is_err() {
                    break;
                }
                Timer::after(Duration::from_millis(1000)).await;
            }
        }
    };

    let blink_fut = async {
        loop {
            led_green_active_low.set_low();
            Timer::after(Duration::from_millis(500)).await;
            led_green_active_low.set_high();
            Timer::after(Duration::from_millis(500)).await;
        }
    };

    let render_fut = async {
        let mut epd = match eink {
            Some(epd) => epd,
            None => core::future::pending().await,
        };
        let cards = placeholder_cards();
        let card_count = cards.len();
        let mut ui_state = hopspot::UiState::new();
        ui_state.sync_card_count(card_count);

        let _ = panel.clear(EpdColor::White);
        hopspot::draw_with_state(
            &mut EinkScreen { panel: &mut panel },
            &cards,
            hopspot::BatteryState::Unknown,
            &ui_state,
        );
        let _ = epd.full_update(panel.buffer());

        let mut since_full = 0u32;
        loop {
            let event = BUTTON_EVENTS.receive().await;
            ui_state.handle_input(event, card_count);

            let _ = panel.clear(EpdColor::White);
            hopspot::draw_with_state(
                &mut EinkScreen { panel: &mut panel },
                &cards,
                hopspot::BatteryState::Unknown,
                &ui_state,
            );

            since_full += 1;
            if since_full >= FULL_REFRESH_INTERVAL {
                let _ = epd.full_update(panel.buffer());
                since_full = 0;
            } else {
                let _ = epd.partial_update(panel.buffer());
            }
        }
    };

    join4(usb_fut, heartbeat_fut, blink_fut, render_fut).await;
    loop {}
}
