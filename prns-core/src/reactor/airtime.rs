//! Actual wall-clock airtime usage, used primarily for duty cycle control on constrained interfaces.

use crate::engine::InstantMillis;
use crate::interfaces::AirtimeUtilization;
use crate::reactor::window_ring::WindowRing;

const SHORT_BUCKET_MS: u64 = 1_000;
const SHORT_BUCKETS: usize = 15;
const LONG_BUCKET_MS: u64 = 60_000;
const LONG_BUCKETS: usize = 60;
const AIRTIME_SHORT_WINDOW_MS: u64 = SHORT_BUCKET_MS * SHORT_BUCKETS as u64;
const AIRTIME_LONG_WINDOW_MS: u64 = LONG_BUCKET_MS * LONG_BUCKETS as u64;

pub fn frame_airtime_us(frame_bytes: usize, bitrate_bps: u32) -> u64 {
    if bitrate_bps == 0 {
        return 0;
    }
    (frame_bytes as u64).saturating_mul(8_000_000) / u64::from(bitrate_bps)
}

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
            short_per_mille: per_mille(self.short.total(now), AIRTIME_SHORT_WINDOW_MS),
            long_per_mille: per_mille(self.long.total(now), AIRTIME_LONG_WINDOW_MS),
        }
    }
}

fn per_mille(total_us: u64, window_ms: u64) -> u16 {
    let window_us = window_ms * 1_000;
    (total_us.saturating_mul(1_000) / window_us).min(1_000) as u16
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
            "a full BROADCAST_MTU frame on a gigabit link still registers, in microseconds",
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
