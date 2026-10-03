#[cfg(test)]
mod architecture;
#[cfg(test)]
mod behavior;
mod inventory;
mod query;
mod synchronize;
#[cfg(test)]
mod tests;

use super::interface_inventory::{
    CloseInterfaceInventory, CloseInterfaceInventoryOutcome, InterfaceInventory,
};
use crate::domain_primitives::{DeviceId, InterfacePageRequest};
use pipecircuit::StateMachine;

pub use query::{ReadDeviceInterfaces, ReadDeviceInterfacesOutcome};
pub use synchronize::{SynchronizeDeviceInterfaces, SynchronizeDeviceInterfacesOutcome};

pub struct DeviceInterfaces<const CAPACITY: usize> {
    device: DeviceId,
    inventory: Option<InterfaceInventory<CAPACITY>>,
}

impl<const CAPACITY: usize> DeviceInterfaces<CAPACITY> {
    pub fn new(device: DeviceId) -> Self {
        Self {
            device,
            inventory: None,
        }
    }

    fn detach(&mut self) -> Option<InterfacePageRequest> {
        let closed = self.step(CloseInterfaceInventory);
        self.inventory = None;
        match closed {
            CloseInterfaceInventoryOutcome::Closed { pending } => pending,
            CloseInterfaceInventoryOutcome::AlreadyClosed => None,
        }
    }
}

impl<const CAPACITY: usize> StateMachine for DeviceInterfaces<CAPACITY> {}
