use crate::engine::InstantMillis;
use crate::interfaces::AnnounceRateLimit;
use crate::lemire_index::buckets_for_two_thirds_load;
use crate::wire::DestinationHash;

#[cfg(feature = "std")]
const MAX_ANNOUNCE_RATE_OBSERVATIONS: usize = 16;

pub const fn destination_announce_limit_index_buckets(entries: usize) -> usize {
    buckets_for_two_thirds_load(entries)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DestinationAnnounceLimit {
    pub last_allowed_announce_at: InstantMillis,
    pub blocked_until: InstantMillis,
    pub rate_violations: u16,
    #[cfg(feature = "std")]
    observations: AnnounceObservationHistory,
}

impl Default for DestinationAnnounceLimit {
    fn default() -> Self {
        Self {
            last_allowed_announce_at: InstantMillis(0),
            blocked_until: InstantMillis(0),
            rate_violations: 0,
            #[cfg(feature = "std")]
            observations: AnnounceObservationHistory::default(),
        }
    }
}

#[cfg(feature = "std")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AnnounceObservationHistory {
    observed_at: [InstantMillis; MAX_ANNOUNCE_RATE_OBSERVATIONS],
    oldest: u8,
    len: u8,
}

#[cfg(feature = "std")]
impl Default for AnnounceObservationHistory {
    fn default() -> Self {
        Self {
            observed_at: [InstantMillis(0); MAX_ANNOUNCE_RATE_OBSERVATIONS],
            oldest: 0,
            len: 0,
        }
    }
}

#[cfg(feature = "std")]
impl AnnounceObservationHistory {
    fn starting_at(observed_at: InstantMillis) -> Self {
        let mut history = Self::default();
        history.record(observed_at);
        history
    }

    fn record(&mut self, observed_at: InstantMillis) {
        let oldest = usize::from(self.oldest);
        let len = usize::from(self.len);
        if len < MAX_ANNOUNCE_RATE_OBSERVATIONS {
            let slot = (oldest + len) % MAX_ANNOUNCE_RATE_OBSERVATIONS;
            self.observed_at[slot] = observed_at;
            self.len += 1;
            return;
        }
        self.observed_at[oldest] = observed_at;
        self.oldest = ((oldest + 1) % MAX_ANNOUNCE_RATE_OBSERVATIONS) as u8;
    }

