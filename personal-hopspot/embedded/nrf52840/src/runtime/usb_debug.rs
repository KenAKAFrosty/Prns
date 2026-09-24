//! CDC ACM console that shares the WebUSB device. Lines stay in a RAM ring until a host opens
//! the serial port, and a full ring drops the oldest bytes. Nothing is written to flash.

use core::cell::RefCell;
use core::fmt::Write;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::Timer;
use embassy_usb::class::cdc_acm::CdcAcmClass;
use embassy_usb::driver::Driver;
use embassy_usb::UsbDevice;

// T114's default image only has ~1.5 KiB of static-RAM headroom above the
// minimum runtime stack. Keep the ring small enough that CDC + larger USB
// descriptor buffers still link; oldest lines drop when the host is slow.
const RING_BYTES: usize = 256;
const PACKET_BYTES: usize = 63;

struct Ring {
    bytes: [u8; RING_BYTES],
    head: usize,
    len: usize,
}

impl Ring {
    const fn new() -> Self {
        Self {
            bytes: [0; RING_BYTES],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, data: &[u8]) {
        for &byte in data {
            if self.len == RING_BYTES {
                self.head = (self.head + 1) % RING_BYTES;
                self.len -= 1;
            }
            let tail = (self.head + self.len) % RING_BYTES;
            self.bytes[tail] = byte;
            self.len += 1;
        }
    }

    fn take(&mut self, out: &mut [u8]) -> usize {
        let n = self.len.min(out.len());
        for slot in out.iter_mut().take(n) {
            *slot = self.bytes[self.head];
            self.head = (self.head + 1) % RING_BYTES;
        }
        self.len -= n;
        n
    }
}

static RING: Mutex<CriticalSectionRawMutex, RefCell<Ring>> = Mutex::new(RefCell::new(Ring::new()));

struct CdcLogger;

impl log::Log for CdcLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut line = heapless::String::<160>::new();
        let _ = write!(line, "[{}] {}\r\n", record.level(), record.args());
        RING.lock(|ring| ring.borrow_mut().push(line.as_bytes()));
    }

    fn flush(&self) {}
}

static LOGGER: CdcLogger = CdcLogger;

pub fn init() {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    log::info!("usb-debug: console armed");
}

pub async fn run<'d, D: Driver<'d>>(mut usb: UsbDevice<'d, D>, class: CdcAcmClass<'d, D>) {
    embassy_futures::join::join(usb.run(), drain(class)).await;
}

async fn drain<'d, D: Driver<'d>>(mut class: CdcAcmClass<'d, D>) {
    let mut packet = [0u8; PACKET_BYTES];
    loop {
        class.wait_connection().await;
        log::info!("usb-debug: host connected");
        loop {
            let n = RING.lock(|ring| ring.borrow_mut().take(&mut packet));
            if n == 0 {
                Timer::after_millis(50).await;
                continue;
            }
            if class.write_packet(&packet[..n]).await.is_err() {
                break;
            }
        }
    }
}
