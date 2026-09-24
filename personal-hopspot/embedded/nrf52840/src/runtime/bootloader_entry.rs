use personal_rns::usb_auto::WebUsbBootloaderEntry;
#[cfg(any(feature = "board-t1000e", feature = "board-sensecap-solar-node"))]
use personal_rns::usb_auto::WebUsbBootloaderMode;

#[cfg(any(feature = "board-t1000e", feature = "board-sensecap-solar-node"))]
pub(crate) fn enter_bare_metal_bootloader(mode: WebUsbBootloaderMode) -> ! {
    #[cfg(feature = "board-t1000e")]
    const ADAFRUIT_SERIAL_ONLY_DFU_GPREGRET: u8 = 0x4e;
    const ADAFRUIT_UF2_DFU_GPREGRET: u8 = 0x57;
    let magic = match mode {
        #[cfg(feature = "board-t1000e")]
        WebUsbBootloaderMode::PrnsFlasher => ADAFRUIT_SERIAL_ONLY_DFU_GPREGRET,
        #[cfg(feature = "board-sensecap-solar-node")]
        WebUsbBootloaderMode::PrnsFlasher => ADAFRUIT_UF2_DFU_GPREGRET,
        WebUsbBootloaderMode::Uf2HandOff => ADAFRUIT_UF2_DFU_GPREGRET,
    };
    embassy_nrf::pac::POWER
        .gpregret()
        .write(|register| register.set_gpregret(magic));
    cortex_m::peripheral::SCB::sys_reset()
}

#[cfg(any(
    feature = "board-t096",
    feature = "board-wio-tracker-l1",
    feature = "board-t1000e",
    feature = "board-sensecap-solar-node",
    feature = "board-mesh-pocket",
    feature = "board-muzi-base-duo",
    any(feature = "board-rak4631", feature = "board-rak10724")
))]
mod request {
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::signal::Signal;
    use embassy_time::{Duration, Timer};
    use personal_rns::usb_auto::WebUsbBootloaderMode;

    const CONTROL_RESPONSE_GRACE_PERIOD: Duration = Duration::from_millis(100);

    static REQUESTED: Signal<CriticalSectionRawMutex, WebUsbBootloaderMode> = Signal::new();

    pub fn request(mode: WebUsbBootloaderMode) {
        REQUESTED.signal(mode);
    }

    #[cfg(any(feature = "board-t1000e", feature = "board-sensecap-solar-node"))]
    pub async fn wait() -> ! {
        let mode = REQUESTED.wait().await;
        Timer::after(CONTROL_RESPONSE_GRACE_PERIOD).await;
        super::enter_bare_metal_bootloader(mode)
    }

    #[cfg(not(any(feature = "board-t1000e", feature = "board-sensecap-solar-node")))]
    pub async fn wait() -> ! {
        loop {
            let _mode = REQUESTED.wait().await;
            Timer::after(CONTROL_RESPONSE_GRACE_PERIOD).await;
            if prepare_bootloader_reset().is_ok() {
                cortex_m::peripheral::SCB::sys_reset();
            }
        }
    }

    #[cfg(not(any(feature = "board-t1000e", feature = "board-sensecap-solar-node")))]
    fn prepare_bootloader_reset() -> Result<(), nrf_softdevice::RawError> {
        const ADAFRUIT_UF2_DFU_GPREGRET: u32 = 0x57;
        // SAFETY: The enabled S140 SoftDevice owns POWER. This synchronous SVC is the Nordic API
        // for setting GPREGRET while the SoftDevice is active; register 0 and the one-byte UF2
        // bootloader request are valid inputs.
        let result =
            unsafe { nrf_softdevice::raw::sd_power_gpregret_set(0, ADAFRUIT_UF2_DFU_GPREGRET) };
        nrf_softdevice::RawError::convert(result)
    }
}

pub const fn webusb_entry() -> WebUsbBootloaderEntry {
    #[cfg(any(
        feature = "board-t096",
        feature = "board-wio-tracker-l1",
        feature = "board-t1000e",
        feature = "board-sensecap-solar-node",
        feature = "board-mesh-pocket",
        feature = "board-muzi-base-duo",
        any(feature = "board-rak4631", feature = "board-rak10724")
    ))]
    return WebUsbBootloaderEntry::Supported {
        request: request::request,
    };

    #[cfg(not(any(
        feature = "board-t096",
        feature = "board-wio-tracker-l1",
        feature = "board-t1000e",
        feature = "board-sensecap-solar-node",
        feature = "board-mesh-pocket",
        feature = "board-muzi-base-duo",
        any(feature = "board-rak4631", feature = "board-rak10724")
    )))]
    WebUsbBootloaderEntry::Unsupported
}

pub async fn wait() -> ! {
    #[cfg(any(
        feature = "board-t096",
        feature = "board-wio-tracker-l1",
        feature = "board-t1000e",
        feature = "board-sensecap-solar-node",
        feature = "board-mesh-pocket",
        feature = "board-muzi-base-duo",
        any(feature = "board-rak4631", feature = "board-rak10724")
    ))]
    request::wait().await;

    #[cfg(not(any(
        feature = "board-t096",
        feature = "board-wio-tracker-l1",
        feature = "board-t1000e",
        feature = "board-sensecap-solar-node",
        feature = "board-mesh-pocket",
        feature = "board-muzi-base-duo",
        any(feature = "board-rak4631", feature = "board-rak10724")
    )))]
    core::future::pending().await
}