    #[cfg(test)]
    fn iter(&self) -> impl Iterator<Item = InstantMillis> + '_ {
        (0..usize::from(self.len)).map(|offset| {
            self.observed_at[(usize::from(self.oldest) + offset) % MAX_ANNOUNCE_RATE_OBSERVATIONS]
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationAnnounceVerdict {
    Allowed,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationAnnounceLimitAdmission {
    Recorded,
    Untrackable,
}

pub trait DestinationAnnounceLimitTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.destinations()
            .iter()
            .position(|candidate| candidate == destination)
    }
    fn destinations(&self) -> &[DestinationHash];
    fn entries_mut(&mut self) -> &mut [DestinationAnnounceLimit];
    /// Returns `Untrackable` only at capacity zero, where the table can hold nothing.
    fn insert(
        &mut self,
        destination: DestinationHash,
        entry: DestinationAnnounceLimit,
    ) -> DestinationAnnounceLimitAdmission;
}

#[derive(Debug, Default)]
pub struct DestinationAnnounceLimits<C: DestinationAnnounceLimitTable> {
    table: C,
}

impl<C: DestinationAnnounceLimitTable> DestinationAnnounceLimits<C> {
    /// Apply RNS's rate accounting for one heard announce and return whether its rebroadcast is blocked. A first sighting is always allowed.
    pub fn observe(
        &mut self,
        destination: DestinationHash,
        now: InstantMillis,
        limit: AnnounceRateLimit,
    ) -> DestinationAnnounceVerdict {
        if let Some(index) = self.table.index_of(&destination) {
            let entry = &mut self.table.entries_mut()[index];
            #[cfg(feature = "std")]
            entry.observations.record(now);
            observe_existing(entry, now, limit)
        } else {
            let entry = DestinationAnnounceLimit {
                last_allowed_announce_at: now,
                blocked_until: InstantMillis(0),
                rate_violations: 0,
                #[cfg(feature = "std")]
                observations: AnnounceObservationHistory::starting_at(now),
            };
            match self.table.insert(destination, entry) {
                // A first sighting is allowed whether or not we could record it. A table that holds nothing cannot rate-limit, so capacity zero fails open rather than silence the mesh.
                DestinationAnnounceLimitAdmission::Recorded
                | DestinationAnnounceLimitAdmission::Untrackable => {
                    DestinationAnnounceVerdict::Allowed
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

fn observe_existing(
    entry: &mut DestinationAnnounceLimit,
    now: InstantMillis,
    limit: AnnounceRateLimit,
) -> DestinationAnnounceVerdict {
    if now.0 <= entry.blocked_until.0 {
        return DestinationAnnounceVerdict::Blocked;
    }
    let interval_ms = now.0.saturating_sub(entry.last_allowed_announce_at.0);
    if interval_ms < limit.target_ms {
        entry.rate_violations = entry.rate_violations.saturating_add(1);
    } else {
        entry.rate_violations = entry.rate_violations.saturating_sub(1);
    }
    if entry.rate_violations > limit.grace {
        entry.blocked_until = InstantMillis(
            entry
                .last_allowed_announce_at
                .0
                .saturating_add(limit.target_ms)
                .saturating_add(limit.penalty_ms),
        );
        DestinationAnnounceVerdict::Blocked
    } else {
        entry.last_allowed_announce_at = now;
        DestinationAnnounceVerdict::Allowed
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn limit(target_ms: u64, grace: u16, penalty_ms: u64) -> AnnounceRateLimit {
        AnnounceRateLimit {
            target_ms,
            grace,
            penalty_ms,
        }
    }

    #[test]
    fn a_first_sighting_is_always_allowed() {
        let mut rates: DestinationAnnounceLimits<FixedDestinationAnnounceLimitTable<4>> =
            DestinationAnnounceLimits::default();
        assert_eq!(
            rates.observe(dest(1), InstantMillis(0), limit(10_000, 0, 60_000)),
            DestinationAnnounceVerdict::Allowed
        );
        assert_eq!(rates.len(), 1);
    }

    #[cfg(feature = "std")]
    #[test]
    fn observation_history_retains_the_latest_sixteen_in_order() {
        let mut rates: DestinationAnnounceLimits<FixedDestinationAnnounceLimitTable<4>> =
            DestinationAnnounceLimits::default();
        let l = limit(10_000, 0, 60_000);
        for observed_at in 0u64..20 {
            rates.observe(dest(1), InstantMillis(observed_at), l);
        }

        let observed_at = rates.table.entries_mut()[0]
            .observations
            .iter()
            .collect::<std::vec::Vec<_>>();
        assert_eq!(
            observed_at,
            (4u64..20).map(InstantMillis).collect::<std::vec::Vec<_>>()
        );
    }

    #[test]
    fn an_announce_faster_than_target_past_grace_is_blocked() {
        let mut rates: DestinationAnnounceLimits<FixedDestinationAnnounceLimitTable<4>> =
            DestinationAnnounceLimits::default();
        assert_eq!(
            rates.observe(dest(1), InstantMillis(0), limit(10_000, 0, 60_000)),
            DestinationAnnounceVerdict::Allowed
        );
        assert_eq!(
            rates.observe(dest(1), InstantMillis(5_000), limit(10_000, 0, 60_000)),
            DestinationAnnounceVerdict::Blocked
        );
    }

    #[test]
    fn an_announcer_slower_than_target_is_never_blocked() {
        let mut rates: DestinationAnnounceLimits<FixedDestinationAnnounceLimitTable<4>> =
            DestinationAnnounceLimits::default();
        let l = limit(10_000, 0, 60_000);
        assert_eq!(
            rates.observe(dest(1), InstantMillis(0), l),
            DestinationAnnounceVerdict::Allowed
        );
        assert_eq!(
            rates.observe(dest(1), InstantMillis(20_000), l),
            DestinationAnnounceVerdict::Allowed
        );
        assert_eq!(
            rates.observe(dest(1), InstantMillis(40_000), l),
            DestinationAnnounceVerdict::Allowed
        );
    }

    #[test]
    fn grace_absorbs_the_first_violations_then_blocks() {
        let mut rates: DestinationAnnounceLimits<FixedDestinationAnnounceLimitTable<4>> =
            DestinationAnnounceLimits::default();
        let l = limit(10_000, 2, 60_000);
        assert_eq!(
            rates.observe(dest(1), InstantMillis(0), l),
            DestinationAnnounceVerdict::Allowed
        );
        assert_eq!(
            rates.observe(dest(1), InstantMillis(1_000), l),
            DestinationAnnounceVerdict::Allowed
        );
        assert_eq!(
            rates.observe(dest(1), InstantMillis(2_000), l),
            DestinationAnnounceVerdict::Allowed
        );
        assert_eq!(
            rates.observe(dest(1), InstantMillis(3_000), l),
            DestinationAnnounceVerdict::Blocked
        );
    }

    #[test]
    fn a_block_holds_until_its_window_expires() {
        let mut rates: DestinationAnnounceLimits<FixedDestinationAnnounceLimitTable<4>> =
            DestinationAnnounceLimits::default();
        let l = limit(10_000, 0, 60_000);
        rates.observe(dest(1), InstantMillis(0), l);
        assert_eq!(
            rates.observe(dest(1), InstantMillis(5_000), l),
            DestinationAnnounceVerdict::Blocked
        );
        assert_eq!(
            rates.observe(dest(1), InstantMillis(70_000), l),
            DestinationAnnounceVerdict::Blocked
        );
        assert_eq!(
            rates.observe(dest(1), InstantMillis(120_000), l),
            DestinationAnnounceVerdict::Allowed
        );
    }

    #[test]
    fn slowing_down_recovers_violations() {
        let mut rates: DestinationAnnounceLimits<FixedDestinationAnnounceLimitTable<4>> =
            DestinationAnnounceLimits::default();
        let l = limit(10_000, 2, 60_000);
        rates.observe(dest(1), InstantMillis(0), l);
        rates.observe(dest(1), InstantMillis(1_000), l); // violations 1
        rates.observe(dest(1), InstantMillis(2_000), l); // violations 2
        assert_eq!(
            rates.observe(dest(1), InstantMillis(20_000), l),
            DestinationAnnounceVerdict::Allowed
        );
        assert_eq!(
            rates.observe(dest(1), InstantMillis(21_000), l),
            DestinationAnnounceVerdict::Allowed
        );
        assert_eq!(
            rates.observe(dest(1), InstantMillis(22_000), l),
            DestinationAnnounceVerdict::Blocked
        );
    }

    #[test]
    fn distinct_destinations_keep_independent_state() {
        let mut rates: DestinationAnnounceLimits<FixedDestinationAnnounceLimitTable<4>> =
            DestinationAnnounceLimits::default();
        let l = limit(10_000, 0, 60_000);
        rates.observe(dest(1), InstantMillis(0), l);
        rates.observe(dest(2), InstantMillis(0), l);
        assert_eq!(
            rates.observe(dest(1), InstantMillis(5_000), l),
            DestinationAnnounceVerdict::Blocked
        );
        assert_eq!(
            rates.observe(dest(2), InstantMillis(5_000), l),
            DestinationAnnounceVerdict::Blocked
        );
        assert_eq!(rates.len(), 2);
    }

    #[test]
    fn eviction_drops_the_least_recently_active() {
        let mut rates: DestinationAnnounceLimits<FixedDestinationAnnounceLimitTable<2>> =
            DestinationAnnounceLimits::default();
        let l = limit(10_000, 8, 60_000);
        rates.observe(dest(1), InstantMillis(100), l);
        rates.observe(dest(2), InstantMillis(200), l);
        rates.observe(dest(3), InstantMillis(300), l);
        assert_eq!(rates.len(), 2);
        assert_eq!(
            rates.observe(dest(1), InstantMillis(400), l),
            DestinationAnnounceVerdict::Allowed
        );
    }

    #[test]
    fn capacity_zero_stores_nothing_and_never_blocks() {
        let mut rates: DestinationAnnounceLimits<FixedDestinationAnnounceLimitTable<0>> =
            DestinationAnnounceLimits::default();
        let l = limit(10_000, 0, 60_000);
        assert_eq!(
            rates.observe(dest(1), InstantMillis(0), l),
            DestinationAnnounceVerdict::Allowed
        );
        assert_eq!(
            rates.observe(dest(1), InstantMillis(1_000), l),
            DestinationAnnounceVerdict::Allowed
        );
        assert!(rates.is_empty());
    }

    #[test]
    fn heap_columns_track_past_any_fixed_ceiling() {
        let mut rates: DestinationAnnounceLimits<HeapDestinationAnnounceLimitTable> =
            DestinationAnnounceLimits::default();
        let l = limit(10_000, 0, 60_000);
        for n in 0..64u8 {
            assert_eq!(
                rates.observe(dest(n), InstantMillis(0), l),
                DestinationAnnounceVerdict::Allowed
            );
        }
        assert_eq!(rates.len(), 64);
        assert_eq!(
            rates.observe(dest(17), InstantMillis(5_000), l),
            DestinationAnnounceVerdict::Blocked
        );
    }

    #[test]
    fn insert_reports_whether_the_entry_could_be_recorded() {
        let mut holds: FixedDestinationAnnounceLimitTable<2> =
            FixedDestinationAnnounceLimitTable::default();
        assert_eq!(
            holds.insert(dest(1), DestinationAnnounceLimit::default()),
            DestinationAnnounceLimitAdmission::Recorded
        );
        let mut holds_nothing: FixedDestinationAnnounceLimitTable<0> =
            FixedDestinationAnnounceLimitTable::default();
        assert_eq!(
            holds_nothing.insert(dest(1), DestinationAnnounceLimit::default()),
            DestinationAnnounceLimitAdmission::Untrackable
        );
    }
}
