use super::{InterfaceInventory, InterfaceInventoryStatus};
use crate::domain_primitives::{InterfacePageRequest, InterfaceRefreshFailure};
use pipecircuit::StepInputOf;
use prns_core::remote_control::{
    RemoteControlInterfaceContinuation, RemoteControlInterfaceInventory, RemoteControlInterfacePage,
};

#[derive(Debug, PartialEq, Eq)]
pub struct ReceiveInterfacePage {
    pub request: InterfacePageRequest,
    pub page: RemoteControlInterfaceInventory,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum ReceiveInterfacePageOutcome {
    More {
        request: InterfacePageRequest,
    },
    Complete {
        count: usize,
    },
    StaleRequest {
        rejected: ReceiveInterfacePage,
    },
    OutOfOrder {
        request: InterfacePageRequest,
    },
    CapacityExceeded {
        request: InterfacePageRequest,
        maximum: usize,
    },
}

impl<const CAPACITY: usize> StepInputOf<InterfaceInventory<CAPACITY>> for ReceiveInterfacePage {
    type Outcome = ReceiveInterfacePageOutcome;

    fn step(self, inventory: &mut InterfaceInventory<CAPACITY>) -> Self::Outcome {
        if inventory.status
            != (InterfaceInventoryStatus::Receiving {
                pending: self.request,
            })
        {
            return ReceiveInterfacePageOutcome::StaleRequest { rejected: self };
        }
        for entry in self.page.entries() {
            if inventory
                .staging
                .last()
                .is_some_and(|previous| previous.id.as_bytes() >= entry.id.as_bytes())
            {
                let reason = InterfaceRefreshFailure::OutOfOrder;
                inventory.fail(reason);
                return ReceiveInterfacePageOutcome::OutOfOrder {
                    request: self.request,
                };
            }
            if inventory.staging.push(*entry).is_err() {
                let reason = InterfaceRefreshFailure::CapacityExceeded { maximum: CAPACITY };
                inventory.fail(reason);
                return ReceiveInterfacePageOutcome::CapacityExceeded {
                    request: self.request,
                    maximum: CAPACITY,
                };
            }
        }
        match self.page.continuation() {
            RemoteControlInterfaceContinuation::More(cursor) => {
                let request = InterfacePageRequest {
                    page: RemoteControlInterfacePage::After(cursor),
                    ..self.request
                };
                inventory.status = InterfaceInventoryStatus::Receiving { pending: request };
                ReceiveInterfacePageOutcome::More { request }
            }
            RemoteControlInterfaceContinuation::Complete => {
                let count = inventory.staging.len();
                inventory.published = Some(core::mem::take(&mut inventory.staging));
                inventory.status = InterfaceInventoryStatus::Ready;
                ReceiveInterfacePageOutcome::Complete { count }
            }
        }
    }
}
