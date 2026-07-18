use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::string::String;
use std::vec::Vec;

use tokio::sync::oneshot;

use crate::engine::{AnnounceRateState, InstantMillis};
use crate::interfaces::PacketPhyStats;
use crate::routing::announce::AnnounceRateAccounting;
use crate::routing::dedup::PacketHash;
use crate::wire::DestinationHash;

pub use crate::engine::RouteSnapshot;

type CoreInterfaceIfacSnapshot =
    prns_runtime::runtime::node_introspection::InterfaceIfacSnapshot<String>;
type CoreInterfaceInventoryEntry =
    prns_runtime::runtime::node_introspection::InterfaceInventoryEntry<String>;

pub use prns_runtime::runtime::node_introspection::{
    logical_interface_inventory, AnnounceRateSnapshot,
};
pub type InterfaceIfacSnapshot = CoreInterfaceIfacSnapshot;
pub type InterfaceInventoryEntry = CoreInterfaceInventoryEntry;

pub trait NodeIntrospection {
    fn interface_inventory(&self) -> Vec<InterfaceInventoryEntry>;

    fn link_count(&self) -> impl Future<Output = u32> + Send;

    fn packet_phy(&self, packet_hash: PacketHash) -> Option<PacketPhyStats>;

    fn announce_rates(&self) -> impl Future<Output = Vec<AnnounceRateSnapshot>> + Send;

    fn routes(&self) -> impl Future<Output = Vec<RouteSnapshot>> + Send;

    fn route(
        &self,
        destination: DestinationHash,
    ) -> impl Future<Output = Option<RouteSnapshot>> + Send;
}

pub enum NodeIntrospectionRequest {
    LinkCount {
        reply: oneshot::Sender<u32>,
    },
    AnnounceRates {
        reply: oneshot::Sender<Vec<AnnounceRateSnapshot>>,
    },
    Routes {
        reply: oneshot::Sender<Vec<RouteSnapshot>>,
    },
    Route {
        destination: DestinationHash,
        reply: oneshot::Sender<Option<RouteSnapshot>>,
    },
}

const MAX_ANNOUNCE_RATE_OBSERVATIONS: usize = 16;

#[derive(Default)]
pub(crate) struct AnnounceRateHistory {
    observed_at: BTreeMap<AnnounceRateHistoryKey, VecDeque<InstantMillis>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AnnounceRateHistoryKey([u8; crate::wire::TRUNCATED_HASH_BYTE_LEN]);

impl From<DestinationHash> for AnnounceRateHistoryKey {
    fn from(destination: DestinationHash) -> Self {
        Self(*destination.as_bytes())
    }
}

impl AnnounceRateHistory {
    pub(crate) fn record(
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

    pub(crate) fn snapshot(&self, state: AnnounceRateState) -> AnnounceRateSnapshot {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_rate_history_is_bounded_and_restartable() {
        let destination = DestinationHash::new([0x42; 16]);
        let mut history = AnnounceRateHistory::default();
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
