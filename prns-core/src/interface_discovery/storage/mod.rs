mod fixed;
mod growable;

pub use fixed::FixedDiscoveryValidationCache;
pub use growable::{
    GrowableInterfaceDiscoveryStorage, HeapDiscoveredConnectionTable, HeapDiscoveredEndpointSet,
    HeapDiscoveryCatalogTable, HeapDiscoveryValidationCache, RNS_VALIDATION_CACHE_CAPACITY,
};

use crate::interfaces::InterfaceId;
use crate::storage::TablePushError;

use super::{
    ActiveDiscoveredInterface, DiscoveredConnectionEndpointId, DiscoveredInterfaceId,
    DiscoveryRecord, StampValue,
};

/// Lemire-index bucket count that keeps a fixed validation cache at or below 2/3 load.
pub const fn discovery_validation_index_buckets(entries: usize) -> usize {
    crate::lemire_index::buckets_for_two_thirds_load(entries)
}

/// The memoization residence behind discovery stamp validation. Implementations may use
/// growable host storage, fixed inline storage, external memory, or zero capacity; cache misses
/// change only validation work, never acceptance semantics.
pub trait DiscoveryValidationCache: Default {
    fn valid(&self, payload_hash: &[u8; 32]) -> Option<(&[u8], StampValue)>;
    fn insufficient(&self, payload_hash: &[u8; 32]) -> Option<StampValue>;
    fn remember_valid(
        &mut self,
        payload_hash: [u8; 32],
        packed_advertisement: &[u8],
        stamp_value: StampValue,
    );
    fn remember_insufficient(&mut self, payload_hash: [u8; 32], stamp_value: StampValue);
}

pub trait DiscoveryCatalogTable: Default {
    type Records<'a>: Iterator<Item = &'a DiscoveryRecord>
    where
        Self: 'a;

    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get(&self, id: DiscoveredInterfaceId) -> Option<&DiscoveryRecord>;
    fn get_mut(&mut self, id: DiscoveredInterfaceId) -> Option<&mut DiscoveryRecord>;
    fn try_insert(
        &mut self,
        id: DiscoveredInterfaceId,
        record: DiscoveryRecord,
    ) -> Result<Option<DiscoveryRecord>, TablePushError>;
    fn remove(&mut self, id: DiscoveredInterfaceId) -> Option<DiscoveryRecord>;
    fn records(&self) -> Self::Records<'_>;
}

pub trait DiscoveredConnectionTable: Default {
    type Connections<'a>: Iterator<Item = &'a ActiveDiscoveredInterface>
    where
        Self: 'a;

    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get_mut(&mut self, interface: InterfaceId) -> Option<&mut ActiveDiscoveredInterface>;
    fn contains_interface(&self, interface: InterfaceId) -> bool;
    fn contains_endpoint(&self, endpoint: DiscoveredConnectionEndpointId) -> bool;
    fn try_insert(
        &mut self,
        interface: ActiveDiscoveredInterface,
    ) -> Result<Option<ActiveDiscoveredInterface>, TablePushError>;
    fn remove(&mut self, interface: InterfaceId) -> Option<ActiveDiscoveredInterface>;
    fn connections(&self) -> Self::Connections<'_>;
}

pub trait DiscoveredEndpointSet: Default {
    type Endpoints<'a>: Iterator<Item = DiscoveredConnectionEndpointId>
    where
        Self: 'a;

    fn try_insert(
        &mut self,
        endpoint: DiscoveredConnectionEndpointId,
    ) -> Result<bool, TablePushError>;
    fn endpoints(&self) -> Self::Endpoints<'_>;
}

pub trait InterfaceDiscoveryStorage {
    type ValidationCache: DiscoveryValidationCache;
    type Catalog: DiscoveryCatalogTable;
    type Connections: DiscoveredConnectionTable;
    type ReservedEndpoints: DiscoveredEndpointSet;
}
