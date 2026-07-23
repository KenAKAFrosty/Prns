use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::interrupt::software::SoftwareInterruptControl;
#[cfg(feature = "bluetooth-auto")]
use esp_hal::peripherals::BT;
#[cfg(feature = "esp-now")]
use esp_hal::peripherals::WIFI;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use personal_rns::engine::InstantMillis;
use personal_rns::interfaces::InterfaceId;
use personal_rns::reactor::embassy::EmbassyTimebase;

pub(crate) const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x13Personal Hopspot C6\xc0";
pub(crate) const NODE_ANNOUNCE_APP_DATA: &[u8] = b"Personal Hopspot C6";
pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"hopsp-c6");

// BLE needs heap for esp-radio's controller + trouble-host's boxed GATT clients/reassemblers; 64 KB
// covers it with margin. Kept off the larger end so the leftover linker `.stack` region stays big
// enough for the BLE construction transient (the single-core main task runs on `.stack` — esp-rtos
// gives it no separate task stack, so RAM spent on the heap is RAM taken from that one stack).
#[cfg(not(any(feature = "bluetooth-auto", feature = "esp-now")))]
const HEAP_BYTES: usize = 32 * 1024;
#[cfg(all(feature = "bluetooth-auto", not(feature = "esp-now")))]
const HEAP_BYTES: usize = 64 * 1024;
#[cfg(all(feature = "esp-now", not(feature = "bluetooth-auto")))]
const HEAP_BYTES: usize = 72 * 1024;
#[cfg(all(feature = "esp-now", feature = "bluetooth-auto"))]
const HEAP_BYTES: usize = 88 * 1024;

pub(crate) struct C6Hardware {
    pub(crate) usb_rx: UsbSerialJtagRx<'static, Async>,
    pub(crate) usb_tx: UsbSerialJtagTx<'static, Async>,
    #[cfg(feature = "esp-now")]
    pub(crate) wifi: WIFI<'static>,
    #[cfg(feature = "bluetooth-auto")]
    pub(crate) bluetooth: BT<'static>,
    pub(crate) mac: [u8; 6],
    pub(crate) timebase: EmbassyTimebase,
    pub(crate) _rtc: Rtc<'static>,
}

pub(crate) struct XiaoEsp32C6;

impl XiaoEsp32C6 {
    pub(crate) fn bringup() -> C6Hardware {
        esp_println::logger::init_logger_from_env();
        esp_alloc::heap_allocator!(size: HEAP_BYTES);

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
            mac,
            timebase,
            _rtc: rtc,
        }
    }
}
