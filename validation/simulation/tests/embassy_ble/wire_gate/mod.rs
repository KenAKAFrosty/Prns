use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, MutexGuard};

use personal_rns::wire::WirePacketHeader;
use tokio::sync::Notify;

mod backend;
#[cfg(test)]
mod tests;

pub(super) use backend::GatedBackend;

enum State {
    Idle,
    Armed {
        header: WirePacketHeader,
        remaining: NonZeroUsize,
    },
    Held(WirePacketHeader),
    Released,
    Losing {
        header: WirePacketHeader,
        passes: usize,
        dropped: usize,
        budget: NonZeroUsize,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum Disposition {
    Forward,
    Drop,
}

struct Inner {
    state: Mutex<State>,
    reached: Notify,
    released: Notify,
}

// One held send per radio, with no copied payload or queued observations. The
// sink retains its original frame; cancellation releases the gate's ownership.
#[derive(Clone)]
pub(super) struct WireGate(Arc<Inner>);

impl WireGate {
    pub(super) fn new() -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State::Idle),
            reached: Notify::new(),
            released: Notify::new(),
        }))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap()
    }

    pub(super) fn arm(&self, header: WirePacketHeader, occurrence: NonZeroUsize) {
        let mut state = self.state();
        assert!(
            matches!(*state, State::Idle),
            "one armed wire gate per radio"
        );
        *state = State::Armed {
            header,
            remaining: occurrence,
        };
    }

    pub(super) fn is_idle(&self) -> bool {
        matches!(*self.state(), State::Idle)
    }

    pub(super) fn lose_after(&self, header: WirePacketHeader, passes: usize, budget: NonZeroUsize) {
        let mut state = self.state();
        assert!(
            matches!(*state, State::Idle),
            "one armed wire gate per radio"
        );
        *state = State::Losing {
            header,
            passes,
            dropped: 0,
            budget,
        };
    }

    pub(super) fn stop_loss(&self) -> usize {
        let mut state = self.state();
        let State::Losing { dropped, .. } = *state else {
            unreachable!("stop only an active loss rule");
        };
        *state = State::Idle;
        dropped
    }

    fn discard(&self, frame: &[u8]) -> Disposition {
        let Ok((observed, _)) = WirePacketHeader::parse(frame) else {
            return Disposition::Forward;
        };
        let mut state = self.state();
        let State::Losing {
            header,
            passes,
            dropped,
            budget,
        } = &mut *state
        else {
            return Disposition::Forward;
        };
        if observed != *header {
            return Disposition::Forward;
        }
        if *passes > 0 {
            *passes -= 1;
            return Disposition::Forward;
        }
        assert!(*dropped < budget.get(), "wire loss budget exhausted");
        *dropped += 1;
        self.0.reached.notify_one();
        Disposition::Drop
    }

    pub(super) async fn first_loss(&self) -> WirePacketHeader {
        loop {
            let reached = self.0.reached.notified();
            match *self.state() {
                State::Losing {
                    header, dropped, ..
                } if dropped > 0 => return header,
                State::Losing { .. } => {}
                _ => unreachable!("observe loss only while its rule is active"),
            }
            reached.await;
        }
    }

    pub(super) async fn held(&self) -> WirePacketHeader {
        loop {
            let reached = self.0.reached.notified();
            match *self.state() {
                State::Held(header) => return header,
                State::Idle | State::Armed { .. } => {}
                State::Released => unreachable!("observe a held frame before releasing it"),
                State::Losing { .. } => unreachable!("loss rules do not hold frames"),
            }
            reached.await;
        }
    }

    pub(super) fn release(&self) {
        let mut state = self.state();
        assert!(
            matches!(*state, State::Held(_)),
            "release only a held frame"
        );
        *state = State::Released;
        self.0.released.notify_one();
    }

    fn hold(&self, frame: &[u8]) -> Option<HeldSend<'_>> {
        let (observed, _) = WirePacketHeader::parse(frame).ok()?;
        let mut state = self.state();
        let State::Armed { header, remaining } = &mut *state else {
            return None;
        };
        if observed != *header {
            return None;
        }
        if let Some(next) = NonZeroUsize::new(remaining.get() - 1) {
            *remaining = next;
            return None;
        }
        *state = State::Held(observed);
        self.0.reached.notify_one();
        Some(HeldSend(self))
    }

    async fn before_send(&self, frame: &[u8]) -> Disposition {
        if self.discard(frame) == Disposition::Drop {
            return Disposition::Drop;
        }
        let Some(_held) = self.hold(frame) else {
            return Disposition::Forward;
        };
        loop {
            let released = self.0.released.notified();
            if matches!(*self.state(), State::Released) {
                return Disposition::Forward;
            }
            released.await;
        }
    }
}

struct HeldSend<'a>(&'a WireGate);

impl Drop for HeldSend<'_> {
    fn drop(&mut self) {
        *self.0.state() = State::Idle;
    }
}
