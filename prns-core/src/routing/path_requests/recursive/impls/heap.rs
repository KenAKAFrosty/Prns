use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::path_requests::recursive::RecursivePathRequestTable;
use crate::wire::DestinationHash;

/// The reference's `discovery_path_requests` is an unbounded dict pruned by age.
/// This cap bounds how many recursive requests a single node carries on others'
/// behalf at once, the same hygiene the sibling path-request tables keep. In-flight
/// recursive requests are rarer than seen tags, so this sits an order below them.
pub const DEFAULT_MAX_RECURSIVE_PATH_REQUESTS: usize = 256;

#[derive(Debug, Default)]
pub struct HeapRecursivePathRequestTable {
    destinations: Vec<DestinationHash>,
    requesting_interfaces: Vec<InterfaceId>,
    expires_ats: Vec<InstantMillis>,
}

impl RecursivePathRequestTable for HeapRecursivePathRequestTable {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_RECURSIVE_PATH_REQUESTS
    }
    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }
    fn requesting_interfaces(&self) -> &[InterfaceId] {
        &self.requesting_interfaces
    }
    fn expires_ats(&self) -> &[InstantMillis] {
        &self.expires_ats
    }

    fn push(
        &mut self,
        destination: DestinationHash,
        requesting_interface: InterfaceId,
        expires_at: InstantMillis,
    ) {
        if self.len() >= self.capacity() {
            return;
        }
        self.destinations.push(destination);
        self.requesting_interfaces.push(requesting_interface);
        self.expires_ats.push(expires_at);
    }

    fn swap_remove(&mut self, index: usize) {
        self.destinations.swap_remove(index);
        self.requesting_interfaces.swap_remove(index);
        self.expires_ats.swap_remove(index);
    }
}
