use std::cell::RefCell;
use std::task::Waker;

use critical_section::Mutex;
use embassy_time_driver::Driver;
mod queue;
use queue::ObservedQueue;
pub(crate) use queue::QueueStats;

const TIMER_CAPACITY: usize = 1024;

struct State {
    now: u64,
    next: Option<u64>,
    queue: ObservedQueue<TIMER_CAPACITY>,
}

impl State {
    const fn new() -> Self {
        Self {
            now: 0,
            next: None,
            queue: ObservedQueue::new(),
        }
    }

    fn settle(&mut self) {
        let next = self.queue.next_expiration(self.now);
        self.next = (next != u64::MAX).then_some(next);
    }
}

struct SimulationClock(Mutex<RefCell<State>>);

embassy_time_driver::time_driver_impl!(static CLOCK: SimulationClock = SimulationClock(Mutex::new(RefCell::new(State::new()))));

impl Driver for SimulationClock {
    fn now(&self) -> u64 {
        critical_section::with(|cs| self.0.borrow_ref(cs).now)
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        critical_section::with(|cs| {
            let mut state = self.0.borrow_ref_mut(cs);
            assert_eq!(
                state.queue.schedule_wake(at, waker),
                Ok(()),
                "Embassy simulation timer admission"
            );
            state.settle();
        });
    }
}

pub(super) fn reset() {
    critical_section::with(|cs| *CLOCK.0.borrow_ref_mut(cs) = State::new());
}

pub(super) fn next_deadline() -> Option<u64> {
    critical_section::with(|cs| CLOCK.0.borrow_ref(cs).next)
}

pub(super) fn stats() -> QueueStats {
    critical_section::with(|cs| CLOCK.0.borrow_ref(cs).queue.stats())
}

pub(super) fn advance(micros: u64) {
    critical_section::with(|cs| {
        let mut state = CLOCK.0.borrow_ref_mut(cs);
        state.now = state.now.checked_add(micros).unwrap();
        state.settle();
    });
}
