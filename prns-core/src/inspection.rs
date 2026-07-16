use std::string::String;
use std::vec::Vec;

pub use crate::engine::{AnnounceRateSnapshot, RouteSnapshot};
use crate::interfaces::ifac::IfacSize;
use crate::interfaces::{InterfaceSnapshot, PacketPhyStats};
use crate::routing::dedup::PacketHash;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceIfacSnapshot {
    pub signature: [u8; 64],
    pub size: IfacSize,
    pub network_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceInventoryEntry {
    pub snapshot: InterfaceSnapshot,
    pub ifac: Option<InterfaceIfacSnapshot>,
}

pub trait InspectionSource {
    fn interface_inventory(&self) -> Vec<InterfaceInventoryEntry>;

    fn link_count(&self) -> impl core::future::Future<Output = u32> + Send;

    fn packet_phy(&self, packet_hash: PacketHash) -> Option<PacketPhyStats>;

    fn announce_rates(
        &self,
    ) -> impl core::future::Future<Output = Vec<AnnounceRateSnapshot>> + Send;

    fn routes(&self) -> impl core::future::Future<Output = Vec<RouteSnapshot>> + Send;

    fn route(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<Output = Option<RouteSnapshot>> + Send;
}
