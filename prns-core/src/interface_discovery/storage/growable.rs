use alloc::collections::{btree_map, btree_set, BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::interfaces::InterfaceId;
use crate::lemire_index::{HeapLemireIndex, IndexRow};
use crate::storage::TablePushError;

use super::{
    DiscoveredConnectionTable, DiscoveredEndpointSet, DiscoveryCatalogTable,
    DiscoveryValidationCache, InterfaceDiscoveryStorage,
};
use crate::interface_discovery::{
    ActiveDiscoveredInterface, DiscoveredConnectionEndpointId, DiscoveredInterfaceId,
    DiscoveryRecord, StampValue,
};

pub const RNS_VALIDATION_CACHE_CAPACITY: usize = 2_048;

#[derive(Debug)]
struct HeapValidatedEnvelope {
    payload_hash: [u8; 32],
    packed_advertisement: Vec<u8>,
    stamp_value: StampValue,
}

impl IndexRow for HeapValidatedEnvelope {
    type Key = [u8; 32];

    fn index_key(&self) -> &Self::Key {
        &self.payload_hash
    }
}

#[derive(Debug)]
struct HeapInsufficientStamp {
    payload_hash: [u8; 32],
    stamp_value: StampValue,
}

impl IndexRow for HeapInsufficientStamp {
    type Key = [u8; 32];

    fn index_key(&self) -> &Self::Key {
        &self.payload_hash
    }
}

#[derive(Debug)]
pub struct HeapDiscoveryValidationCache {
    valid: Vec<HeapValidatedEnvelope>,
    valid_index: HeapLemireIndex,
    valid_next_evict: usize,
    insufficient: Vec<HeapInsufficientStamp>,
    insufficient_index: HeapLemireIndex,
    insufficient_next_evict: usize,
}

impl Default for HeapDiscoveryValidationCache {
    fn default() -> Self {
        Self {
            valid: Vec::new(),
            valid_index: HeapLemireIndex::default(),
            valid_next_evict: 0,
            insufficient: Vec::new(),
            insufficient_index: HeapLemireIndex::default(),
            insufficient_next_evict: 0,
        }
    }
}

impl HeapDiscoveryValidationCache {
    #[cfg(test)]
    pub(crate) fn lengths(&self) -> (usize, usize) {
        (self.valid.len(), self.insufficient.len())
    }
}

impl DiscoveryValidationCache for HeapDiscoveryValidationCache {
    fn valid(&self, payload_hash: &[u8; 32]) -> Option<(&[u8], StampValue)> {
        let slot = self.valid_index.get(payload_hash, &self.valid)?;
        let entry = &self.valid[slot];
        Some((entry.packed_advertisement.as_slice(), entry.stamp_value))
    }

    fn insufficient(&self, payload_hash: &[u8; 32]) -> Option<StampValue> {
        let slot = self
            .insufficient_index
            .get(payload_hash, &self.insufficient)?;
        Some(self.insufficient[slot].stamp_value)
    }

    fn remember_valid(
        &mut self,
        payload_hash: [u8; 32],
        packed_advertisement: &[u8],
        stamp_value: StampValue,
    ) {
        if self.valid_index.contains(&payload_hash, &self.valid) {
            return;
        }
        let entry = HeapValidatedEnvelope {
            payload_hash,
            packed_advertisement: packed_advertisement.to_vec(),
            stamp_value,
        };
        if self.valid.len() < RNS_VALIDATION_CACHE_CAPACITY {
            let slot = self.valid.len();
            self.valid.push(entry);
            self.valid_index.insert(slot, &self.valid);
        } else {
            let slot = self.valid_next_evict;
            self.valid_index.remove_slot(slot, &self.valid);
            self.valid[slot] = entry;
            self.valid_index.insert(slot, &self.valid);
            self.valid_next_evict = (slot + 1) % RNS_VALIDATION_CACHE_CAPACITY;
        }
    }

    fn remember_insufficient(&mut self, payload_hash: [u8; 32], stamp_value: StampValue) {
        if self
            .insufficient_index
            .contains(&payload_hash, &self.insufficient)
        {
            return;
        }
        let entry = HeapInsufficientStamp {
            payload_hash,
            stamp_value,
        };
        if self.insufficient.len() < RNS_VALIDATION_CACHE_CAPACITY {
            let slot = self.insufficient.len();
            self.insufficient.push(entry);
            self.insufficient_index.insert(slot, &self.insufficient);
        } else {
            let slot = self.insufficient_next_evict;
            self.insufficient_index
                .remove_slot(slot, &self.insufficient);
            self.insufficient[slot] = entry;
            self.insufficient_index.insert(slot, &self.insufficient);
            self.insufficient_next_evict = (slot + 1) % RNS_VALIDATION_CACHE_CAPACITY;
        }
    }
}

#[derive(Debug, Default)]
pub struct HeapDiscoveryCatalogTable {
    records: BTreeMap<DiscoveredInterfaceId, DiscoveryRecord>,
}

impl DiscoveryCatalogTable for HeapDiscoveryCatalogTable {
    type Records<'a> = btree_map::Values<'a, DiscoveredInterfaceId, DiscoveryRecord>;

    fn len(&self) -> usize {
        self.records.len()
    }

    fn get(&self, id: DiscoveredInterfaceId) -> Option<&DiscoveryRecord> {
        self.records.get(&id)
    }

    fn get_mut(&mut self, id: DiscoveredInterfaceId) -> Option<&mut DiscoveryRecord> {
        self.records.get_mut(&id)
    }

    fn try_insert(
        &mut self,
        id: DiscoveredInterfaceId,
        record: DiscoveryRecord,
    ) -> Result<Option<DiscoveryRecord>, TablePushError> {
        Ok(self.records.insert(id, record))
    }

    fn remove(&mut self, id: DiscoveredInterfaceId) -> Option<DiscoveryRecord> {
        self.records.remove(&id)
    }

    fn records(&self) -> Self::Records<'_> {
        self.records.values()
    }
}

