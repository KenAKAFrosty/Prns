#[cfg(test)]
mod architecture;
#[cfg(test)]
mod behavior;
mod receive;
mod refresh;
mod settlement;
mod state;
#[cfg(test)]
mod tests;

use crate::domain_primitives::{Connection, InterfaceRefreshFailure};
use core::num::NonZeroU64;
use pipecircuit::StateMachine;
use prns_core::remote_control::RemoteControlInterfaceEntry;

pub use receive::{ReceiveInterfacePage, ReceiveInterfacePageOutcome};
pub use refresh::{RefreshInterfaces, RefreshInterfacesOutcome};
pub use settlement::{
    CloseInterfaceInventory, CloseInterfaceInventoryOutcome, InterfaceRefreshFailed,
    InterfaceRefreshFailedOutcome,
};
pub use state::{InterfaceInventorySnapshot, InterfaceInventoryStatus, ReadInterfaces};

pub struct InterfaceInventory<const CAPACITY: usize> {
    connection: Connection,
    next_refresh: Option<NonZeroU64>,
    status: InterfaceInventoryStatus,
    staging: heapless::Vec<RemoteControlInterfaceEntry, CAPACITY>,
    published: Option<heapless::Vec<RemoteControlInterfaceEntry, CAPACITY>>,
}

impl<const CAPACITY: usize> InterfaceInventory<CAPACITY> {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            next_refresh: Some(NonZeroU64::MIN),
            status: InterfaceInventoryStatus::NotRequested,
            staging: heapless::Vec::new(),
            published: None,
        }
    }

    fn fail(&mut self, reason: InterfaceRefreshFailure) {
        self.staging.clear();
        self.status = InterfaceInventoryStatus::Failed { reason };
    }
}

impl<const CAPACITY: usize> StateMachine for InterfaceInventory<CAPACITY> {}
