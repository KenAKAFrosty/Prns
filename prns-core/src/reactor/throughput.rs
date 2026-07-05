//! The interface data-rate meter: the *active* transfer rate (while bytes are actually moving, how fast is the pipe?) averaged over the last few data events, not over wall-clock time.
//! (e.g., A 15 KB burst that finishes in half a second moved at ~30 KB/s. No activity for another half-second still means 30 KB/s, not 15).

use crate::engine::InstantMillis;
use crate::interfaces::TransferRates;

const CONSIDERED_IDLE_AFTER_MS: u64 = 2_000;
const SAMPLE_WINDOW: usize = 8;

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

    fn record(&mut self, now: InstantMillis, bytes: u64) {
        let now = now.0;
        let dt = if self.seen {
            now.saturating_sub(self.last_ms)
                .clamp(1, CONSIDERED_IDLE_AFTER_MS)
        } else {
            CONSIDERED_IDLE_AFTER_MS
        };
        let sample = u32::try_from(bytes.saturating_mul(8_000) / dt).unwrap_or(u32::MAX);
        self.samples[self.head] = sample;
        self.head = (self.head + 1) % SAMPLE_WINDOW;
        self.filled = (self.filled + 1).min(SAMPLE_WINDOW);
        self.seen = true;
        self.last_ms = now;
    }

    /// The mean of the recent timing samples. Held between bursts; `0` only before any data has moved.
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
        // The first-ever event samples at the floor (1000 B over the 2 s clamp = 4 kbps); the
        // second moved 1000 B over 100 ms = 80 kbps. Mean 42 kbps — far above the ~1 kbps a
        // 15-second wall-clock window would report.
        ledger.record_rx(InstantMillis(0), 1_000);
        ledger.record_rx(InstantMillis(100), 1_000);
        assert_eq!(ledger.rates().rx_bps, 42_000);
    }

    #[test]
    fn a_quick_burst_reads_its_real_rate_not_diluted_by_surrounding_idle() {
        let mut ledger = ThroughputLedger::new();
        // 15 KB moved as ten 1500-byte chunks 10 ms apart — a ~100 ms burst. Each in-burst
        // interval is 1500 B / 10 ms = 1.2 Mbps, and the burst's own length pushes its one
        // floor seed out of the 8-sample window, so the active rate sits at the real figure,
        // not the `15 KB / 15 s` (~8 kbps) a wall-clock window would report.
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
        // Floor seed 4 kbps, then 1000 B / 100 ms = 80 kbps, then 1000 B / 200 ms = 40 kbps.
        ledger.record_rx(InstantMillis(0), 1_000);
        ledger.record_rx(InstantMillis(100), 1_000);
        ledger.record_rx(InstantMillis(300), 1_000);
        assert_eq!(ledger.rates().rx_bps, 41_333);
    }

    #[test]
    fn the_rate_is_held_between_bursts_not_dropped_to_zero() {
        let mut ledger = ThroughputLedger::new();
        ledger.record_rx(InstantMillis(0), 1_000);
        ledger.record_rx(InstantMillis(100), 1_000);
        // Long after the last byte, the meter still reports the rate the pipe moved data at.
        assert_eq!(ledger.rates().rx_bps, 42_000);
    }

    #[test]
    fn an_event_after_an_idle_gap_samples_the_floor_not_the_smear() {
        let mut ledger = ThroughputLedger::new();
        ledger.record_tx(InstantMillis(0), 1_000); // floor seed: 4 kbps
        ledger.record_tx(InstantMillis(100), 1_000); // 80 kbps
                                                     // 5 s later: the gap clamps to 2 s, a 4 kbps floor sample — not the 1.6 kbps smear the
                                                     // raw 4.9 s interval would read.
        ledger.record_tx(InstantMillis(5_000), 1_000);
        assert_eq!(ledger.rates().tx_bps, 29_333);
        // A fresh interval from there samples cleanly and joins the mean.
        ledger.record_tx(InstantMillis(5_100), 1_000);
        assert_eq!(ledger.rates().tx_bps, 42_000);
    }

    #[test]
    fn sparse_lone_frames_read_a_nonzero_floor() {
        let mut ledger = ThroughputLedger::new();
        // Announce-only traffic: one 167-byte frame every three minutes. Every event samples at
        // the floor (167 B over the 2 s clamp = 668 bps), so the meter reads moving-not-idle
        // instead of the 0 bps that hid all sparse traffic.
        ledger.record_tx(InstantMillis(1_000), 167);
        ledger.record_tx(InstantMillis(181_000), 167);
        ledger.record_tx(InstantMillis(361_000), 167);
        assert_eq!(ledger.rates().tx_bps, 668);
        assert_eq!(ledger.rates().rx_bps, 0, "nothing was ever received");
    }
}
