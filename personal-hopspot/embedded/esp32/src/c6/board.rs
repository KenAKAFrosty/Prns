use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::interrupt::software::SoftwareInterruptControl;
#[cfg(feature = "bluetooth-auto")]
use esp_hal::peripherals::BT;
#[cfg(feature = "esp-now")]
use esp_hal::peripherals::WIFI;
use esp_hal::rng::TrngSource;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use personal_hopspot_memory::{MemoryProfile, XIAO_ESP32_C6};
use personal_rns::engine::InstantMillis;
use personal_rns::interfaces::InterfaceId;
use personal_rns::manifold::embassy::EmbassyTimebase;

pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x13Personal Hopspot C6\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot C6";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"hopsp-c6");
pub(crate) const MEMORY_PROFILE: &MemoryProfile = &XIAO_ESP32_C6;

// Keep the existing per-radio heap budgets, placing up to 64 KiB in RAM released
// by the ESP-IDF bootloader. The linker owns that separate bank; this does not
// enlarge the heap or borrow from the CPU0 stack used by the Embassy executor.
#[cfg(not(any(feature = "bluetooth-auto", feature = "esp-now")))]
const HEAP_BYTES: usize = 32 * 1024;
#[cfg(all(feature = "bluetooth-auto", not(feature = "esp-now")))]
const HEAP_BYTES: usize = 64 * 1024;
#[cfg(all(feature = "esp-now", not(feature = "bluetooth-auto")))]
const HEAP_BYTES: usize = 72 * 1024;
#[cfg(all(feature = "esp-now", feature = "bluetooth-auto"))]
const HEAP_BYTES: usize = 88 * 1024;

const RECLAIMED_HEAP_BYTES: usize = if HEAP_BYTES < 64 * 1024 {
    HEAP_BYTES
} else {
    64 * 1024
};
#[cfg(feature = "esp-now")]
const INTERNAL_HEAP_BYTES: usize = HEAP_BYTES - RECLAIMED_HEAP_BYTES;

pub(crate) struct C6Hardware {
    pub(crate) usb_rx: UsbSerialJtagRx<'static, Async>,
    pub(crate) usb_tx: UsbSerialJtagTx<'static, Async>,
    #[cfg(feature = "esp-now")]
    pub(crate) wifi: WIFI<'static>,
    #[cfg(feature = "bluetooth-auto")]
    pub(crate) bluetooth: BT<'static>,
    pub(crate) identity_entropy: TrngSource<'static>,
    pub(crate) mac: [u8; 6],
    pub(crate) timebase: EmbassyTimebase,
    pub(crate) _rtc: Rtc<'static>,
}

pub(crate) struct XiaoEsp32C6;

impl XiaoEsp32C6 {
    pub(crate) fn bringup() -> C6Hardware {
        esp_println::logger::init_logger_from_env();
        esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: RECLAIMED_HEAP_BYTES);
        // The 32/64 KiB configurations fit entirely in the reclaimed bank; do
        // not register a zero-sized allocator region for those combinations.
        #[cfg(feature = "esp-now")]
        esp_alloc::heap_allocator!(size: INTERNAL_HEAP_BYTES);
        esp_println::println!("XIAO ESP32-C6 boot {}", env!("HOPSPOT_BUILD_IDENTITY"));

        let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
        let peripherals = esp_hal::init(config);
        let (usb_rx, usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE)
            .into_async()
            .split();

        let timer_group = TimerGroup::new(peripherals.TIMG0);
        let software_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
        esp_rtos::start(timer_group.timer0, software_interrupt.software_interrupt0);

        let mut rtc = Rtc::new(peripherals.LPWR);
        rtc.rwdt.disable();
        rtc.swd.disable();
        let timebase = EmbassyTimebase::start_at(InstantMillis(rtc.current_time_us() / 1000));
        let identity_entropy = TrngSource::new(peripherals.RNG, peripherals.ADC1);

        let base_mac = base_mac_address();
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&base_mac.as_bytes()[..6]);

        C6Hardware {
            usb_rx,
            usb_tx,
            #[cfg(feature = "esp-now")]
            wifi: peripherals.WIFI,
            #[cfg(feature = "bluetooth-auto")]
            bluetooth: peripherals.BT,
            identity_entropy,
            mac,
            timebase,
            _rtc: rtc,
        }
    }
}
