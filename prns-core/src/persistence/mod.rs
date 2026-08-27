//! Memory-first persistence of network-learned state.
//! The engine's tables are the truth; a host flushes sealed snapshots of them to a [`PersistedStore`] and seeds the tables from it at the next boot; destination identities deliberately retain RNS's historical `known_destinations` filename and their existing Prns region tag.
//! Config-derived state (held identities, registered destinations, group keys) is never snapshotted — the identity vault and the host recipe re-supply it at boot.

mod destination_identities;
pub mod envelope;
mod impls;
mod remote_control_access;
mod routing_table;
mod self_ratchets;
mod store;
mod timebase;
mod tunnels;

#[cfg(feature = "parallel-persistence")]
const PARALLEL_PERSISTENCE_MIN_BYTES: usize = 512 * 1024;

#[cfg(feature = "parallel-persistence")]
fn should_parallelize_persistence(snapshot_len: usize) -> bool {
    snapshot_len >= PARALLEL_PERSISTENCE_MIN_BYTES && rayon::current_num_threads() > 1
}

#[cfg(all(test, feature = "parallel-persistence"))]
mod parallel_tests {
    use super::*;

    #[test]
    fn worker_count_and_snapshot_size_select_the_writer() {
        let one_worker = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let two_workers = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        assert!(!one_worker
            .install(|| { should_parallelize_persistence(PARALLEL_PERSISTENCE_MIN_BYTES) }));
        assert!(!two_workers
            .install(|| { should_parallelize_persistence(PARALLEL_PERSISTENCE_MIN_BYTES - 1) }));
        assert!(two_workers
            .install(|| { should_parallelize_persistence(PARALLEL_PERSISTENCE_MIN_BYTES) }));
    }
}

pub use destination_identities::{
    destination_identities_snapshot_len, persisted_destination_identity_wire_len,
    read_destination_identities_snapshot, write_destination_identities_snapshot,
    DestinationIdentitiesSnapshotWriteError, PersistedDestinationIdentityRows,
};
pub use envelope::{
    open_snapshot, seal_snapshot, seal_snapshot_in_place, snapshot_fingerprint,
    SnapshotFingerprint, SnapshotOpenError, SnapshotSealError, SNAPSHOT_OVERHEAD_LEN,
};
#[allow(unused_imports)]
pub use impls::*;
pub use remote_control_access::{
    read_remote_control_access_snapshot, remote_control_access_snapshot_len,
    write_remote_control_access_snapshot, PersistedRemoteControlControllerIdentities,
    REMOTE_CONTROL_CONTROLLER_IDENTITY_WIRE_LEN,
};
pub use routing_table::{
    maximum_persisted_route_row_wire_len, maximum_route_upsert_payload_len,
    persisted_route_row_wire_len, read_routing_table_snapshot, routing_table_snapshot_len,
    write_routing_table_snapshot, PersistedRouteRows, RoutingTableSnapshotWriteError,
};
pub use self_ratchets::{
    read_self_ratchets_snapshot, self_ratchets_snapshot_len, write_self_ratchets_snapshot,
    PersistedSelfRatchets,
};
pub use store::{PersistedStore, Removal};
pub use timebase::{read_timebase_snapshot, write_timebase_snapshot, TIMEBASE_SNAPSHOT_LEN};
pub use tunnels::{
    read_tunnels_snapshot, tunnels_snapshot_len, write_tunnels_snapshot, PersistedTunnelRows,
    TUNNEL_ROW_WIRE_LEN,
};

/// `SelfRatchets` blobs carry secrets, so they ride the identity vault rather than a
/// [`PersistedStore`] — the region tag still seals them against cross-region mixups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotRegion {
    Timebase,
    RoutingTable,
    Tunnels,
    SelfRatchets,
    DestinationIdentities,
    RemoteControlAccess,
}

impl SnapshotRegion {
    pub const fn tag(self) -> u8 {
        match self {
            SnapshotRegion::Timebase => 0x01,
            SnapshotRegion::RoutingTable => 0x02,
            SnapshotRegion::Tunnels => 0x03,
            SnapshotRegion::SelfRatchets => 0x04,
            SnapshotRegion::DestinationIdentities => 0x05,
            SnapshotRegion::RemoteControlAccess => 0x06,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotReadError {
    Envelope(SnapshotOpenError),
    MalformedPayload,
}
