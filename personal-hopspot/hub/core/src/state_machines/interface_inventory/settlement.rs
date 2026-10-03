use super::{InterfaceInventory, InterfaceInventoryStatus};
use crate::domain_primitives::{InterfacePageRequest, InterfaceRefreshFailure};
use pipecircuit::StepInputOf;

#[derive(Debug, PartialEq, Eq)]
pub struct InterfaceRefreshFailed {
    pub request: InterfacePageRequest,
    pub reason: InterfaceRefreshFailure,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum InterfaceRefreshFailedOutcome {
    Failed {
        request: InterfacePageRequest,
        reason: InterfaceRefreshFailure,
    },
    StaleRequest {
        rejected: InterfaceRefreshFailed,
    },
}

impl<const CAPACITY: usize> StepInputOf<InterfaceInventory<CAPACITY>> for InterfaceRefreshFailed {
    type Outcome = InterfaceRefreshFailedOutcome;

    fn step(self, inventory: &mut InterfaceInventory<CAPACITY>) -> Self::Outcome {
        if inventory.status
            != (InterfaceInventoryStatus::Receiving {
                pending: self.request,
            })
        {
            return InterfaceRefreshFailedOutcome::StaleRequest { rejected: self };
        }
        inventory.fail(self.reason);
        InterfaceRefreshFailedOutcome::Failed {
            request: self.request,
            reason: self.reason,
        }
    }
}

pub struct CloseInterfaceInventory;

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum CloseInterfaceInventoryOutcome {
    Closed {
        pending: Option<InterfacePageRequest>,
    },
    AlreadyClosed,
}

impl<const CAPACITY: usize> StepInputOf<InterfaceInventory<CAPACITY>> for CloseInterfaceInventory {
    type Outcome = CloseInterfaceInventoryOutcome;

    fn step(self, inventory: &mut InterfaceInventory<CAPACITY>) -> Self::Outcome {
        let pending = match inventory.status {
            InterfaceInventoryStatus::Receiving { pending } => Some(pending),
            InterfaceInventoryStatus::Closed => {
                return CloseInterfaceInventoryOutcome::AlreadyClosed;
            }
            InterfaceInventoryStatus::NotRequested
            | InterfaceInventoryStatus::Ready
            | InterfaceInventoryStatus::Failed { .. } => None,
        };
        inventory.staging.clear();
        inventory.published = None;
        inventory.status = InterfaceInventoryStatus::Closed;
        CloseInterfaceInventoryOutcome::Closed { pending }
    }
}
