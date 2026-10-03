use super::{InterfaceInventory, InterfaceInventoryStatus};
use crate::domain_primitives::InterfacePageRequest;
use pipecircuit::StepInputOf;

pub struct RefreshInterfaces;

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum RefreshInterfacesOutcome {
    Requested { request: InterfacePageRequest },
    Busy { pending: InterfacePageRequest },
    Closed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RefreshInterfacesError {
    IdentifiersExhausted,
}

impl<const CAPACITY: usize> StepInputOf<InterfaceInventory<CAPACITY>> for RefreshInterfaces {
    type Outcome = Result<RefreshInterfacesOutcome, RefreshInterfacesError>;

    fn step(self, inventory: &mut InterfaceInventory<CAPACITY>) -> Self::Outcome {
        match inventory.status {
            InterfaceInventoryStatus::Receiving { pending } => {
                return Ok(RefreshInterfacesOutcome::Busy { pending });
            }
            InterfaceInventoryStatus::Closed => return Ok(RefreshInterfacesOutcome::Closed),
            InterfaceInventoryStatus::NotRequested
            | InterfaceInventoryStatus::Ready
            | InterfaceInventoryStatus::Failed { .. } => {}
        }
        let Some(generation) = inventory.next_refresh else {
            return Err(RefreshInterfacesError::IdentifiersExhausted);
        };
        Ok(RefreshInterfacesOutcome::Requested {
            request: inventory.begin_refresh(generation),
        })
    }
}