#[derive(Debug, Default)]
pub struct HeapDiscoveredConnectionTable {
    connections: BTreeMap<InterfaceId, ActiveDiscoveredInterface>,
}

impl DiscoveredConnectionTable for HeapDiscoveredConnectionTable {
    type Connections<'a> = btree_map::Values<'a, InterfaceId, ActiveDiscoveredInterface>;

    fn len(&self) -> usize {
        self.connections.len()
    }

    fn get_mut(&mut self, interface: InterfaceId) -> Option<&mut ActiveDiscoveredInterface> {
        self.connections.get_mut(&interface)
    }

    fn contains_interface(&self, interface: InterfaceId) -> bool {
        self.connections.contains_key(&interface)
    }

    fn contains_endpoint(&self, endpoint: DiscoveredConnectionEndpointId) -> bool {
        self.connections
            .values()
            .any(|active| active.endpoint_id() == endpoint)
    }

    fn try_insert(
        &mut self,
        interface: ActiveDiscoveredInterface,
    ) -> Result<Option<ActiveDiscoveredInterface>, TablePushError> {
        Ok(self.connections.insert(interface.interface_id(), interface))
    }

    fn remove(&mut self, interface: InterfaceId) -> Option<ActiveDiscoveredInterface> {
        self.connections.remove(&interface)
    }

    fn connections(&self) -> Self::Connections<'_> {
        self.connections.values()
    }
}

#[derive(Debug, Default)]
pub struct HeapDiscoveredEndpointSet {
    endpoints: BTreeSet<DiscoveredConnectionEndpointId>,
}

impl DiscoveredEndpointSet for HeapDiscoveredEndpointSet {
    type Endpoints<'a> = core::iter::Copied<btree_set::Iter<'a, DiscoveredConnectionEndpointId>>;

    fn try_insert(
        &mut self,
        endpoint: DiscoveredConnectionEndpointId,
    ) -> Result<bool, TablePushError> {
        Ok(self.endpoints.insert(endpoint))
    }

    fn endpoints(&self) -> Self::Endpoints<'_> {
        self.endpoints.iter().copied()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GrowableInterfaceDiscoveryStorage;

impl InterfaceDiscoveryStorage for GrowableInterfaceDiscoveryStorage {
    type ValidationCache = HeapDiscoveryValidationCache;
    type Catalog = HeapDiscoveryCatalogTable;
    type Connections = HeapDiscoveredConnectionTable;
    type ReservedEndpoints = HeapDiscoveredEndpointSet;
}
