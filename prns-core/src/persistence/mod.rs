//! Memory-first persistence of network-learned state.
//! The engine's tables are the truth; a host flushes sealed snapshots of them to a [`PersistedStore`] and seeds the tables from it at the next boot.
//! Config-derived state (held identities, registered destinations, group keys) is never snapshotted — the identity vault and the host recipe re-supply it at boot.

pub mod envelope;
mod impls;
mod routing_table;
mod store;
mod timebase;
mod tunnels;

pub use envelope::{
    open_snapshot, seal_snapshot, seal_snapshot_in_place, SnapshotOpenError, SnapshotSealError,
    SNAPSHOT_OVERHEAD_LEN,
};
#[allow(unused_imports)]
pub use impls::*;
pub use routing_table::{
    persisted_route_row_wire_len, read_routing_table_snapshot, routing_table_snapshot_len,
    write_routing_table_snapshot, PersistedRouteRows, RoutingTableSnapshotWriteError,
};
pub use store::{PersistedStore, Removal};
pub use timebase::{read_timebase_snapshot, write_timebase_snapshot, TIMEBASE_SNAPSHOT_LEN};
pub use tunnels::{
    read_tunnels_snapshot, tunnels_snapshot_len, write_tunnels_snapshot, PersistedTunnelRows,
    TUNNEL_ROW_WIRE_LEN,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotRegion {
    Timebase,
    RoutingTable,
    Tunnels,
}

impl SnapshotRegion {
    pub const fn tag(self) -> u8 {
        match self {
            SnapshotRegion::Timebase => 0x01,
            SnapshotRegion::RoutingTable => 0x02,
            SnapshotRegion::Tunnels => 0x03,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotReadError {
    Envelope(SnapshotOpenError),
    MalformedPayload,
}
