use crate::engine::reverse_routes::{ReverseRouteColumns, ReverseRouteEntry};
use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;

#[derive(Debug)]
pub struct FixedReverseRouteColumns<const MAX_REVERSE_ROUTES: usize> {
    len: usize,
    proof_destinations: [DestinationHash; MAX_REVERSE_ROUTES],
    received_interfaces: [InterfaceId; MAX_REVERSE_ROUTES],
    outbound_interfaces: [InterfaceId; MAX_REVERSE_ROUTES],
    expires_ats: [InstantMillis; MAX_REVERSE_ROUTES],
}

impl<const MAX_REVERSE_ROUTES: usize> Default for FixedReverseRouteColumns<MAX_REVERSE_ROUTES> {
    fn default() -> Self {
        Self {
            len: 0,
            proof_destinations: [DestinationHash::new([0u8; 16]); MAX_REVERSE_ROUTES],
            received_interfaces: [InterfaceId::new([0u8; 16]); MAX_REVERSE_ROUTES],
            outbound_interfaces: [InterfaceId::new([0u8; 16]); MAX_REVERSE_ROUTES],
            expires_ats: [InstantMillis(0); MAX_REVERSE_ROUTES],
        }
    }
}

impl<const MAX_REVERSE_ROUTES: usize> ReverseRouteColumns
    for FixedReverseRouteColumns<MAX_REVERSE_ROUTES>
{
    fn capacity(&self) -> usize {
        MAX_REVERSE_ROUTES
    }
    fn len(&self) -> usize {
        self.len
    }

    fn proof_destinations(&self) -> &[DestinationHash] {
        &self.proof_destinations[..self.len]
    }
    fn received_interfaces(&self) -> &[InterfaceId] {
        &self.received_interfaces[..self.len]
    }
    fn outbound_interfaces(&self) -> &[InterfaceId] {
        &self.outbound_interfaces[..self.len]
    }
    fn expires_ats(&self) -> &[InstantMillis] {
        &self.expires_ats[..self.len]
    }

    fn push(&mut self, entry: ReverseRouteEntry) {
        if self.len >= MAX_REVERSE_ROUTES {
            return;
        }
        let i = self.len;
        self.proof_destinations[i] = entry.proof_destination;
        self.received_interfaces[i] = entry.received_interface;
        self.outbound_interfaces[i] = entry.outbound_interface;
        self.expires_ats[i] = entry.expires_at;
        self.len += 1;
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        if index != last {
            self.proof_destinations[index] = self.proof_destinations[last];
            self.received_interfaces[index] = self.received_interfaces[last];
            self.outbound_interfaces[index] = self.outbound_interfaces[last];
            self.expires_ats[index] = self.expires_ats[last];
        }
        self.len = last;
    }
}
