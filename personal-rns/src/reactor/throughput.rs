use crate::engine::InstantMillis;
use crate::interfaces::TransferRates;
use crate::reactor::window_ring::WindowRing;

const RATE_BUCKET_MS: u64 = 1_000;
const RATE_BUCKETS: usize = 15;
const RATE_WINDOW_SECS: u64 = 15;

pub struct ThroughputLedger {
    rx: WindowRing<RATE_BUCKETS>,
    tx: WindowRing<RATE_BUCKETS>,
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
            rx: WindowRing::new(RATE_BUCKET_MS),
            tx: WindowRing::new(RATE_BUCKET_MS),
        }
    }

    pub fn record_rx(&mut self, now: InstantMillis, bytes: u64) {
        self.rx.record(now, bytes);
    }

    pub fn record_tx(&mut self, now: InstantMillis, bytes: u64) {
        self.tx.record(now, bytes);
    }

    pub fn rates(&mut self, now: InstantMillis) -> TransferRates {
        TransferRates {
            rx_bps: bps(self.rx.total(now)),
            tx_bps: bps(self.tx.total(now)),
        }
    }
}

fn bps(window_bytes: u64) -> u32 {
    u32::try_from(window_bytes.saturating_mul(8) / RATE_WINDOW_SECS).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_average_the_window_per_direction() {
        let mut ledger = ThroughputLedger::new();
        ledger.record_rx(InstantMillis(1_000), 15_000);
        ledger.record_tx(InstantMillis(1_500), 3_000);
        let rates = ledger.rates(InstantMillis(2_000));
        assert_eq!(rates.rx_bps, 8_000, "15 KB over the 15s window is 8 kbps");
        assert_eq!(rates.tx_bps, 1_600);
    }

    #[test]
    fn rates_fall_as_the_window_ages_out() {
        let mut ledger = ThroughputLedger::new();
        ledger.record_rx(InstantMillis(1_000), 15_000);
        let rates = ledger.rates(InstantMillis(1_000 + RATE_BUCKETS as u64 * RATE_BUCKET_MS));
        assert_eq!(rates.rx_bps, 0);
        assert_eq!(rates.tx_bps, 0);
    }
}
