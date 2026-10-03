use super::DeviceInterfaces;
use crate::state_machines::interface_inventory::{
    CloseInterfaceInventory, CloseInterfaceInventoryOutcome, InterfaceRefreshFailed,
    InterfaceRefreshFailedOutcome, ReceiveInterfacePage, ReceiveInterfacePageOutcome,
    RefreshInterfaces, RefreshInterfacesError, RefreshInterfacesOutcome,
};
use pipecircuit::{StateMachine, StepInputOf};

impl<const CAPACITY: usize> StepInputOf<DeviceInterfaces<CAPACITY>> for RefreshInterfaces {
    type Outcome = Result<RefreshInterfacesOutcome, RefreshInterfacesError>;

    fn step(self, interfaces: &mut DeviceInterfaces<CAPACITY>) -> Self::Outcome {
        match &mut interfaces.inventory {
            Some(inventory) => inventory.step(self),
            None => Ok(RefreshInterfacesOutcome::Closed),
        }
    }
}

impl<const CAPACITY: usize> StepInputOf<DeviceInterfaces<CAPACITY>> for ReceiveInterfacePage {
    type Outcome = ReceiveInterfacePageOutcome;

    fn step(self, interfaces: &mut DeviceInterfaces<CAPACITY>) -> Self::Outcome {
        match &mut interfaces.inventory {
            Some(inventory) => inventory.step(self),
            None => ReceiveInterfacePageOutcome::StaleRequest { rejected: self },
        }
    }
}

impl<const CAPACITY: usize> StepInputOf<DeviceInterfaces<CAPACITY>> for InterfaceRefreshFailed {
    type Outcome = InterfaceRefreshFailedOutcome;

    fn step(self, interfaces: &mut DeviceInterfaces<CAPACITY>) -> Self::Outcome {
        match &mut interfaces.inventory {
            Some(inventory) => inventory.step(self),
            None => InterfaceRefreshFailedOutcome::StaleRequest { rejected: self },
        }
    }
}

impl<const CAPACITY: usize> StepInputOf<DeviceInterfaces<CAPACITY>> for CloseInterfaceInventory {
    type Outcome = CloseInterfaceInventoryOutcome;

    fn step(self, interfaces: &mut DeviceInterfaces<CAPACITY>) -> Self::Outcome {
        match &mut interfaces.inventory {
            Some(inventory) => inventory.step(self),
            None => CloseInterfaceInventoryOutcome::AlreadyClosed,
        }
    }
}
