use personal_rns::usb_auto::WebUsbBootloaderEntry;

#[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
mod request {
    use core::sync::atomic::{AtomicBool, Ordering};

    use embassy_time::{Duration, Timer};

    const REQUEST_POLL_INTERVAL: Duration = Duration::from_millis(25);
    const CONTROL_RESPONSE_GRACE_PERIOD: Duration = Duration::from_millis(100);

    enum ResetPreparation {
        Ready,
        #[cfg(feature = "board-t096")]
        Rejected,
    }

    static REQUESTED: AtomicBool = AtomicBool::new(false);

    pub fn request() {
        REQUESTED.store(true, Ordering::SeqCst);
    }

    pub async fn wait() -> ! {
        loop {
            if REQUESTED.swap(false, Ordering::SeqCst) {
                Timer::after(CONTROL_RESPONSE_GRACE_PERIOD).await;
                match prepare_bootloader_reset() {
                    ResetPreparation::Ready => cortex_m::peripheral::SCB::sys_reset(),
                    #[cfg(feature = "board-t096")]
                    ResetPreparation::Rejected => {}
                }
            }
            Timer::after(REQUEST_POLL_INTERVAL).await;
        }
    }

    #[cfg(feature = "board-t1000e")]
    fn prepare_bootloader_reset() -> ResetPreparation {
        const ADAFRUIT_SERIAL_ONLY_DFU_GPREGRET: u8 = 0x4e;
        embassy_nrf::pac::POWER
            .gpregret()
            .write(|register| register.set_gpregret(ADAFRUIT_SERIAL_ONLY_DFU_GPREGRET));
        ResetPreparation::Ready
    }

    #[cfg(feature = "board-t096")]
    fn prepare_bootloader_reset() -> ResetPreparation {
        const ADAFRUIT_UF2_DFU_GPREGRET: u32 = 0x57;
        // SAFETY: S140 is enabled on T096 and owns POWER. This synchronous SVC is the Nordic API
        // for setting GPREGRET while the SoftDevice is active; register 0 and the one-byte UF2
        // bootloader request are valid inputs.
        let result =
            unsafe { nrf_softdevice::raw::sd_power_gpregret_set(0, ADAFRUIT_UF2_DFU_GPREGRET) };
        match nrf_softdevice::RawError::convert(result) {
            Ok(()) => ResetPreparation::Ready,
            Err(_) => ResetPreparation::Rejected,
        }
    }
}

pub const fn webusb_entry() -> WebUsbBootloaderEntry {
    #[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
    return WebUsbBootloaderEntry::Supported {
        request: request::request,
    };

    #[cfg(not(any(feature = "board-t096", feature = "board-t1000e")))]
    WebUsbBootloaderEntry::Unsupported
}

pub async fn wait() -> ! {
    #[cfg(any(feature = "board-t096", feature = "board-t1000e"))]
    request::wait().await;

    #[cfg(not(any(feature = "board-t096", feature = "board-t1000e")))]
    core::future::pending().await
}
