use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::lemire_index::HeapLemireIndex;
use crate::routing::reverse_routes::{ReverseRouteEntry, ReverseRouteTable};
#[cfg(feature = "std")]
use crate::routing::temporal_index::HeapDeadlineIndex;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapReverseRouteTable {
    proof_destinations: Vec<DestinationHash>,
    received_interfaces: Vec<InterfaceId>,
    outbound_interfaces: Vec<InterfaceId>,
    expires_ats: Vec<InstantMillis>,
    index: HeapLemireIndex,
    #[cfg(feature = "std")]
    expiry_index: HeapDeadlineIndex,
}

impl ReverseRouteTable for HeapReverseRouteTable {
    fn capacity(&self) -> usize {
        usize::MAX
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

    fn index_of(&self, proof_destination: &DestinationHash) -> Option<usize> {
        self.index.get(proof_destination, &self.proof_destinations)
    }

    fn first_expired(&mut self, now: InstantMillis) -> Option<usize> {
        #[cfg(feature = "std")]
        {
            let row_count = self.expires_ats.len();
            let expires_ats = &self.expires_ats;
            return self
                .expiry_index
                .first_due(row_count, now, |row| expires_ats.get(row).copied());
        }
        #[cfg(not(feature = "std"))]
        self.expires_ats
            .iter()
            .position(|expires_at| *expires_at <= now)
    }

    fn push(&mut self, entry: ReverseRouteEntry) {
        let row = self.proof_destinations.len();
        self.proof_destinations.push(entry.proof_destination);
        self.received_interfaces.push(entry.received_interface);
        self.outbound_interfaces.push(entry.outbound_interface);
        self.expires_ats.push(entry.expires_at);
        self.index.insert(row, &self.proof_destinations);
        #[cfg(feature = "std")]
        {
            let expires_ats = &self.expires_ats;
            self.expiry_index
                .insert(row, Some(entry.expires_at), |row| {
                    expires_ats.get(row).copied()
                });
        }
    }

    fn swap_remove(&mut self, index: usize) {
        if index >= self.proof_destinations.len() {
            return;
        }
        let last = self.proof_destinations.len() - 1;
        self.index.remove_slot(index, &self.proof_destinations);
        if index != last {
            self.index
                .repoint_slot(last, index, &self.proof_destinations);
        }
        #[cfg(feature = "std")]
        {
            let expires_ats = &self.expires_ats;
            self.expiry_index
                .swap_remove(index, last, |row| expires_ats.get(row).copied());
        }
        self.proof_destinations.swap_remove(index);
        self.received_interfaces.swap_remove(index);
        self.outbound_interfaces.swap_remove(index);
        self.expires_ats.swap_remove(index);
    }
}
