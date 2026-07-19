mod learning;
mod lifetime;
mod lookup;
mod model;
mod persistence;
mod removal;
mod updates;

pub use model::RoutingTable;

use super::announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecordTable};
use super::announce::Announce;
use super::route_expiry::RouteExpiryIndex;
use super::routes::RouteTable;
use super::types::StoredAnnounce;
use crate::wire::DestinationHash;

impl<R, A, H, D, I> RoutingTable<R, A, H, D, I>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
    I: RouteExpiryIndex,
{
    pub fn app_data_for(&self, destination: &DestinationHash) -> Option<&[u8]> {
        Some(self.stored_announce_for(destination)?.announce.app_data)
    }

    pub fn stored_announce_for(&self, destination: &DestinationHash) -> Option<StoredAnnounce<'_>> {
        let i = self.index_of(destination)?;
        let handle = self.announce_records.app_data_handles()[i]?;
        let app_data = self.announce_app_data.get(handle);
        Some(StoredAnnounce {
            hops: self.routes.hops()[i],
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
            announce: Announce {
                destination: self.routes.destinations()[i],
                public_keys: self.announce_records.public_keys()[i],
                dotted_name_hash: self.announce_records.dotted_name_hashes()[i],
                announce_id: self.announce_records.announce_ids()[i],
                ratchet: self.announce_records.ratchets()[i],
                signature: self.announce_records.signatures()[i],
                app_data,
            },
        })
    }
}

#[cfg(test)]
mod tests;
