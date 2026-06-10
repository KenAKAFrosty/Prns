use crate::engine::InstantMillis;
use crate::interfaces::AirtimeUtilization;

pub const AIRTIME_SHORT_WINDOW_MS: u64 = 15_000;
pub const AIRTIME_LONG_WINDOW_MS: u64 = 3_600_000;

const SHORT_BUCKET_MS: u64 = 1_000;
const SHORT_BUCKETS: usize = 15;
const LONG_BUCKET_MS: u64 = 60_000;
const LONG_BUCKETS: usize = 60;

pub fn frame_airtime_us(frame_bytes: usize, bitrate_bps: u32) -> u64 {
    if bitrate_bps == 0 {
        return 0;
    }
    (frame_bytes as u64).saturating_mul(8_000_000) / u64::from(bitrate_bps)
}

#[derive(Debug, Clone)]
pub struct AirtimeLedger {
    short: WindowRing<SHORT_BUCKETS>,
    long: WindowRing<LONG_BUCKETS>,
}

impl Default for AirtimeLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl AirtimeLedger {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            short: WindowRing::new(SHORT_BUCKET_MS),
            long: WindowRing::new(LONG_BUCKET_MS),
        }
    }

    pub fn record_tx(&mut self, now: InstantMillis, airtime_us: u64) -> AirtimeUtilization {
        self.short.record(now, airtime_us);
        self.long.record(now, airtime_us);
        self.utilization(now)
    }

    pub fn utilization(&mut self, now: InstantMillis) -> AirtimeUtilization {
        AirtimeUtilization {
            short_per_mille: self.short.per_mille(now),
            long_per_mille: self.long.per_mille(now),
        }
    }
}

#[derive(Debug, Clone)]
struct WindowRing<const BUCKETS: usize> {
    bucket_us: [u32; BUCKETS],
    bucket_ms: u64,
    head_ordinal: u64,
}

impl<const BUCKETS: usize> WindowRing<BUCKETS> {
    const fn new(bucket_ms: u64) -> Self {
        Self {
            bucket_us: [0; BUCKETS],
            bucket_ms,
            head_ordinal: 0,
        }
    }

    fn advance(&mut self, now: InstantMillis) {
        let ordinal = now.0 / self.bucket_ms;
        let skipped = ordinal.saturating_sub(self.head_ordinal);
        if skipped >= BUCKETS as u64 {
            self.bucket_us = [0; BUCKETS];
        } else {
            for stale in 1..=skipped {
                let slot = ((self.head_ordinal + stale) % BUCKETS as u64) as usize;
                self.bucket_us[slot] = 0;
            }
        }
        self.head_ordinal = ordinal.max(self.head_ordinal);
    }

    fn record(&mut self, now: InstantMillis, airtime_us: u64) {
        self.advance(now);
        let slot = (self.head_ordinal % BUCKETS as u64) as usize;
        let capped = u32::try_from(airtime_us).unwrap_or(u32::MAX);
        self.bucket_us[slot] = self.bucket_us[slot].saturating_add(capped);
    }

    fn per_mille(&mut self, now: InstantMillis) -> u16 {
        self.advance(now);
        let total_us: u64 = self.bucket_us.iter().map(|&us| u64::from(us)).sum();
        let window_us = BUCKETS as u64 * self.bucket_ms * 1_000;
        let per_mille = total_us.saturating_mul(1_000) / window_us;
        per_mille.min(1_000) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_airtime_spans_the_bitrate_extremes() {
        assert_eq!(
            frame_airtime_us(167, 5_000),
            267_200,
            "a typical announce on a slow LoRa-class link costs about a quarter second",
        );
        assert_eq!(
            frame_airtime_us(500, 1_000_000_000),
            4,
            "a full MTU frame on a gigabit link still registers, in microseconds",
        );
        assert_eq!(frame_airtime_us(500, 0), 0, "no bitrate, no accounting");
    }

    #[test]
    fn utilization_reflects_what_was_recorded_in_the_window() {
        let mut ledger = AirtimeLedger::new();
        let report = ledger.record_tx(InstantMillis(1_000), 1_500_000);
        assert_eq!(
            report.short_per_mille, 100,
            "1.5s of airtime in a 15s window is 10%",
        );
        assert_eq!(
            report.long_per_mille, 0,
            "the same burst rounds to under a per-mille of an hour",
        );
    }

    #[test]
    fn the_short_window_forgets_and_the_long_window_remembers() {
        let mut ledger = AirtimeLedger::new();
        ledger.record_tx(InstantMillis(1_000), 3_600_000);
        let later = ledger.utilization(InstantMillis(1_000 + AIRTIME_SHORT_WINDOW_MS));
        assert_eq!(later.short_per_mille, 0, "the burst aged out of 15s");
        assert_eq!(later.long_per_mille, 1, "3.6s of an hour is one per-mille");

        let much_later = ledger.utilization(InstantMillis(1_000 + AIRTIME_LONG_WINDOW_MS));
        assert_eq!(much_later.long_per_mille, 0, "and out of the hour too");
    }

    #[test]
    fn buckets_accumulate_within_and_age_individually() {
        let mut ledger = AirtimeLedger::new();
        ledger.record_tx(InstantMillis(500), 750_000);
        ledger.record_tx(InstantMillis(900), 750_000);
        assert_eq!(
            ledger.utilization(InstantMillis(900)).short_per_mille,
            100,
            "same bucket accumulates",
        );

        ledger.record_tx(InstantMillis(5_000), 1_500_000);
        assert_eq!(
            ledger.utilization(InstantMillis(5_000)).short_per_mille,
            200,
            "two live buckets sum across the window",
        );
        assert_eq!(
            ledger.utilization(InstantMillis(16_200)).short_per_mille,
            100,
            "the first bucket aged out, the second still counts",
        );
    }

    #[test]
    fn utilization_is_clamped_at_full() {
        let mut ledger = AirtimeLedger::new();
        let report = ledger.record_tx(InstantMillis(1_000), 60_000_000);
        assert_eq!(
            report.short_per_mille, 1_000,
            "a frame longer than the window cannot read past 100%",
        );
    }

    #[test]
    fn a_long_idle_gap_clears_the_whole_ring() {
        let mut ledger = AirtimeLedger::new();
        ledger.record_tx(InstantMillis(1_000), 10_000_000);
        let report = ledger.utilization(InstantMillis(1_000 + 100 * AIRTIME_LONG_WINDOW_MS));
        assert_eq!(report.short_per_mille, 0);
        assert_eq!(report.long_per_mille, 0);
    }
}
