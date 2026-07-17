use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::num::NonZeroU64;

use crate::units::InstantMillis;

use super::{
    discovered_interface_status, DiscoveredInterface, DiscoveredInterfaceId,
    DiscoveredInterfaceStatus,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiscoveryObservationCount(NonZeroU64);

impl DiscoveryObservationCount {
    pub const FIRST: Self = Self(NonZeroU64::MIN);

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    fn increment(&mut self) {
        self.0 = self.0.saturating_add(1);
    }
}

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

#[derive(Debug, Default)]
pub struct DiscoveryCatalog {
    records: BTreeMap<DiscoveredInterfaceId, DiscoveryRecord>,
}

impl DiscoveryCatalog {
    pub const fn new() -> Self {
        Self {
            records: BTreeMap::new(),
        }
    }

    pub fn observe(&mut self, interface: DiscoveredInterface) -> DiscoveryCatalogUpdate {
        let id = interface.id;
        let Some(record) = self.records.get_mut(&id) else {
            self.records.insert(id, DiscoveryRecord::first(interface));
            return DiscoveryCatalogUpdate::Added { id };
        };
        let received_at = interface.provenance.received_at;
        let last_heard = record.last_heard();
        if received_at < last_heard {
            return DiscoveryCatalogUpdate::IgnoredOutOfOrder {
                id,
                received_at,
                last_heard,
            };
        }
        let refresh = if record.interface.advertisement == interface.advertisement {
            DiscoveryCatalogRefresh::AdvertisementUnchanged
        } else {
            DiscoveryCatalogRefresh::AdvertisementChanged
        };
        record.interface = interface;
        record.observation_count.increment();
        DiscoveryCatalogUpdate::Refreshed { id, refresh }
    }

    pub fn get(&self, id: DiscoveredInterfaceId) -> Option<&DiscoveryRecord> {
        self.records.get(&id)
    }

    pub fn records(&self) -> impl Iterator<Item = &DiscoveryRecord> {
        self.records.values()
    }

    pub fn ranked_records(&self, now: InstantMillis) -> Vec<&DiscoveryRecord> {
        let mut records = self.records.values().collect::<Vec<_>>();
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
            .records
            .iter()
            .filter_map(|(id, record)| {
                matches!(record.status(now), DiscoveredInterfaceStatus::Expired).then_some(*id)
            })
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|id| self.records.remove(&id))
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
