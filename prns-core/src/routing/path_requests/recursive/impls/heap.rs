use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::lemire_index::HeapLemireIndex;
use crate::routing::path_requests::recursive::RecursivePathRequestTable;
#[cfg(feature = "std")]
use crate::routing::temporal_index::HeapDeadlineIndex;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapRecursivePathRequestTable {
    destinations: Vec<DestinationHash>,
    requesting_interfaces: Vec<InterfaceId>,
    expires_ats: Vec<InstantMillis>,
    index: HeapLemireIndex,
    #[cfg(feature = "std")]
    expiry_index: HeapDeadlineIndex,
}

impl RecursivePathRequestTable for HeapRecursivePathRequestTable {
    fn capacity(&self) -> usize {
        usize::MAX
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

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index.get(destination, &self.destinations)
    }

    fn earliest_indexed_expiry(&mut self) -> Option<InstantMillis> {
        #[cfg(feature = "std")]
        {
            let row_count = self.expires_ats.len();
            let expires_ats = &self.expires_ats;
            return self
                .expiry_index
                .earliest_exact(row_count, |row| expires_ats.get(row).copied());
        }
        #[cfg(not(feature = "std"))]
        self.expires_ats.iter().copied().min()
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

    fn push(
        &mut self,
        destination: DestinationHash,
        requesting_interface: InterfaceId,
        expires_at: InstantMillis,
    ) {
        let row = self.destinations.len();
        self.destinations.push(destination);
        self.requesting_interfaces.push(requesting_interface);
        self.expires_ats.push(expires_at);
        self.index.insert(row, &self.destinations);
        #[cfg(feature = "std")]
        {
            let expires_ats = &self.expires_ats;
            self.expiry_index
                .insert(row, Some(expires_at), |row| expires_ats.get(row).copied());
        }
    }

    fn swap_remove(&mut self, index: usize) {
        if index >= self.destinations.len() {
            return;
        }
        let last = self.destinations.len() - 1;
        self.index.remove_slot(index, &self.destinations);
        if index != last {
            self.index.repoint_slot(last, index, &self.destinations);
        }
        #[cfg(feature = "std")]
        {
            let expires_ats = &self.expires_ats;
            self.expiry_index
                .swap_remove(index, last, |row| expires_ats.get(row).copied());
        }
        self.destinations.swap_remove(index);
        self.requesting_interfaces.swap_remove(index);
        self.expires_ats.swap_remove(index);
    }
}
