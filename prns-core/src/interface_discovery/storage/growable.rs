use alloc::collections::{btree_map, btree_set, BTreeMap, BTreeSet};

use crate::interfaces::InterfaceId;
use crate::storage::TablePushError;

use super::{
    DiscoveredConnectionTable, DiscoveredEndpointSet, DiscoveryCatalogTable,
    InterfaceDiscoveryStorage,
};
use crate::interface_discovery::{
    ActiveDiscoveredInterface, DiscoveredConnectionEndpointId, DiscoveredInterfaceId,
    DiscoveryRecord,
};

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
    type Catalog = HeapDiscoveryCatalogTable;
    type Connections = HeapDiscoveredConnectionTable;
    type ReservedEndpoints = HeapDiscoveredEndpointSet;
}
