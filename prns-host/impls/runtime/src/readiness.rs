//! Callback registration and runtime-independent asynchronous readiness.
//! Callbacks must be short, nonblocking signals and must not unregister themselves.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use crate::ReadinessSignal;
use tokio::sync::watch;

pub struct RegisteredReadiness {
    callback: ReadinessSignal,
    state: Mutex<RegisteredReadinessState>,
    idle: Condvar,
}

struct RegisteredReadinessState {
    active: bool,
    in_flight: usize,
}

impl RegisteredReadiness {
    fn new(callback: ReadinessSignal) -> Self {
        Self {
            callback,
            state: Mutex::new(RegisteredReadinessState {
                active: true,
                in_flight: 0,
            }),
            idle: Condvar::new(),
        }
    }

    fn notify(&self) {
        {
            let mut state = lock(&self.state);
            if !state.active {
                return;
            }
            state.in_flight = state.in_flight.saturating_add(1);
        }
        let _in_flight = InFlight(self);
        (self.callback)();
    }

    fn deactivate(&self) {
        let mut state = lock(&self.state);
        state.active = false;
        while state.in_flight > 0 {
            state = self
                .idle
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

pub struct Readiness {
    active: Mutex<Option<Arc<RegisteredReadiness>>>,
    changed: watch::Sender<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadinessAlreadyRegistered;

impl Readiness {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(None),
            changed: watch::channel(()).0,
        }
    }

    pub fn register(
        &self,
        callback: ReadinessSignal,
    ) -> Result<Arc<RegisteredReadiness>, ReadinessAlreadyRegistered> {
        let mut active = lock(&self.active);
        if active.is_some() {
            return Err(ReadinessAlreadyRegistered);
        }
        let registered = Arc::new(RegisteredReadiness::new(callback));
        *active = Some(Arc::clone(&registered));
        Ok(registered)
    }

    pub fn notify(&self) {
        self.changed.send_replace(());
        let registered = lock(&self.active).as_ref().map(Arc::clone);
        if let Some(registered) = registered {
            registered.notify();
        }
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<()> {
        self.changed.subscribe()
    }

    pub fn clear(&self) {
        let removed = lock(&self.active).take();
        if let Some(removed) = removed {
            removed.deactivate();
        }
    }

    pub fn unregister(&self, registered: &Arc<RegisteredReadiness>) {
        let removed = {
            let mut active = lock(&self.active);
            match active.as_ref() {
                Some(active_registration) if Arc::ptr_eq(active_registration, registered) => {
                    active.take()
                }
                Some(_) | None => None,
            }
        };
        if let Some(removed) = removed {
            removed.deactivate();
        } else {
            registered.deactivate();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Default for Readiness {
    fn default() -> Self {
        Self::new()
    }
}

struct InFlight<'a>(&'a RegisteredReadiness);
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.in_flight = state.in_flight.saturating_sub(1);
        if state.in_flight == 0 {
            self.0.idle.notify_all();
        }
    }
}
