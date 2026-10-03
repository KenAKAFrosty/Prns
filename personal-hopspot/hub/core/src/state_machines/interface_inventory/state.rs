use super::InterfaceInventory;
use crate::domain_primitives::{Connection, InterfacePageRequest, InterfaceRefreshFailure};
use pipecircuit::StepInputOf;
use prns_core::remote_control::RemoteControlInterfaceEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceInventoryStatus {
    NotRequested,
    Receiving { pending: InterfacePageRequest },
    Ready,
    Failed { reason: InterfaceRefreshFailure },
    Closed,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct InterfaceInventorySnapshot<const CAPACITY: usize> {
    pub connection: Connection,
    pub status: InterfaceInventoryStatus,
    pub interfaces: Option<heapless::Vec<RemoteControlInterfaceEntry, CAPACITY>>,
}

pub struct ReadInterfaces;

impl<const CAPACITY: usize> StepInputOf<InterfaceInventory<CAPACITY>> for ReadInterfaces {
    type Outcome = InterfaceInventorySnapshot<CAPACITY>;

    fn step(self, inventory: &mut InterfaceInventory<CAPACITY>) -> Self::Outcome {
        InterfaceInventorySnapshot {
            connection: inventory.connection,
            status: inventory.status,
            interfaces: inventory.published.clone(),
        }
    }
}
