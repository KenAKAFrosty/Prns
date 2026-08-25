use embassy_futures::select::{select, Either};
use embassy_nrf::gpio::Input;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};

use personal_hopspot_core::InputEvent;

const LONG_PRESS: Duration = Duration::from_millis(500);
const DEBOUNCE: Duration = Duration::from_millis(25);

pub(crate) static EVENTS: Channel<CriticalSectionRawMutex, InputEvent, 4> = Channel::new();

pub(crate) async fn drive(mut button: Input<'static>) -> ! {
    loop {
        button.wait_for_falling_edge().await;
        match select(button.wait_for_rising_edge(), Timer::after(LONG_PRESS)).await {
            Either::First(()) => EVENTS.send(InputEvent::ShortPress).await,
            Either::Second(()) => {
                EVENTS.send(InputEvent::LongPress).await;
                button.wait_for_rising_edge().await;
            }
        }
        Timer::after(DEBOUNCE).await;
    }
}
