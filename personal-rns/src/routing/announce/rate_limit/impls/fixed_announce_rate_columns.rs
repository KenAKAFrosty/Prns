use crate::routing::announce::rate_limit::{
    AnnounceRateColumns, AnnounceRateEntry, RateEntryAdmission,
};
use crate::wire::DestinationHash;

/// A fixed-capacity rate table. There is at most one entry per tracked
/// destination, so a recipe sizes this by its route ceiling and eviction is a
/// churn-only edge; when it does fire it drops the least-recently-active entry.
#[derive(Debug)]
pub struct FixedAnnounceRateColumns<const MAX_ANNOUNCE_RATE_ENTRIES: usize> {
    len: usize,
    destinations: [DestinationHash; MAX_ANNOUNCE_RATE_ENTRIES],
    entries: [AnnounceRateEntry; MAX_ANNOUNCE_RATE_ENTRIES],
}

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize> Default
    for FixedAnnounceRateColumns<MAX_ANNOUNCE_RATE_ENTRIES>
{
    fn default() -> Self {
        Self {
            len: 0,
            destinations: [DestinationHash::new([0u8; 16]); MAX_ANNOUNCE_RATE_ENTRIES],
            entries: [AnnounceRateEntry::default(); MAX_ANNOUNCE_RATE_ENTRIES],
        }
    }
}

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize> FixedAnnounceRateColumns<MAX_ANNOUNCE_RATE_ENTRIES> {
    fn least_recently_active(&self) -> usize {
        let mut victim = 0;
        for index in 1..self.len {
            if self.entries[index].last_allowed_announce_at.0
                < self.entries[victim].last_allowed_announce_at.0
            {
                victim = index;
            }
        }
        victim
    }
}

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize> AnnounceRateColumns
    for FixedAnnounceRateColumns<MAX_ANNOUNCE_RATE_ENTRIES>
{
    fn capacity(&self) -> usize {
        MAX_ANNOUNCE_RATE_ENTRIES
    }
    fn len(&self) -> usize {
        self.len
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations[..self.len]
    }
    fn entries_mut(&mut self) -> &mut [AnnounceRateEntry] {
        &mut self.entries[..self.len]
    }

    fn insert(
        &mut self,
        destination: DestinationHash,
        entry: AnnounceRateEntry,
    ) -> RateEntryAdmission {
        if MAX_ANNOUNCE_RATE_ENTRIES == 0 {
            return RateEntryAdmission::Untrackable;
        }
        let index = if self.len < MAX_ANNOUNCE_RATE_ENTRIES {
            let i = self.len;
            self.len += 1;
            i
        } else {
            self.least_recently_active()
        };
        self.destinations[index] = destination;
        self.entries[index] = entry;
        RateEntryAdmission::Recorded
    }
}
