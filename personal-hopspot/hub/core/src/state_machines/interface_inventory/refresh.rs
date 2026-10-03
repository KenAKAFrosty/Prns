use super::{InterfaceInventory, InterfaceInventoryStatus};
use crate::domain_primitives::InterfacePageRequest;
use pipecircuit::StepInputOf;
use prns_core::remote_control::RemoteControlInterfacePage;

pub struct RefreshInterfaces;

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum RefreshInterfacesOutcome {
    Requested { request: InterfacePageRequest },
    Busy { pending: InterfacePageRequest },
    Closed,
    IdentifiersExhausted,
}

impl<const CAPACITY: usize> StepInputOf<InterfaceInventory<CAPACITY>> for RefreshInterfaces {
    type Outcome = RefreshInterfacesOutcome;

    fn step(self, inventory: &mut InterfaceInventory<CAPACITY>) -> Self::Outcome {
        match inventory.status {
            InterfaceInventoryStatus::Receiving { pending } => {
                return RefreshInterfacesOutcome::Busy { pending };
            }
            InterfaceInventoryStatus::Closed => return RefreshInterfacesOutcome::Closed,
            InterfaceInventoryStatus::NotRequested
            | InterfaceInventoryStatus::Ready
            | InterfaceInventoryStatus::Failed { .. } => {}
        }
        let Some(generation) = inventory.next_refresh else {
            return RefreshInterfacesOutcome::IdentifiersExhausted;
        };
        let request = InterfacePageRequest {
            connection: inventory.connection,
            generation,
            page: RemoteControlInterfacePage::First,
        };
        inventory.next_refresh = generation.checked_add(1);
        inventory.staging.clear();
        inventory.status = InterfaceInventoryStatus::Receiving { pending: request };
        RefreshInterfacesOutcome::Requested { request }
    }
}
