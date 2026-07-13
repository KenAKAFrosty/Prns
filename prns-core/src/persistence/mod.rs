//! Memory-first persistence of network-learned state.
//! The engine's tables are the truth; a host flushes sealed snapshots of them to a [`PersistedStore`] and seeds the tables from it at the next boot.
//! Config-derived state (held identities, registered destinations, group keys) is never snapshotted — the identity vault and the host recipe re-supply it at boot.

pub mod envelope;
mod impls;
mod store;
mod timebase;

pub use envelope::{
    open_snapshot, seal_snapshot, SnapshotOpenError, SnapshotSealError, SNAPSHOT_OVERHEAD_LEN,
};
#[allow(unused_imports)]
pub use impls::*;
pub use store::{PersistedStore, Removal};
pub use timebase::{
    read_timebase_snapshot, write_timebase_snapshot, SnapshotReadError, TIMEBASE_SNAPSHOT_LEN,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotRegion {
    Timebase,
}

impl SnapshotRegion {
    pub const fn tag(self) -> u8 {
        match self {
            SnapshotRegion::Timebase => 0x01,
        }
    }
}
