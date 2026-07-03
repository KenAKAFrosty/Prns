//! The interface data-rate meter: the *active* transfer rate (while bytes are actually moving,
//! how fast is the pipe?) averaged over the last few data events, not over wall-clock time. A
//! 15 KB burst that finishes in half a second moved at ~30 KB/s, and that is what a diagnostic
//! should read, not `15 KB ÷ 15 s` because the meter happened to look over a 15-second window.
//!
//! Each sample is one event's bytes over the gap since the previous event; a gap longer than
//! `IDLE_GAP_MS` is idle, not transit, and is skipped. The mean over `SAMPLE_WINDOW` samples is
//! *held* between bursts (the rate the pipe moves data at does not become zero because the pipe
//! paused). Per direction: a fixed `[u32; SAMPLE_WINDOW]` ring plus two cursors, reported in bits per second.

use crate::engine::InstantMillis;
use crate::interfaces::TransferRates;

/// A data event farther from the previous one than this is idle, not transit: averaging across
/// it would smear a real rate, so the interval takes no sample and the held mean stays.
const IDLE_GAP_MS: u64 = 2_000;

/// How many recent per-event samples the mean is taken over: small enough to track a real rate
/// change quickly, large enough that one fast or slow event does not make the figure jump.
const SAMPLE_WINDOW: usize = 8;

/// One direction's active-rate estimate.
struct ActiveRate {
    last_ms: u64,
    seen: bool,
    samples: [u32; SAMPLE_WINDOW],
    /// Number of samples taken, capped at `SAMPLE_WINDOW`; the mean is over this many slots.
    filled: usize,
    /// Next slot to overwrite once the ring is full.
    head: usize,
}

impl ActiveRate {
    const fn new() -> Self {
        Self {
            last_ms: 0,
            seen: false,
            samples: [0; SAMPLE_WINDOW],
            filled: 0,
            head: 0,
        }
    }

    /// Fold one data event of `bytes` at `now` into the rate, attributed to the interval since
    /// the previous event (back-to-back chunks sample fast, spaced chunks slow); an interval
    /// past `IDLE_GAP_MS` is idle and takes no sample, leaving the held mean intact.
    fn record(&mut self, now: InstantMillis, bytes: u64) {
        let now = now.0;
        if self.seen {
            let gap = now.saturating_sub(self.last_ms);
            if gap <= IDLE_GAP_MS {
                let dt = gap.max(1);
                let sample = u32::try_from(bytes.saturating_mul(8_000) / dt).unwrap_or(u32::MAX);
                self.samples[self.head] = sample;
                self.head = (self.head + 1) % SAMPLE_WINDOW;
                self.filled = (self.filled + 1).min(SAMPLE_WINDOW);
            }
        }
        self.seen = true;
        self.last_ms = now;
    }

    /// The mean of the recent samples, held between bursts; `0` only before any data has moved.
    fn rate(&self) -> u32 {
        if self.filled == 0 {
            return 0;
        }
        let sum: u64 = self.samples[..self.filled]
            .iter()
            .map(|&sample| u64::from(sample))
            .sum();
        u32::try_from(sum / self.filled as u64).unwrap_or(u32::MAX)
    }
}

pub struct ThroughputLedger {
    rx: ActiveRate,
    tx: ActiveRate,
}

impl Default for ThroughputLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl ThroughputLedger {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            rx: ActiveRate::new(),
            tx: ActiveRate::new(),
        }
    }

    pub fn record_rx(&mut self, now: InstantMillis, bytes: u64) {
        self.rx.record(now, bytes);
    }

    pub fn record_tx(&mut self, now: InstantMillis, bytes: u64) {
        self.tx.record(now, bytes);
    }

    pub fn rates(&self) -> TransferRates {
        TransferRates {
            rx_bps: self.rx.rate(),
            tx_bps: self.tx.rate(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_the_active_transfer_rate_not_a_wall_clock_average() {
        let mut ledger = ThroughputLedger::new();
        // Two 1000-byte chunks 100 ms apart: the second moved 1000 B over 100 ms = 80 kbps.
        ledger.record_rx(InstantMillis(0), 1_000);
        ledger.record_rx(InstantMillis(100), 1_000);
        assert_eq!(ledger.rates().rx_bps, 80_000);
    }

    #[test]
    fn a_quick_burst_reads_its_real_rate_not_diluted_by_surrounding_idle() {
        let mut ledger = ThroughputLedger::new();
        // 15 KB moved as ten 1500-byte chunks 10 ms apart — a ~100 ms burst. Each interval is
        // 1500 B / 10 ms = 1.2 Mbps, so the active rate sits there, not at the `15 KB / 15 s`
        // (~8 kbps) a wall-clock window would report.
        let mut t = 1_000;
        ledger.record_tx(InstantMillis(t), 1_500);
        for _ in 0..9 {
            t += 10;
            ledger.record_tx(InstantMillis(t), 1_500);
        }
        assert_eq!(ledger.rates().tx_bps, 1_200_000);
    }

    #[test]
    fn the_rate_is_the_mean_of_the_recent_samples() {
        let mut ledger = ThroughputLedger::new();
        // Sample one: 1000 B / 100 ms = 80 kbps. Sample two: 1000 B / 200 ms = 40 kbps. Mean 60.
        ledger.record_rx(InstantMillis(0), 1_000);
        ledger.record_rx(InstantMillis(100), 1_000);
        ledger.record_rx(InstantMillis(300), 1_000);
        assert_eq!(ledger.rates().rx_bps, 60_000);
    }

    #[test]
    fn the_rate_is_held_between_bursts_not_dropped_to_zero() {
        let mut ledger = ThroughputLedger::new();
        ledger.record_rx(InstantMillis(0), 1_000);
        ledger.record_rx(InstantMillis(100), 1_000);
        // Long after the last byte, the meter still reports the rate the pipe moved data at.
        assert_eq!(ledger.rates().rx_bps, 80_000);
    }

    #[test]
    fn an_idle_gap_takes_no_sample_so_it_cannot_skew_the_mean() {
        let mut ledger = ThroughputLedger::new();
        ledger.record_tx(InstantMillis(0), 1_000);
        ledger.record_tx(InstantMillis(100), 1_000); // 80 kbps sample
                                                     // 5 s later: the gap is past the idle window, so no skewed sample is taken.
        ledger.record_tx(InstantMillis(5_000), 1_000);
        assert_eq!(ledger.rates().tx_bps, 80_000);
        // A fresh interval from there samples cleanly and joins the mean (80 kbps again).
        ledger.record_tx(InstantMillis(5_100), 1_000);
        assert_eq!(ledger.rates().tx_bps, 80_000);
    }
}
