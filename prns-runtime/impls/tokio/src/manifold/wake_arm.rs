use std::sync::atomic::{fence, AtomicBool, Ordering};

pub(super) struct WakeArm(AtomicBool);

impl WakeArm {
    pub(super) fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub(super) fn arm_before_recheck(&self) {
        self.0.store(true, Ordering::Release);
        fence(Ordering::SeqCst);
    }

    pub(super) fn disarm(&self) {
        self.0.store(false, Ordering::Release);
    }

    pub(super) fn take_after_publish(&self) -> bool {
        fence(Ordering::SeqCst);
        self.0.load(Ordering::Acquire) && self.0.swap(false, Ordering::AcqRel)
    }
}
