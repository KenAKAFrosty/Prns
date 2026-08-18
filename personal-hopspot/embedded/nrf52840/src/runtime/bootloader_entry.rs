use personal_rns::usb_auto::WebUsbBootloaderEntry;

#[cfg(feature = "board-t1000e")]
mod selected {
    use core::sync::atomic::{AtomicBool, Ordering};

    use embassy_time::{Duration, Timer};

    const REQUEST_POLL_INTERVAL: Duration = Duration::from_millis(25);
    const CONTROL_RESPONSE_GRACE_PERIOD: Duration = Duration::from_millis(100);
    const ADAFRUIT_SERIAL_ONLY_DFU_GPREGRET: u8 = 0x4e;

    static REQUESTED: AtomicBool = AtomicBool::new(false);

    pub fn request() {
        REQUESTED.store(true, Ordering::SeqCst);
    }

    pub async fn wait() -> ! {
        loop {
            if REQUESTED.swap(false, Ordering::SeqCst) {
                Timer::after(CONTROL_RESPONSE_GRACE_PERIOD).await;
                embassy_nrf::pac::POWER
                    .gpregret()
                    .write(|register| register.set_gpregret(ADAFRUIT_SERIAL_ONLY_DFU_GPREGRET));
                cortex_m::peripheral::SCB::sys_reset();
            }
            Timer::after(REQUEST_POLL_INTERVAL).await;
        }
    }
}

pub const fn webusb_entry() -> WebUsbBootloaderEntry {
    #[cfg(feature = "board-t1000e")]
    return WebUsbBootloaderEntry::Supported {
        request: selected::request,
    };

    #[cfg(not(feature = "board-t1000e"))]
    WebUsbBootloaderEntry::Unsupported
}

pub async fn wait() -> ! {
    #[cfg(feature = "board-t1000e")]
    selected::wait().await;

    #[cfg(not(feature = "board-t1000e"))]
    core::future::pending().await
}
