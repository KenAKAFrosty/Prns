mod impls;

pub use impls::*;

use crate::engine::InstantMillis;
use crate::wire::DestinationHash;

/// RNS 1.3.1 `Transport.PATH_REQUEST_MI` (20 seconds)
pub const PATH_REQUEST_MIN_INTERVAL_MS: u64 = 20 * 1_000;

pub trait RecentPathRequestColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn requested_ats(&self) -> &[InstantMillis];
    fn push(&mut self, destination: DestinationHash, requested_at: InstantMillis);
    fn swap_remove(&mut self, index: usize);
}

#[derive(Debug, Default)]
pub struct RecentPathRequests<C: RecentPathRequestColumns> {
    columns: C,
}

impl<C: RecentPathRequestColumns> RecentPathRequests<C> {
    pub fn mark_seen_at(&mut self, destination: DestinationHash, now: InstantMillis) {
        if let Some(index) = self.index_of(&destination) {
            self.columns.swap_remove(index);
        }
        self.evict_stale(now);
        if self.columns.len() >= self.columns.capacity() {
            self.evict_oldest();
        }
        self.columns.push(destination, now);
    }

    pub fn is_throttled(&self, destination: &DestinationHash, now: InstantMillis) -> bool {
        self.columns
            .destinations()
            .iter()
            .zip(self.columns.requested_ats())
            .any(|(candidate, requested_at)| {
                candidate == destination && !aged_out(*requested_at, now)
            })
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.columns
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)
    }

    fn evict_stale(&mut self, now: InstantMillis) {
        while let Some(index) = self
            .columns
            .requested_ats()
            .iter()
            .position(|requested_at| aged_out(*requested_at, now))
        {
            self.columns.swap_remove(index);
        }
    }

    fn evict_oldest(&mut self) {
        let Some(index) = self
            .columns
            .requested_ats()
            .iter()
            .enumerate()
            .min_by_key(|(_, requested_at)| requested_at.0)
            .map(|(index, _)| index)
        else {
            return;
        };
        self.columns.swap_remove(index);
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

fn aged_out(requested_at: InstantMillis, now: InstantMillis) -> bool {
    now.0.saturating_sub(requested_at.0) >= PATH_REQUEST_MIN_INTERVAL_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    #[test]
    fn a_fresh_stamp_throttles_until_the_interval_passes() {
        let mut recent: RecentPathRequests<FixedRecentPathRequestColumns<4>> =
            RecentPathRequests::default();
        recent.mark_seen_at(dest(1), InstantMillis(1_000));

        assert!(recent.is_throttled(&dest(1), InstantMillis(1_000)));
        assert!(recent.is_throttled(
            &dest(1),
            InstantMillis(1_000 + PATH_REQUEST_MIN_INTERVAL_MS - 1)
        ));
        assert!(!recent.is_throttled(
            &dest(1),
            InstantMillis(1_000 + PATH_REQUEST_MIN_INTERVAL_MS)
        ));
    }

    #[test]
    fn an_unstamped_destination_is_never_throttled() {
        let recent: RecentPathRequests<FixedRecentPathRequestColumns<4>> =
            RecentPathRequests::default();
        assert!(!recent.is_throttled(&dest(7), InstantMillis(1_000)));
    }

    #[test]
    fn a_second_stamp_overwrites_rather_than_duplicates() {
        let mut recent: RecentPathRequests<FixedRecentPathRequestColumns<4>> =
            RecentPathRequests::default();
        recent.mark_seen_at(dest(1), InstantMillis(1_000));
        recent.mark_seen_at(dest(1), InstantMillis(5_000));

        assert_eq!(recent.len(), 1, "one destination, one row");
        assert!(
            recent.is_throttled(
                &dest(1),
                InstantMillis(1_000 + PATH_REQUEST_MIN_INTERVAL_MS)
            ),
            "the window runs from the newer stamp, so it outlasts the older stamp's expiry",
        );
        assert!(
            !recent.is_throttled(
                &dest(1),
                InstantMillis(5_000 + PATH_REQUEST_MIN_INTERVAL_MS)
            ),
            "and clears once the newer stamp's own interval passes",
        );
    }

    #[test]
    fn stamping_sweeps_rows_that_aged_out_of_the_window() {
        let mut recent: RecentPathRequests<FixedRecentPathRequestColumns<4>> =
            RecentPathRequests::default();
        recent.mark_seen_at(dest(1), InstantMillis(1_000));
        recent.mark_seen_at(dest(2), InstantMillis(2_000));

        recent.mark_seen_at(dest(3), InstantMillis(1_000 + PATH_REQUEST_MIN_INTERVAL_MS));

        assert_eq!(recent.len(), 2, "the aged-out first stamp is gone");
        assert!(!recent.is_throttled(
            &dest(1),
            InstantMillis(1_000 + PATH_REQUEST_MIN_INTERVAL_MS)
        ));
        assert!(recent.is_throttled(
            &dest(2),
            InstantMillis(2_000 + PATH_REQUEST_MIN_INTERVAL_MS - 1)
        ));
        assert!(recent.is_throttled(
            &dest(3),
            InstantMillis(1_000 + PATH_REQUEST_MIN_INTERVAL_MS)
        ));
    }

    #[test]
    fn a_full_table_evicts_its_oldest_stamp_for_the_new_one() {
        let mut recent: RecentPathRequests<FixedRecentPathRequestColumns<2>> =
            RecentPathRequests::default();
        recent.mark_seen_at(dest(1), InstantMillis(1_000));
        recent.mark_seen_at(dest(2), InstantMillis(2_000));
        recent.mark_seen_at(dest(3), InstantMillis(3_000));

        assert_eq!(recent.len(), 2);
        assert!(
            !recent.is_throttled(&dest(1), InstantMillis(3_000)),
            "the oldest stamp made way for the newcomer",
        );
        assert!(recent.is_throttled(&dest(2), InstantMillis(3_000)));
        assert!(recent.is_throttled(&dest(3), InstantMillis(3_000)));
    }

    #[test]
    fn heap_columns_grow_past_any_fixed_ceiling() {
        let mut recent: RecentPathRequests<HeapRecentPathRequestColumns> =
            RecentPathRequests::default();
        for n in 0..64u8 {
            recent.mark_seen_at(dest(n), InstantMillis(1_000));
        }
        assert_eq!(recent.len(), 64);
        assert!(recent.is_throttled(&dest(17), InstantMillis(1_000)));
    }
}
