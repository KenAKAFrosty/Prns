use super::DeviceInterfaces;
use crate::domain_primitives::DeviceId;
use crate::state_machines::interface_inventory::{InterfaceInventorySnapshot, ReadInterfaces};
use pipecircuit::{StateMachine, StepInputOf};

pub struct ReadDeviceInterfaces;

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum ReadDeviceInterfacesOutcome<const CAPACITY: usize> {
    Found {
        device: DeviceId,
        inventory: InterfaceInventorySnapshot<CAPACITY>,
    },
    Unavailable {
        device: DeviceId,
    },
}

impl<const CAPACITY: usize> StepInputOf<DeviceInterfaces<CAPACITY>> for ReadDeviceInterfaces {
    type Outcome = ReadDeviceInterfacesOutcome<CAPACITY>;

    fn step(self, interfaces: &mut DeviceInterfaces<CAPACITY>) -> Self::Outcome {
        match &mut interfaces.inventory {
            Some(inventory) => ReadDeviceInterfacesOutcome::Found {
                device: interfaces.device,
                inventory: inventory.step(ReadInterfaces),
            },
            None => ReadDeviceInterfacesOutcome::Unavailable {
                device: interfaces.device,
            },
        }
    }
}
