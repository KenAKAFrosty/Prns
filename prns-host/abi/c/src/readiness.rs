use std::ffi::c_void;
use std::sync::Arc;

pub use prns_host_native::{Readiness, RegisteredReadiness};
pub type ReadinessCallback = unsafe extern "C" fn(*mut c_void);

struct CallbackContext {
    callback: ReadinessCallback,
    context: *mut c_void,
}

// The C registration contract requires this context to remain valid and the
// callback to be safe on the producer thread until unregister returns.
unsafe impl Send for CallbackContext {}
unsafe impl Sync for CallbackContext {}

impl CallbackContext {
    fn notify(&self) {
        // SAFETY: The caller's registration contract keeps the context alive;
        // shared readiness quiesces in-flight calls before unregister returns.
        unsafe { (self.callback)(self.context) }
    }
}

pub fn callback_signal(
    callback: ReadinessCallback,
    context: *mut c_void,
) -> prns_host_native::ReadinessSignal {
    let context = CallbackContext { callback, context };
    Arc::new(move || context.notify())
}
