use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::future::Future;

use prns_core::engine::{AnnounceRateState, RouteSnapshot};
use prns_core::interfaces::PacketPhyStats;
use prns_core::routing::announce::AnnounceRateAccounting;
use prns_core::routing::dedup::PacketHash;
use prns_core::units::InstantMillis;
use prns_core::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};

use super::super::{
    fold_logical_interface_inventory, InterfaceInventoryEntry, InterfaceTimingSnapshot,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceRateSnapshot {
    pub destination: DestinationHash,
    pub last_allowed_announce_at: InstantMillis,
    pub blocked_until: InstantMillis,
    pub rate_violations: u16,
    pub observed_at: Vec<InstantMillis>,
}

const MAX_ANNOUNCE_RATE_OBSERVATIONS: usize = 16;

#[derive(Default)]
pub struct HeapAnnounceRateHistory {
    observed_at: BTreeMap<AnnounceRateHistoryKey, VecDeque<InstantMillis>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AnnounceRateHistoryKey([u8; TRUNCATED_HASH_BYTE_LEN]);

impl From<DestinationHash> for AnnounceRateHistoryKey {
    fn from(destination: DestinationHash) -> Self {
        Self(*destination.as_bytes())
    }
}

impl HeapAnnounceRateHistory {
    pub fn record(
        &mut self,
        destination: DestinationHash,
        observed_at: InstantMillis,
        accounting: AnnounceRateAccounting,
    ) {
        match accounting {
            AnnounceRateAccounting::NotApplied => return,
            AnnounceRateAccounting::Started => {
                self.observed_at.insert(destination.into(), VecDeque::new());
            }
            AnnounceRateAccounting::Continued => {}
        }
        let history = self.observed_at.entry(destination.into()).or_default();
        if history.len() == MAX_ANNOUNCE_RATE_OBSERVATIONS {
            history.pop_front();
        }
        history.push_back(observed_at);
    }

    #[must_use]
    pub fn snapshot(&self, state: AnnounceRateState) -> AnnounceRateSnapshot {
        AnnounceRateSnapshot {
            destination: state.destination,
            last_allowed_announce_at: state.last_allowed_announce_at,
            blocked_until: state.blocked_until,
            rate_violations: state.rate_violations,
            observed_at: self
                .observed_at
                .get(&state.destination.into())
                .map(|history| history.iter().copied().collect())
                .unwrap_or_default(),
        }
    }
}

pub trait NodeIntrospection {
    fn interface_inventory(&self) -> Vec<InterfaceInventoryEntry<String>>;

    fn interface_timing_inventory(&self) -> Vec<InterfaceTimingSnapshot> {
        Vec::new()
    }

    fn link_count(&self) -> impl Future<Output = u32> + Send;

    fn packet_phy(&self, packet_hash: PacketHash) -> Option<PacketPhyStats>;

    fn announce_rates(&self) -> impl Future<Output = Vec<AnnounceRateSnapshot>> + Send;

    fn routes(&self) -> impl Future<Output = Vec<RouteSnapshot>> + Send;

    fn route(
        &self,
        destination: DestinationHash,
    ) -> impl Future<Output = Option<RouteSnapshot>> + Send;
}

#[must_use]
pub fn logical_interface_inventory<Label: Clone + Ord>(
    mut inventory: Vec<InterfaceInventoryEntry<Label>>,
) -> Vec<InterfaceInventoryEntry<Label>> {
    let logical_len = fold_logical_interface_inventory(&mut inventory).len();
    inventory.truncate(logical_len);
    inventory
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_rate_history_is_bounded_and_restartable() {
        let destination = DestinationHash::new([0x42; 16]);
        let mut history = HeapAnnounceRateHistory::default();
        history.record(
            destination,
            InstantMillis(99),
            AnnounceRateAccounting::Started,
        );
        for observed_at in 0..20 {
            history.record(
                destination,
                InstantMillis(observed_at),
                AnnounceRateAccounting::Continued,
            );
        }

        let snapshot = history.snapshot(AnnounceRateState {
            destination,
            last_allowed_announce_at: InstantMillis(19),
            blocked_until: InstantMillis(0),
            rate_violations: 0,
        });

        assert_eq!(
            snapshot.observed_at,
            (4..20).map(InstantMillis).collect::<Vec<_>>()
        );

        history.record(
            destination,
            InstantMillis(25),
            AnnounceRateAccounting::Started,
        );

        assert_eq!(
            history
                .snapshot(AnnounceRateState {
                    destination,
                    last_allowed_announce_at: InstantMillis(25),
                    blocked_until: InstantMillis(0),
                    rate_violations: 0,
                })
                .observed_at,
            vec![InstantMillis(25)]
        );
    }
}
