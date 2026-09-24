use embassy_futures::join::join;
use embassy_futures::select::{select, select4, Either};
use embassy_nrf::gpio::Input;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};

use personal_hopspot_core::InputEvent;

const LONG_PRESS: Duration = Duration::from_millis(500);
const DEBOUNCE: Duration = Duration::from_millis(25);
pub(crate) const EVENT_CAPACITY: usize = 4;

pub(crate) static EVENTS: Channel<CriticalSectionRawMutex, InputEvent, EVENT_CAPACITY> =
    Channel::new();

/// The user button and the five-way joystick. Every contact is active-low.
///
/// The shared face is driven by one button: a short press advances and a long press selects. The
/// user button and the joystick centre both keep that short/long behaviour, while each joystick
/// direction is a short press so the stick can step through cards and menus.
pub(crate) struct WioInputs {
    pub(crate) button: Input<'static>,
    pub(crate) press: Input<'static>,
    pub(crate) up: Input<'static>,
    pub(crate) down: Input<'static>,
    pub(crate) left: Input<'static>,
    pub(crate) right: Input<'static>,
}

pub(crate) async fn drive(inputs: WioInputs) -> ! {
    let WioInputs {
        button,
        press,
        up,
        down,
        left,
        right,
    } = inputs;
    join(
        join(drive_press(button), drive_press(press)),
        drive_directions(up, down, left, right),
    )
    .await;
    unreachable!()
}

async fn drive_press(mut contact: Input<'static>) -> ! {
    loop {
        contact.wait_for_falling_edge().await;
        match select(contact.wait_for_rising_edge(), Timer::after(LONG_PRESS)).await {
            Either::First(()) => EVENTS.send(InputEvent::ShortPress).await,
            Either::Second(()) => {
                EVENTS.send(InputEvent::LongPress).await;
                contact.wait_for_rising_edge().await;
            }
        }
        Timer::after(DEBOUNCE).await;
    }
}

async fn drive_directions(
    mut up: Input<'static>,
    mut down: Input<'static>,
    mut left: Input<'static>,
    mut right: Input<'static>,
) -> ! {
    loop {
        select4(
            up.wait_for_falling_edge(),
            down.wait_for_falling_edge(),
            left.wait_for_falling_edge(),
            right.wait_for_falling_edge(),
        )
        .await;
        EVENTS.send(InputEvent::ShortPress).await;
        Timer::after(DEBOUNCE).await;
        // Wait for the stick to return to centre so one deflection is one step.
        while up.is_low() || down.is_low() || left.is_low() || right.is_low() {
            Timer::after(DEBOUNCE).await;
        }
    }
}
