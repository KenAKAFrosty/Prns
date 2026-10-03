use super::DeviceInterfaces;
use crate::domain_primitives::{Connection, InterfacePageRequest};
use crate::state_machines::{
    ConnectionState, DeviceRegistry, InterfaceInventory, ReadDevice, ReadDeviceOutcome,
    ReadInventoryConnection,
};
use pipecircuit::{StateMachine, StepInputOf};
use prns_core::routing::links::LinkId;

pub struct SynchronizeDeviceInterfaces<'registry> {
    pub registry: &'registry mut DeviceRegistry,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum SynchronizeDeviceInterfacesOutcome {
    Started {
        request: InterfacePageRequest,
        link: LinkId,
        cancelled: Option<InterfacePageRequest>,
    },
    Unchanged {
        connection: Connection,
        link: LinkId,
    },
    Unavailable {
        cancelled: Option<InterfacePageRequest>,
    },
}

impl<const CAPACITY: usize> StepInputOf<DeviceInterfaces<CAPACITY>>
    for SynchronizeDeviceInterfaces<'_>
{
    type Outcome = SynchronizeDeviceInterfacesOutcome;

    fn step(self, interfaces: &mut DeviceInterfaces<CAPACITY>) -> Self::Outcome {
        let connected = match self.registry.step(ReadDevice {
            device: interfaces.device,
        }) {
            ReadDeviceOutcome::Found { device } => match device.connection {
                ConnectionState::Connected { connection, link } => Some((connection, link)),
                ConnectionState::NotConnected
                | ConnectionState::Connecting { .. }
                | ConnectionState::Disconnected { .. } => None,
            },
            ReadDeviceOutcome::MissingDevice { .. } => None,
        };
        let Some((connection, link)) = connected else {
            return SynchronizeDeviceInterfacesOutcome::Unavailable {
                cancelled: interfaces.detach(),
            };
        };
        if interfaces
            .inventory
            .as_mut()
            .is_some_and(|inventory| inventory.step(ReadInventoryConnection) == connection)
        {
            return SynchronizeDeviceInterfacesOutcome::Unchanged { connection, link };
        }
        let cancelled = interfaces.detach();
        let (inventory, request) = InterfaceInventory::with_initial_refresh(connection);
        interfaces.inventory = Some(inventory);
        SynchronizeDeviceInterfacesOutcome::Started {
            request,
            link,
            cancelled,
        }
    }
}
