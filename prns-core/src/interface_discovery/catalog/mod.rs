use alloc::vec::Vec;
use core::num::NonZeroU64;

use crate::identity::IdentityHash;
use crate::storage::TablePushError;
use crate::units::InstantMillis;

use super::{
    discovered_interface_status, DiscoveredInterface, DiscoveredInterfaceId,
    DiscoveredInterfaceStatus, DiscoveryCatalogTable, HeapDiscoveryCatalogTable,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiscoveryObservationCount(NonZeroU64);

impl DiscoveryObservationCount {
    pub const FIRST: Self = Self(NonZeroU64::MIN);

    pub const fn from_non_zero(count: NonZeroU64) -> Self {
        Self(count)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    fn increment(&mut self) {
        self.0 = self.0.saturating_add(1);
    }
}

#[derive(Debug, PartialEq)]
pub struct DiscoveryCatalogSeed {
    pub interface: DiscoveredInterface,
    pub first_heard: InstantMillis,
    pub observation_count: DiscoveryObservationCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryCatalogRestoreError {
    Duplicate(DiscoveredInterfaceId),
    CapacityReached(DiscoveredInterfaceId),
    FirstHeardAfterLastHeard {
        first_heard: InstantMillis,
        last_heard: InstantMillis,
    },
}

impl core::fmt::Display for DiscoveryCatalogRestoreError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Duplicate(id) => {
                write!(
                    formatter,
                    "discovery record {:?} is already restored",
                    id.as_bytes()
                )
            }
            Self::CapacityReached(id) => write!(
                formatter,
                "discovery catalog has no capacity for record {:?}",
                id.as_bytes()
            ),
            Self::FirstHeardAfterLastHeard {
                first_heard,
                last_heard,
            } => write!(
                formatter,
                "discovery first-heard time {} is after last-heard time {}",
                first_heard.0, last_heard.0
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DiscoveryCatalogRestoreError {}

#[derive(Debug, PartialEq)]
pub struct DiscoveryRecord {
    interface: DiscoveredInterface,
    first_heard: InstantMillis,
    observation_count: DiscoveryObservationCount,
}

impl DiscoveryRecord {
    fn first(interface: DiscoveredInterface) -> Self {
        Self {
            first_heard: interface.provenance.received_at,
            interface,
            observation_count: DiscoveryObservationCount::FIRST,
        }
    }

    pub const fn interface(&self) -> &DiscoveredInterface {
        &self.interface
    }

    pub const fn id(&self) -> DiscoveredInterfaceId {
        self.interface.id
    }

    pub const fn first_heard(&self) -> InstantMillis {
        self.first_heard
    }

    pub const fn last_heard(&self) -> InstantMillis {
        self.interface.provenance.received_at
    }

    pub const fn observation_count(&self) -> DiscoveryObservationCount {
        self.observation_count
    }

    pub const fn status(&self, now: InstantMillis) -> DiscoveredInterfaceStatus {
        discovered_interface_status(self.last_heard(), now)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryCatalogRefresh {
    AdvertisementUnchanged,
    AdvertisementChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryCatalogUpdate {
    Added {
        id: DiscoveredInterfaceId,
    },
    Refreshed {
        id: DiscoveredInterfaceId,
        refresh: DiscoveryCatalogRefresh,
    },
    IgnoredOutOfOrder {
        id: DiscoveredInterfaceId,
        received_at: InstantMillis,
        last_heard: InstantMillis,
    },
}

impl DiscoveryCatalogUpdate {
    pub const fn id(self) -> DiscoveredInterfaceId {
        match self {
            Self::Added { id }
            | Self::Refreshed { id, .. }
            | Self::IgnoredOutOfOrder { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryCatalogStoreError {
    CapacityReached(DiscoveredInterfaceId),
}

impl core::fmt::Display for DiscoveryCatalogStoreError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CapacityReached(id) => write!(
                formatter,
                "discovery catalog has no capacity for record {:?}",
                id.as_bytes()
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DiscoveryCatalogStoreError {}

#[derive(Debug)]
pub struct DiscoveryCatalog<T: DiscoveryCatalogTable = HeapDiscoveryCatalogTable> {
    records: T,
}

impl<T: DiscoveryCatalogTable> Default for DiscoveryCatalog<T> {
    fn default() -> Self {
        Self {
            records: T::default(),
        }
    }
}

impl DiscoveryCatalog<HeapDiscoveryCatalogTable> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<T: DiscoveryCatalogTable> DiscoveryCatalog<T> {
    pub fn with_table(records: T) -> Self {
        Self { records }
    }

    pub fn observe(
        &mut self,
        interface: DiscoveredInterface,
    ) -> Result<DiscoveryCatalogUpdate, DiscoveryCatalogStoreError> {
        let id = interface.id;
        let Some(record) = self.records.get_mut(id) else {
            let previous = self
                .records
                .try_insert(id, DiscoveryRecord::first(interface))
                .map_err(|TablePushError::TableFull| {
                    DiscoveryCatalogStoreError::CapacityReached(id)
                })?;
            debug_assert!(previous.is_none());
            return Ok(DiscoveryCatalogUpdate::Added { id });
        };
        let received_at = interface.provenance.received_at;
        let last_heard = record.last_heard();
        if received_at < last_heard {
            return Ok(DiscoveryCatalogUpdate::IgnoredOutOfOrder {
                id,
                received_at,
                last_heard,
            });
        }
        let refresh = if record.interface.advertisement == interface.advertisement {
            DiscoveryCatalogRefresh::AdvertisementUnchanged
        } else {
            DiscoveryCatalogRefresh::AdvertisementChanged
        };
        record.interface = interface;
        record.observation_count.increment();
        Ok(DiscoveryCatalogUpdate::Refreshed { id, refresh })
    }

    pub fn restore(
        &mut self,
        seed: DiscoveryCatalogSeed,
    ) -> Result<(), DiscoveryCatalogRestoreError> {
        let id = seed.interface.id;
        let last_heard = seed.interface.provenance.received_at;
        if seed.first_heard > last_heard {
            return Err(DiscoveryCatalogRestoreError::FirstHeardAfterLastHeard {
                first_heard: seed.first_heard,
                last_heard,
            });
        }
        if self.records.get(id).is_some() {
            return Err(DiscoveryCatalogRestoreError::Duplicate(id));
        }
        let previous = self
            .records
            .try_insert(
                id,
                DiscoveryRecord {
                    interface: seed.interface,
                    first_heard: seed.first_heard,
                    observation_count: seed.observation_count,
                },
            )
            .map_err(|TablePushError::TableFull| {
                DiscoveryCatalogRestoreError::CapacityReached(id)
            })?;
        debug_assert!(previous.is_none());
        Ok(())
    }

    pub fn get(&self, id: DiscoveredInterfaceId) -> Option<&DiscoveryRecord> {
        self.records.get(id)
    }

    pub fn records(&self) -> T::Records<'_> {
        self.records.records()
    }

    pub fn ranked_records(&self, now: InstantMillis) -> Vec<&DiscoveryRecord> {
        let mut records = self.records.records().collect::<Vec<_>>();
        records.sort_by(|left, right| {
            status_priority(right.status(now))
                .cmp(&status_priority(left.status(now)))
                .then_with(|| right.interface.stamp_value.cmp(&left.interface.stamp_value))
                .then_with(|| right.last_heard().cmp(&left.last_heard()))
                .then_with(|| left.id().cmp(&right.id()))
        });
        records
    }

    pub fn remove_expired(&mut self, now: InstantMillis) -> Vec<DiscoveryRecord> {
        let expired = self
            .records()
            .filter_map(|record| {
                matches!(record.status(now), DiscoveredInterfaceStatus::Expired)
                    .then_some(record.id())
            })
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|id| self.records.remove(id))
            .collect()
    }

    pub fn remove_below_stamp_cost(&mut self, required: super::StampCost) -> Vec<DiscoveryRecord> {
        let below_cost = self
            .records()
            .filter_map(|record| {
                (record.interface().stamp_value.get() < u16::from(required.get()))
                    .then_some(record.id())
            })
            .collect::<Vec<_>>();
        below_cost
            .into_iter()
            .filter_map(|id| self.records.remove(id))
            .collect()
    }

    pub fn remove_blackholed(&mut self, identities: &[IdentityHash]) -> Vec<DiscoveryRecord> {
        let blackholed = self
            .records()
            .filter_map(|record| {
                let interface = record.interface();
                let transport =
                    IdentityHash::new(*interface.advertisement.transport.transport_id().as_bytes());
                (identities.contains(&interface.provenance.announced_by)
                    || identities.contains(&transport))
                .then_some(record.id())
            })
            .collect::<Vec<_>>();
        blackholed
            .into_iter()
            .filter_map(|id| self.records.remove(id))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

const fn status_priority(status: DiscoveredInterfaceStatus) -> u8 {
    match status {
        DiscoveredInterfaceStatus::Available => 3,
        DiscoveredInterfaceStatus::Unknown => 2,
        DiscoveredInterfaceStatus::Stale => 1,
        DiscoveredInterfaceStatus::Expired => 0,
    }
}

#[cfg(test)]
mod tests;
