use alloc::vec::Vec;

use crate::routing::announce::announce_rate::{
    AnnounceRateColumns, AnnounceRateEntry, RateEntryAdmission,
};
use crate::wire::DestinationHash;

/// A daemon-grade ceiling on tracked rate entries — far above any realistic
/// count of distinct destinations a node rebroadcasts, a backstop against
/// unbounded growth matching the other engine tables' hygiene.
pub const DEFAULT_MAX_ANNOUNCE_RATE_ENTRIES: usize = 1024;

#[derive(Debug, Default)]
pub struct HeapAnnounceRateColumns {
    destinations: Vec<DestinationHash>,
    entries: Vec<AnnounceRateEntry>,
}

impl HeapAnnounceRateColumns {
    fn least_recently_active(&self) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.last_allowed_announce_at.0)
            .map(|(index, _)| index)
    }
}

impl AnnounceRateColumns for HeapAnnounceRateColumns {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_ANNOUNCE_RATE_ENTRIES
    }
    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }
    fn entries_mut(&mut self) -> &mut [AnnounceRateEntry] {
        &mut self.entries
    }

    fn insert(
        &mut self,
        destination: DestinationHash,
        entry: AnnounceRateEntry,
    ) -> RateEntryAdmission {
        if self.destinations.len() >= DEFAULT_MAX_ANNOUNCE_RATE_ENTRIES {
            if let Some(victim) = self.least_recently_active() {
                self.destinations.remove(victim);
                self.entries.remove(victim);
            }
        }
        self.destinations.push(destination);
        self.entries.push(entry);
        RateEntryAdmission::Recorded
    }
}
