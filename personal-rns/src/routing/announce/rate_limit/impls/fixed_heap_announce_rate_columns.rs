//! The fixed-capacity, heap-backed twin of [`FixedAnnounceRateColumns`]: the
//! destination and entry columns live in a caller-chosen heap region (PSRAM on
//! the S3) via the allocator `A`. There is at most one entry per tracked
//! destination, so a recipe sizes this by its route ceiling; eviction drops the
//! least-recently-active entry, exactly as the inline twin does.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::routing::announce::rate_limit::{
    AnnounceRateColumns, AnnounceRateEntry, RateEntryAdmission,
};
use crate::wire::DestinationHash;

fn filled<T: Clone, A: Allocator>(value: T, len: usize, alloc: A) -> Box<[T], A> {
    let mut column = Vec::with_capacity_in(len, alloc);
    column.resize(len, value);
    column.into_boxed_slice()
}

pub struct FixedHeapAnnounceRateColumns<
    const MAX_ANNOUNCE_RATE_ENTRIES: usize,
    A: Allocator = Global,
> {
    len: usize,
    destinations: Box<[DestinationHash], A>,
    entries: Box<[AnnounceRateEntry], A>,
}

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize, A: Allocator + Default> Default
    for FixedHeapAnnounceRateColumns<MAX_ANNOUNCE_RATE_ENTRIES, A>
{
    fn default() -> Self {
        Self {
            len: 0,
            destinations: filled(
                DestinationHash::new([0u8; 16]),
                MAX_ANNOUNCE_RATE_ENTRIES,
                A::default(),
            ),
            entries: filled(
                AnnounceRateEntry::default(),
                MAX_ANNOUNCE_RATE_ENTRIES,
                A::default(),
            ),
        }
    }
}

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize, A: Allocator>
    FixedHeapAnnounceRateColumns<MAX_ANNOUNCE_RATE_ENTRIES, A>
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

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize, A: Allocator> AnnounceRateColumns
    for FixedHeapAnnounceRateColumns<MAX_ANNOUNCE_RATE_ENTRIES, A>
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::InstantMillis;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn entry_at(ms: u64) -> AnnounceRateEntry {
        let mut entry = AnnounceRateEntry::default();
        entry.last_allowed_announce_at = InstantMillis(ms);
        entry
    }

    #[test]
    fn records_until_full_then_evicts_the_least_recently_active() {
        let mut columns = FixedHeapAnnounceRateColumns::<2>::default();
        assert_eq!(columns.capacity(), 2);
        assert_eq!(
            columns.insert(dest(1), entry_at(100)),
            RateEntryAdmission::Recorded
        );
        assert_eq!(
            columns.insert(dest(2), entry_at(200)),
            RateEntryAdmission::Recorded
        );
        assert_eq!(columns.len(), 2);

        assert_eq!(
            columns.insert(dest(3), entry_at(300)),
            RateEntryAdmission::Recorded
        );
        assert_eq!(columns.len(), 2);
        assert!(columns.destinations().contains(&dest(2)));
        assert!(columns.destinations().contains(&dest(3)));
        assert!(!columns.destinations().contains(&dest(1)));
    }

    #[test]
    fn a_zero_capacity_table_is_untrackable() {
        let mut columns = FixedHeapAnnounceRateColumns::<0>::default();
        assert_eq!(
            columns.insert(dest(1), entry_at(1)),
            RateEntryAdmission::Untrackable
        );
    }

    #[test]
    fn the_bulk_columns_carry_a_large_table() {
        let mut columns = FixedHeapAnnounceRateColumns::<2048>::default();
        for n in 0..2048u32 {
            assert_eq!(
                columns.insert(dest(n as u8), entry_at(n as u64)),
                RateEntryAdmission::Recorded
            );
        }
        assert_eq!(columns.len(), 2048);
    }
}
