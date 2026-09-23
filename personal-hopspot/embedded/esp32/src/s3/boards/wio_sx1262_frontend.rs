use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::{Level, Output, OutputConfig, OutputPin};
use personal_rns::radios::sx126x::FrontendControl;

/// How long the activity LED stays lit after a decoded receive frame. A transmit holds the LED
/// lit for its whole airtime instead: [`enter_receive`] clears it when the radio returns to
/// listening.
const RX_ACTIVITY_HOLD: Duration = Duration::from_millis(60);

/// Poll interval for [`activity_led_task`]. Short enough that a single frame produces a visible
/// blink, long enough to be negligible on the radio core.
const ACTIVITY_SERVICE_INTERVAL: Duration = Duration::from_millis(20);

/// Hold the Wio receive-enable pin outside the board future so the radio driver's synchronous
/// mode callbacks can change it at every TX/RX transition.
static RX_ENABLE: Mutex<CriticalSectionRawMutex, RefCell<Option<Output<'static>>>> =
    Mutex::new(RefCell::new(None));

/// The LoRa activity LED plus, when a receive pulse is in flight, the instant to extinguish it.
/// The receive-enable pin and the LED live together because both are wired to the driver's
/// TX/RX callbacks.
static ACTIVITY: Mutex<CriticalSectionRawMutex, RefCell<Option<ActivityLed>>> =
    Mutex::new(RefCell::new(None));

struct ActivityLed {
    led: Output<'static>,
    /// `None` while the LED is held on for a whole transmit; `Some` is a receive-pulse deadline.
    until: Option<Instant>,
}

pub(super) fn initialize(
    receive_enable_pin: impl OutputPin + 'static,
    activity_led_pin: impl OutputPin + 'static,
) -> FrontendControl {
    let receive_enable = Output::new(receive_enable_pin, Level::High, OutputConfig::default());
    RX_ENABLE.lock(|held| {
        held.replace(Some(receive_enable));
    });
    let led = Output::new(activity_led_pin, Level::Low, OutputConfig::default());
    ACTIVITY.lock(|held| {
        *held.borrow_mut() = Some(ActivityLed { led, until: None });
    });
    FrontendControl::TxRx {
        enter_transmit,
        enter_receive,
        on_frame_received: Some(on_frame_received),
    }
}

fn enter_transmit() {
    RX_ENABLE.lock(|held| {
        if let Some(receive_enable) = held.borrow_mut().as_mut() {
            receive_enable.set_low();
        }
    });
    ACTIVITY.lock(|held| {
        if let Some(activity) = held.borrow_mut().as_mut() {
            activity.led.set_high();
            activity.until = None;
        }
    });
}

fn enter_receive() {
    RX_ENABLE.lock(|held| {
        if let Some(receive_enable) = held.borrow_mut().as_mut() {
            receive_enable.set_high();
        }
    });
    ACTIVITY.lock(|held| {
        if let Some(activity) = held.borrow_mut().as_mut() {
            activity.led.set_low();
            activity.until = None;
        }
    });
}

fn on_frame_received() {
    ACTIVITY.lock(|held| {
        if let Some(activity) = held.borrow_mut().as_mut() {
            activity.led.set_high();
            activity.until = Some(Instant::now() + RX_ACTIVITY_HOLD);
        }
    });
}

/// Extinguish the LED once a receive pulse expires. Spawned by the board so the synchronous
/// driver callbacks never have to wait.
#[embassy_executor::task]
pub(super) async fn activity_led_task() {
    loop {
        Timer::after(ACTIVITY_SERVICE_INTERVAL).await;
        let now = Instant::now();
        ACTIVITY.lock(|held| {
            let mut held = held.borrow_mut();
            let Some(activity) = held.as_mut() else {
                return;
            };
            let Some(until) = activity.until else {
                return;
            };
            if now >= until {
                activity.led.set_low();
                activity.until = None;
            }
        });
    }
}
