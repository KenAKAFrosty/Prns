use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use esp_hal::gpio::{Level, Output, OutputConfig, OutputPin};
use personal_rns::radios::sx126x::{FrontendControl, RadioActivityControl};

struct WioPins {
    receive_enable: Output<'static>,
    activity_led: Output<'static>,
}

static PINS: Mutex<CriticalSectionRawMutex, RefCell<Option<WioPins>>> =
    Mutex::new(RefCell::new(None));

pub(super) static RADIO_ACTIVITY: RadioActivityControl = RadioActivityControl::TxRx {
    enter_transmit: activity_started,
    leave_transmit: activity_finished,
    enter_receive: activity_started,
    leave_receive: activity_finished,
};

pub(super) fn initialize(
    receive_enable_pin: impl OutputPin + 'static,
    activity_led_pin: impl OutputPin + 'static,
) -> FrontendControl {
    PINS.lock(|pins| {
        *pins.borrow_mut() = Some(WioPins {
            receive_enable: Output::new(receive_enable_pin, Level::High, OutputConfig::default()),
            activity_led: Output::new(activity_led_pin, Level::Low, OutputConfig::default()),
        });
    });
    FrontendControl::TxRx {
        enter_transmit,
        enter_receive,
    }
}

fn enter_transmit() {
    PINS.lock(|pins| {
        if let Some(pins) = pins.borrow_mut().as_mut() {
            pins.receive_enable.set_low();
        }
    });
}

fn enter_receive() {
    PINS.lock(|pins| {
        if let Some(pins) = pins.borrow_mut().as_mut() {
            pins.receive_enable.set_high();
        }
    });
}

fn activity_started() {
    PINS.lock(|pins| {
        if let Some(pins) = pins.borrow_mut().as_mut() {
            pins.activity_led.set_high();
        }
    });
}

fn activity_finished() {
    PINS.lock(|pins| {
        if let Some(pins) = pins.borrow_mut().as_mut() {
            pins.activity_led.set_low();
        }
    });
}
