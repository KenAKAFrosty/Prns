//! A restrictive execution fence; the coordinator remains the authority.
use super::runtime;
use prns_storage::CloseSignal;
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
const STAGED: u8 = 0;
const OPEN: u8 = 1;
const FENCED: u8 = 2;

/// Bound to one immutable runtime owner, never an identity-wide authorization
/// snapshot. Native effects must also hold their real scoped admission/leases.
/// This read-only view cannot open/reopen a generation or settle its work.
#[derive(Clone)]
pub struct DispatchLease {
    owner: runtime::OwnerToken,
    state: Arc<AtomicU8>,
    closed: CloseSignal,
}
impl DispatchLease {
    pub fn owner(&self) -> runtime::OwnerToken {
        self.owner
    }
    pub fn is_open(&self) -> bool {
        !self.closed.is_closed() && self.state.load(Ordering::Acquire) == OPEN
    }
}
pub(super) struct Control(DispatchLease);
impl Control {
    pub fn new(owner: runtime::OwnerToken, closed: CloseSignal) -> Self {
        Self(DispatchLease {
            owner,
            state: Arc::new(AtomicU8::new(STAGED)),
            closed,
        })
    }
    pub fn owner(&self) -> runtime::OwnerToken {
        self.0.owner
    }
    pub fn lease(&self) -> DispatchLease {
        self.0.clone()
    }
    pub fn open(&self) -> bool {
        self.0
            .state
            .compare_exchange(STAGED, OPEN, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            && !self.0.closed.is_closed()
    }
    pub fn fence(&self) {
        self.0.state.store(FENCED, Ordering::Release);
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.fence();
    }
}
