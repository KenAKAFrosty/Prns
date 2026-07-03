use crate::engine::EngineState;
#[cfg(feature = "alloc")]
use crate::interfaces::InterfaceId;
#[cfg(feature = "alloc")]
use crate::routing::types::NextHop;
use crate::storage::StorageLayout;
#[cfg(feature = "alloc")]
use crate::units::InstantMillis;
#[cfg(feature = "alloc")]
use crate::wire::DestinationHash;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// RNS `Reticulum.rpc_loop`'s read-only queries, demuxed onto the command lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcQuery {
    /// `get_link_count` — the number of live links the node carries.
    LinkCount,
    /// `get_path_table` — every known destination, how it is reached, and when it was learned.
    #[cfg(feature = "alloc")]
    PathTable,
    /// `get_next_hop` / `get_next_hop_if_name` — the one route to a destination, if known.
    #[cfg(feature = "alloc")]
    Route(DestinationHash),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcQueryResult {
    LinkCount(u32),
    #[cfg(feature = "alloc")]
    PathTable(Vec<RpcPathEntry>),
    #[cfg(feature = "alloc")]
    Route(Option<RpcPathEntry>),
}

/// Rendered by the RPC shim to RNS's path-table dict
/// (`hash`, `via`, `hops`, `timestamp`, `expires`, `interface`).
#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcPathEntry {
    pub destination: DestinationHash,
    pub hops: u8,
    pub via: NextHop,
    pub learned_at: InstantMillis,
    pub interface: InterfaceId,
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn run_rpc_query(&self, query: RpcQuery) -> RpcQueryResult {
        match query {
            RpcQuery::LinkCount => RpcQueryResult::LinkCount(self.links.len() as u32),
            #[cfg(feature = "alloc")]
            RpcQuery::PathTable => RpcQueryResult::PathTable(
                self.routing_table
                    .path_rows()
                    .map(|(destination, entry)| RpcPathEntry {
                        destination,
                        hops: entry.hops,
                        via: entry.next_hop,
                        learned_at: entry.learned_at,
                        interface: entry.receiving_interface,
                    })
                    .collect(),
            ),
            #[cfg(feature = "alloc")]
            RpcQuery::Route(destination) => {
                RpcQueryResult::Route(self.routing_table.path_row(&destination).map(|entry| {
                    RpcPathEntry {
                        destination,
                        hops: entry.hops,
                        via: entry.next_hop,
                        learned_at: entry.learned_at,
                        interface: entry.receiving_interface,
                    }
                }))
            }
        }
    }
}
