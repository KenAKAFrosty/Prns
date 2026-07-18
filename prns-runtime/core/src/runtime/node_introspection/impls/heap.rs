use alloc::string::String;
use alloc::vec::Vec;
use core::future::Future;

use prns_core::engine::RouteSnapshot;
use prns_core::interfaces::PacketPhyStats;
use prns_core::routing::dedup::PacketHash;
use prns_core::units::InstantMillis;
use prns_core::wire::DestinationHash;

use super::super::{fold_logical_interface_inventory, InterfaceInventoryEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceRateSnapshot {
    pub destination: DestinationHash,
    pub last_allowed_announce_at: InstantMillis,
    pub blocked_until: InstantMillis,
    pub rate_violations: u16,
    pub observed_at: Vec<InstantMillis>,
}

pub trait NodeIntrospection {
    fn interface_inventory(&self) -> Vec<InterfaceInventoryEntry<String>>;

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
pub fn logical_interface_inventory<Label: Ord>(
    mut inventory: Vec<InterfaceInventoryEntry<Label>>,
) -> Vec<InterfaceInventoryEntry<Label>> {
    let logical_len = fold_logical_interface_inventory(&mut inventory).len();
    inventory.truncate(logical_len);
    inventory
}
