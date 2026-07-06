use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::routing::announce::destination_announce_limit::{
    destination_announce_limit_index_buckets, DestinationAnnounceLimit,
    DestinationAnnounceLimitAdmission, DestinationAnnounceLimitColumns,
};
use crate::routing::lemire_index::LemireIndex;
use crate::wire::DestinationHash;

fn filled<T: Clone, A: Allocator>(value: T, len: usize, alloc: A) -> Box<[T], A> {
    let mut column = Vec::with_capacity_in(len, alloc);
    column.resize(len, value);
    column.into_boxed_slice()
}

pub struct FixedHeapDestinationAnnounceLimitColumns<
    const MAX_ANNOUNCE_RATE_ENTRIES: usize,
    const BUCKETS: usize,
    A: Allocator = Global,
> {
    len: usize,
    index: LemireIndex<BUCKETS>,
    destinations: Box<[DestinationHash], A>,
    entries: Box<[DestinationAnnounceLimit], A>,
}

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize, const BUCKETS: usize, A: Allocator + Default> Default
    for FixedHeapDestinationAnnounceLimitColumns<MAX_ANNOUNCE_RATE_ENTRIES, BUCKETS, A>
{
    fn default() -> Self {
        const {
            assert!(
                BUCKETS >= destination_announce_limit_index_buckets(MAX_ANNOUNCE_RATE_ENTRIES),
                "BUCKETS must give the index its 2/3-load headroom over the entry cap: size it with destination_announce_limit_index_buckets(MAX_ANNOUNCE_RATE_ENTRIES)",
            );
            assert!(
                MAX_ANNOUNCE_RATE_ENTRIES < u16::MAX as usize,
                "FixedHeapDestinationAnnounceLimitColumns indexes slots as u16; keep the entry cap below 65535",
            );
        }
        Self {
            len: 0,
            index: LemireIndex::default(),
            destinations: filled(
                DestinationHash::new([0u8; 16]),
                MAX_ANNOUNCE_RATE_ENTRIES,
                A::default(),
            ),
            entries: filled(
                DestinationAnnounceLimit::default(),
                MAX_ANNOUNCE_RATE_ENTRIES,
                A::default(),
            ),
        }
    }
}

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize, const BUCKETS: usize, A: Allocator>
    FixedHeapDestinationAnnounceLimitColumns<MAX_ANNOUNCE_RATE_ENTRIES, BUCKETS, A>
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

impl<const MAX_ANNOUNCE_RATE_ENTRIES: usize, const BUCKETS: usize, A: Allocator>
    DestinationAnnounceLimitColumns
    for FixedHeapDestinationAnnounceLimitColumns<MAX_ANNOUNCE_RATE_ENTRIES, BUCKETS, A>
{
    fn capacity(&self) -> usize {
        MAX_ANNOUNCE_RATE_ENTRIES
    }
    fn len(&self) -> usize {
        self.len
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index.get(destination, &self.destinations[..self.len])
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
            let victim = self.least_recently_active();
            self.index
                .remove(&self.destinations[victim], &self.destinations[..self.len]);
            victim
        };
        self.destinations[index] = destination;
        self.entries[index] = entry;
        self.index.insert(index, &self.destinations[..self.len]);
        DestinationAnnounceLimitAdmission::Recorded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::InstantMillis;

    fn dest_n(n: u32) -> DestinationHash {
        let key = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&key.to_be_bytes());
        b[8..12].copy_from_slice(&n.to_be_bytes());
        DestinationHash::new(b)
    }

    fn entry_at(ms: u64) -> DestinationAnnounceLimit {
        DestinationAnnounceLimit {
            last_allowed_announce_at: InstantMillis(ms),
            ..DestinationAnnounceLimit::default()
        }
    }

    type Rates2 = FixedHeapDestinationAnnounceLimitColumns<
        2,
        { destination_announce_limit_index_buckets(2) },
    >;
    type Rates0 = FixedHeapDestinationAnnounceLimitColumns<
        0,
        { destination_announce_limit_index_buckets(0) },
    >;
    type Rates2048 = FixedHeapDestinationAnnounceLimitColumns<
        2048,
        { destination_announce_limit_index_buckets(2048) },
    >;

    #[test]
    fn records_until_full_then_evicts_the_least_recently_active() {
        let mut columns = Rates2::default();
        assert_eq!(columns.capacity(), 2);
        assert_eq!(
            columns.insert(dest_n(1), entry_at(100)),
            DestinationAnnounceLimitAdmission::Recorded
        );
        assert_eq!(
            columns.insert(dest_n(2), entry_at(200)),
            DestinationAnnounceLimitAdmission::Recorded
        );
        assert_eq!(columns.len(), 2);

        assert_eq!(
            columns.insert(dest_n(3), entry_at(300)),
            DestinationAnnounceLimitAdmission::Recorded
        );
        assert_eq!(columns.len(), 2);
        assert!(columns.destinations().contains(&dest_n(2)));
        assert!(columns.destinations().contains(&dest_n(3)));
        assert!(!columns.destinations().contains(&dest_n(1)));
    }

    #[test]
    fn the_index_tracks_inserts_and_evictions() {
        let mut columns = Rates2::default();
        columns.insert(dest_n(1), entry_at(100));
        columns.insert(dest_n(2), entry_at(200));
        assert_eq!(columns.index_of(&dest_n(1)), Some(0));
        assert_eq!(columns.index_of(&dest_n(2)), Some(1));
        assert_eq!(columns.index_of(&dest_n(999)), None);

        columns.insert(dest_n(3), entry_at(300));
        assert_eq!(
            columns.index_of(&dest_n(1)),
            None,
            "the evicted destination is gone"
        );
        assert_eq!(
            columns.index_of(&dest_n(3)),
            Some(0),
            "the newcomer sits in the evicted row's slot"
        );
        assert_eq!(columns.index_of(&dest_n(2)), Some(1));
    }

    #[test]
    fn a_zero_capacity_table_is_untrackable() {
        let mut columns = Rates0::default();
        assert_eq!(
            columns.insert(dest_n(1), entry_at(1)),
            DestinationAnnounceLimitAdmission::Untrackable
        );
        assert_eq!(columns.index_of(&dest_n(1)), None);
    }

    #[test]
    fn the_bulk_columns_carry_a_large_table_the_inline_index_keys() {
        let mut columns = Rates2048::default();
        for n in 0..2048u32 {
            assert_eq!(
                columns.insert(dest_n(n), entry_at(n as u64)),
                DestinationAnnounceLimitAdmission::Recorded
            );
        }
        assert_eq!(columns.len(), 2048);
        assert_eq!(columns.index_of(&dest_n(0)), Some(0));
        assert_eq!(columns.index_of(&dest_n(2047)), Some(2047));
        assert_eq!(columns.index_of(&dest_n(99999)), None);
    }
}
