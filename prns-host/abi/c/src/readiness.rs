use std::ffi::c_void;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

pub type ReadinessCallback = unsafe extern "C" fn(*mut c_void);

pub struct RegisteredReadiness {
    callback: ReadinessCallback,
    context: *mut c_void,
    state: Mutex<RegisteredReadinessState>,
    idle: Condvar,
}

struct RegisteredReadinessState {
    active: bool,
    in_flight: usize,
}

unsafe impl Send for RegisteredReadiness {}
unsafe impl Sync for RegisteredReadiness {}

impl RegisteredReadiness {
    fn new(callback: ReadinessCallback, context: *mut c_void) -> Self {
        Self {
            callback,
            context,
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
        unsafe {
            (self.callback)(self.context);
        }
        let mut state = lock(&self.state);
        state.in_flight = state.in_flight.saturating_sub(1);
        if state.in_flight == 0 {
            self.idle.notify_all();
        }
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
}

pub struct ReadinessAlreadyRegistered;

impl Readiness {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(None),
        }
    }

    pub fn register(
        &self,
        callback: ReadinessCallback,
        context: *mut c_void,
    ) -> Result<Arc<RegisteredReadiness>, ReadinessAlreadyRegistered> {
        let mut active = lock(&self.active);
        if active.is_some() {
            return Err(ReadinessAlreadyRegistered);
        }
        let registered = Arc::new(RegisteredReadiness::new(callback, context));
        *active = Some(Arc::clone(&registered));
        Ok(registered)
    }

    pub fn notify(&self) {
        let registered = lock(&self.active).as_ref().map(Arc::clone);
        if let Some(registered) = registered {
            registered.notify();
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
