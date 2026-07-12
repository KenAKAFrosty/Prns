use crate::engine::InstantMillis;

pub(crate) struct WindowRing<const BUCKETS: usize> {
    buckets: [u32; BUCKETS],
    bucket_ms: u64,
    head_ordinal: u64,
    total: u64,
}

impl<const BUCKETS: usize> WindowRing<BUCKETS> {
    pub(crate) const fn new(bucket_ms: u64) -> Self {
        Self {
            buckets: [0; BUCKETS],
            bucket_ms,
            head_ordinal: 0,
            total: 0,
        }
    }

    pub(crate) fn record(&mut self, now: InstantMillis, amount: u64) {
        self.advance(now);
        let slot = (self.head_ordinal % BUCKETS as u64) as usize;
        let capped = u32::try_from(amount).unwrap_or(u32::MAX);
        let grown = self.buckets[slot].saturating_add(capped);
        self.total += u64::from(grown - self.buckets[slot]);
        self.buckets[slot] = grown;
    }

    pub(crate) fn total(&mut self, now: InstantMillis) -> u64 {
        self.advance(now);
        debug_assert_eq!(
            self.total,
            self.buckets
                .iter()
                .map(|&amount| u64::from(amount))
                .sum::<u64>(),
            "the running total desynced from the buckets"
        );
        self.total
    }

    fn advance(&mut self, now: InstantMillis) {
        let ordinal = now.0 / self.bucket_ms;
        let skipped = ordinal.saturating_sub(self.head_ordinal);
        if skipped >= BUCKETS as u64 {
            self.buckets = [0; BUCKETS];
            self.total = 0;
        } else {
            for stale in 1..=skipped {
                let slot = ((self.head_ordinal + stale) % BUCKETS as u64) as usize;
                self.total -= u64::from(self.buckets[slot]);
                self.buckets[slot] = 0;
            }
        }
        self.head_ordinal = ordinal.max(self.head_ordinal);
    }
}
