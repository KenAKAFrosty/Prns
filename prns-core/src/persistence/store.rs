use super::SnapshotRegion;
pub use crate::identity::vault::Removal;

/// Where snapshots sleep between boots.
/// The engine's tables stay the truth: a host flushes sealed snapshots here and reads them back at the next boot to seed the tables.
pub trait PersistedStore {
    type Error;

    fn stored_len(&self, region: SnapshotRegion) -> Result<Option<usize>, Self::Error>;

    /// A `buf` shorter than the stored snapshot is the impl's error, never a silent truncation.
    fn load<'b>(
        &self,
        region: SnapshotRegion,
        buf: &'b mut [u8],
    ) -> Result<Option<&'b [u8]>, Self::Error>;

    fn store(&mut self, region: SnapshotRegion, snapshot: &[u8]) -> Result<(), Self::Error>;

    fn remove(&mut self, region: SnapshotRegion) -> Result<Removal, Self::Error>;
}
