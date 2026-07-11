use crate::routing::announce::destination_announce_limit::{
    DestinationAnnounceLimit, DestinationAnnounceLimitAdmission, DestinationAnnounceLimitTable,
};
use crate::wire::DestinationHash;

#[derive(Debug)]
pub struct FixedDestinationAnnounceLimitTable<const MAX_ANNOUNCE_RATE_ENTRIES: usize> {
    len: usize,
    destinations: [DestinationHash; MAX_ANNOUNCE_RATE_ENTRIES],
    entries: [DestinationAnnounceLimit; MAX_ANNOUNCE_RATE_ENTRIES],
}

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize> Default
    for FixedDestinationAnnounceLimitTable<MAX_ANNOUNCE_RATE_ENTRIES>
{
    fn default() -> Self {
        Self {
            len: 0,
            destinations: [DestinationHash::new([0u8; 16]); MAX_ANNOUNCE_RATE_ENTRIES],
            entries: [DestinationAnnounceLimit::default(); MAX_ANNOUNCE_RATE_ENTRIES],
        }
    }
}

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize>
    FixedDestinationAnnounceLimitTable<MAX_ANNOUNCE_RATE_ENTRIES>
{
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

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize> DestinationAnnounceLimitTable
    for FixedDestinationAnnounceLimitTable<MAX_ANNOUNCE_RATE_ENTRIES>
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
    fn entries_mut(&mut self) -> &mut [DestinationAnnounceLimit] {
        &mut self.entries[..self.len]
    }

    fn insert(
        &mut self,
        destination: DestinationHash,
        entry: DestinationAnnounceLimit,
    ) -> DestinationAnnounceLimitAdmission {
        if MAX_ANNOUNCE_RATE_ENTRIES == 0 {
            return DestinationAnnounceLimitAdmission::Untrackable;
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
        DestinationAnnounceLimitAdmission::Recorded
    }
}
