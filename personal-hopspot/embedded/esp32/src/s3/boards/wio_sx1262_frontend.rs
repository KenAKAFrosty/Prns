use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use esp_hal::gpio::{Level, Output, OutputConfig, OutputPin};
use personal_rns::radios::sx126x::FrontendControl;

/// Hold the Wio receive-enable pin outside the board future so the radio driver's synchronous
/// mode callbacks can change it at every TX/RX transition.
static RX_ENABLE: Mutex<CriticalSectionRawMutex, RefCell<Option<Output<'static>>>> =
    Mutex::new(RefCell::new(None));

pub(super) fn initialize(pin: impl OutputPin + 'static) -> FrontendControl {
    let receive_enable = Output::new(pin, Level::High, OutputConfig::default());
    RX_ENABLE.lock(|held| {
        held.replace(Some(receive_enable));
    });
    FrontendControl::TxRx {
        enter_transmit,
        enter_receive,
    }
}

fn enter_transmit() {
    RX_ENABLE.lock(|held| {
        if let Some(receive_enable) = held.borrow_mut().as_mut() {
            receive_enable.set_low();
        }
    });
}

fn enter_receive() {
    RX_ENABLE.lock(|held| {
        if let Some(receive_enable) = held.borrow_mut().as_mut() {
            receive_enable.set_high();
        }
    });
}
