use core::sync::atomic::{AtomicU32, Ordering};

use embassy_futures::select::{select, Either};
use embassy_nrf::gpio::{Input, Output};
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};

use personal_hopspot_core::InputEvent;

use crate::runtime::node::Mtx;

const BUTTON_LONG_PRESS: Duration = Duration::from_millis(500);
const BUTTON_DEBOUNCE: Duration = Duration::from_millis(25);
const FRONTLIGHT_HOLD: Duration = Duration::from_secs(8);

pub(crate) const EVENT_CAPACITY: usize = 4;
pub(crate) static EVENTS: Channel<Mtx, InputEvent, EVENT_CAPACITY> = Channel::new();
static BUTTON_COUNT: AtomicU32 = AtomicU32::new(0);
static FRONTLIGHT_WAKE: Signal<Mtx, ()> = Signal::new();

pub(crate) async fn drive_button(mut button: Input<'static>) -> ! {
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
                EVENTS.send(InputEvent::ShortPress).await;
            }
            Either::Second(()) => {
                BUTTON_COUNT.fetch_add(1, Ordering::Relaxed);
                EVENTS.send(InputEvent::LongPress).await;
                button.wait_for_rising_edge().await;
            }
        }
        Timer::after(BUTTON_DEBOUNCE).await;
    }
}

pub(crate) async fn drive_frontlight(mut frontlight: Output<'static>) -> ! {
    loop {
        FRONTLIGHT_WAKE.wait().await;
        frontlight.set_high();
        while let Either::First(()) =
            select(FRONTLIGHT_WAKE.wait(), Timer::after(FRONTLIGHT_HOLD)).await
        {}
        frontlight.set_low();
    }
}
