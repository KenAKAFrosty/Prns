use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::reverse_routes::{ReverseRouteColumns, ReverseRouteEntry};
use crate::wire::DestinationHash;

/// The reference's `reverse_table` is an unbounded dict culled by timeout;
/// a daemon-grade cap keeps a hostile packet flood from ballooning memory,
/// matching the receipts table's hygiene at the same order of magnitude.
pub const DEFAULT_MAX_REVERSE_ROUTES: usize = 1024;

#[derive(Debug, Default)]
pub struct HeapReverseRouteColumns {
    proof_destinations: Vec<DestinationHash>,
    received_interfaces: Vec<InterfaceId>,
    outbound_interfaces: Vec<InterfaceId>,
    expires_ats: Vec<InstantMillis>,
}

impl ReverseRouteColumns for HeapReverseRouteColumns {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_REVERSE_ROUTES
    }
    fn len(&self) -> usize {
        self.proof_destinations.len()
    }

    fn proof_destinations(&self) -> &[DestinationHash] {
        &self.proof_destinations
    }
    fn received_interfaces(&self) -> &[InterfaceId] {
        &self.received_interfaces
    }
    fn outbound_interfaces(&self) -> &[InterfaceId] {
        &self.outbound_interfaces
    }
    fn expires_ats(&self) -> &[InstantMillis] {
        &self.expires_ats
    }

    fn push(&mut self, entry: ReverseRouteEntry) {
        if self.len() >= self.capacity() {
            return;
        }
        self.proof_destinations.push(entry.proof_destination);
        self.received_interfaces.push(entry.received_interface);
        self.outbound_interfaces.push(entry.outbound_interface);
        self.expires_ats.push(entry.expires_at);
    }

    fn swap_remove(&mut self, index: usize) {
        self.proof_destinations.swap_remove(index);
        self.received_interfaces.swap_remove(index);
        self.outbound_interfaces.swap_remove(index);
        self.expires_ats.swap_remove(index);
    }
}
