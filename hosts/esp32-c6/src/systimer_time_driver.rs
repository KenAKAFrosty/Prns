//! An `embassy-time` timebase backed by the ESP32-C6 SystemTimer.
//!
//! The async Runtime (`spike_c_async`) runs on an embassy executor, which needs
//! a real timebase: `Timer::after(..)` only resolves if some driver wakes the
//! task when its deadline passes. The pre-built
//! `esp-hal-embassy` driver isn't usable on our pinned 1.1.x esp-* stack (it
//! requires esp-hal's internal `__esp_hal_embassy` feature), so we wire the
//! ~standard ~80-line driver ourselves: `embassy_time_driver::Driver` over one
//! SystemTimer comparator (`alarm0`) plus the self-contained generic timer
//! queue from `embassy-time-queue-utils`.
//!
//! Timebase is microseconds (`tick-hz-1_000_000`): `esp_hal::time::Instant` is
//! already a microsecond `fugit` instant reading the same SystemTimer unit the
//! comparator counts against, so `now()` and the alarm share one clock with no
//! conversion. Deadlines are armed *relative* to the comparator's live count
//! (`Alarm::load_value`), which means the target is always in the future — there
//! is no already-elapsed-target race even if the `now` read is slightly stale.

use core::cell::RefCell;
use core::task::Waker;

use critical_section::{CriticalSection, Mutex};
use embassy_time_driver::Driver;
use embassy_time_queue_utils::Queue;
use esp_hal::handler;
use esp_hal::time::{Duration, Instant};
use esp_hal::timer::systimer::Alarm;
use esp_hal::timer::Timer;

/// Cap on a single comparator arming, well inside the 52-bit / 16 MHz ceiling
/// (~285 years). A deadline farther out than this simply fires early and
/// re-arms; the queue re-offers it until it's within range. In practice every
/// spike deadline is sub-second, so the cap never bites.
const MAX_ALARM_DELAY_US: u64 = 60 * 60 * 1_000_000;

/// Microseconds since boot — the embassy timebase. A direct read of the
/// SystemTimer unit the comparator also counts against.
fn now_micros() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}

struct SystemTimerDriver {
    /// Pending timers, woken as their deadlines pass. The generic
    /// `embassy-time` queue (capacity 64): self-contained, so this driver owns
    /// timekeeping outright with no dependence on the executor's integrated
    /// per-task timer-queue items.
    queue: Mutex<RefCell<Queue>>,
    /// The comparator the next deadline fires on. `None` until [`init`] hands
    /// over `alarm0` during boot.
    alarm: Mutex<RefCell<Option<Alarm<'static>>>>,
}

impl SystemTimerDriver {
    const fn new() -> Self {
        Self {
            queue: Mutex::new(RefCell::new(Queue::new())),
            alarm: Mutex::new(RefCell::new(None)),
        }
    }

    /// Program the comparator to fire at absolute microsecond `at`, or disable
    /// it when nothing is pending (`u64::MAX`). Returns `false` when `at` has
    /// already passed, signalling the caller to drain the queue again and retry
    /// with the next deadline.
    fn arm(&self, cs: CriticalSection<'_>, at: u64) -> bool {
        let alarm_cell = self.alarm.borrow(cs).borrow();
        let Some(alarm) = alarm_cell.as_ref() else {
            // Pre-`init`: no comparator yet, nothing to arm.
            return true;
        };

        if at == u64::MAX {
            alarm.enable_interrupt(false);
            alarm.stop();
            return true;
        }

        let now = now_micros();
        if at <= now {
            return false;
        }

        let delay = (at - now).min(MAX_ALARM_DELAY_US);
        alarm.clear_interrupt();
        // Relative target (count + delay) is always future; the cap keeps it
        // inside the comparator's range, so this never errors.
        let _ = alarm.load_value(Duration::from_micros(delay));
        alarm.enable_interrupt(true);
        alarm.start();
        true
    }
}

impl Driver for SystemTimerDriver {
    fn now(&self) -> u64 {
        now_micros()
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        critical_section::with(|cs| {
            let mut queue = self.queue.borrow(cs).borrow_mut();
            if queue.schedule_wake(at, waker) {
                let mut next = queue.next_expiration(now_micros());
                while !self.arm(cs, next) {
                    next = queue.next_expiration(now_micros());
                }
            }
        });
    }
}

embassy_time_driver::time_driver_impl!(static DRIVER: SystemTimerDriver = SystemTimerDriver::new());

/// SystemTimer comparator interrupt: a scheduled deadline elapsed. Acknowledge
/// the comparator, wake every timer now due, and re-arm for the next one.
#[handler]
fn systimer_alarm_isr() {
    critical_section::with(|cs| {
        if let Some(alarm) = DRIVER.alarm.borrow(cs).borrow().as_ref() {
            alarm.clear_interrupt();
            alarm.enable_interrupt(false);
            alarm.stop();
        }

        let mut queue = DRIVER.queue.borrow(cs).borrow_mut();
        let mut next = queue.next_expiration(now_micros());
        while !DRIVER.arm(cs, next) {
            next = queue.next_expiration(now_micros());
        }
    });
}

/// Hand the driver the SystemTimer comparator it fires deadlines on, and bind
/// its interrupt. Call once during boot, before starting the executor.
pub fn init(alarm: Alarm<'static>) {
    alarm.set_interrupt_handler(systimer_alarm_isr);
    alarm.enable_interrupt(false);
    critical_section::with(|cs| {
        DRIVER.alarm.borrow(cs).replace(Some(alarm));
    });
}
