use crate::units::{DurationMillis, InstantMillis};

pub const WATCHDOG_TICK_INTERVAL: DurationMillis = DurationMillis(1_000);
pub const KEEPALIVE_AFTER: DurationMillis = DurationMillis(10_000);
pub const STALE_AFTER: DurationMillis = DurationMillis(20_000);
pub const READ_TIMEOUT: DurationMillis = DurationMillis(110_000);
pub const HDLC_KEEPALIVE: [u8; 2] = [0x7e, 0x7e];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2pReadObservation {
    Responsive,
    Recovered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2pWatchdogVerdict {
    Continue,
    Degrade,
    TransmitKeepalive,
    DegradeAndTransmitKeepalive,
    Disconnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct I2pIdleWatchdog {
    last_read: InstantMillis,
    last_ordinary_write: InstantMillis,
    degraded: bool,
}

impl I2pIdleWatchdog {
    pub const fn start(now: InstantMillis) -> Self {
        Self {
            last_read: now,
            last_ordinary_write: now,
            degraded: false,
        }
    }

    pub fn observe_read(&mut self, now: InstantMillis) -> I2pReadObservation {
        self.last_read = now;
        if core::mem::replace(&mut self.degraded, false) {
            I2pReadObservation::Recovered
        } else {
            I2pReadObservation::Responsive
        }
    }

    pub fn observe_ordinary_write(&mut self, now: InstantMillis) {
        self.last_ordinary_write = now;
    }

    pub fn tick(&mut self, now: InstantMillis) -> I2pWatchdogVerdict {
        let read_idle = now.duration_since(self.last_read);
        if read_idle > READ_TIMEOUT {
            return I2pWatchdogVerdict::Disconnect;
        }

        let degrade = read_idle > STALE_AFTER && !core::mem::replace(&mut self.degraded, true);
        let keepalive = now.duration_since(self.last_ordinary_write) > KEEPALIVE_AFTER;
        match (degrade, keepalive) {
            (false, false) => I2pWatchdogVerdict::Continue,
            (true, false) => I2pWatchdogVerdict::Degrade,
            (false, true) => I2pWatchdogVerdict::TransmitKeepalive,
            (true, true) => I2pWatchdogVerdict::DegradeAndTransmitKeepalive,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_thresholds_are_strict_and_reads_recover_degraded_connections() {
        let mut watchdog = I2pIdleWatchdog::start(InstantMillis(5_000));

        assert_eq!(
            watchdog.tick(InstantMillis(15_000)),
            I2pWatchdogVerdict::Continue
        );
        assert_eq!(
            watchdog.tick(InstantMillis(15_001)),
            I2pWatchdogVerdict::TransmitKeepalive
        );
        assert_eq!(
            watchdog.tick(InstantMillis(25_001)),
            I2pWatchdogVerdict::DegradeAndTransmitKeepalive
        );
        assert_eq!(
            watchdog.observe_read(InstantMillis(25_002)),
            I2pReadObservation::Recovered
        );
        assert_eq!(
            watchdog.observe_read(InstantMillis(25_003)),
            I2pReadObservation::Responsive
        );
    }

    #[test]
    fn keepalives_do_not_count_as_ordinary_writes() {
        let mut watchdog = I2pIdleWatchdog::start(InstantMillis(0));

        assert_eq!(
            watchdog.tick(InstantMillis(10_001)),
            I2pWatchdogVerdict::TransmitKeepalive
        );
        assert_eq!(
            watchdog.tick(InstantMillis(11_001)),
            I2pWatchdogVerdict::TransmitKeepalive
        );
        watchdog.observe_ordinary_write(InstantMillis(11_001));
        assert_eq!(
            watchdog.tick(InstantMillis(12_001)),
            I2pWatchdogVerdict::Continue
        );
    }

    #[test]
    fn read_timeout_disconnects_before_any_other_action() {
        let mut watchdog = I2pIdleWatchdog::start(InstantMillis(7_000));

        assert_eq!(
            watchdog.tick(InstantMillis(117_000)),
            I2pWatchdogVerdict::DegradeAndTransmitKeepalive
        );
        assert_eq!(
            watchdog.tick(InstantMillis(117_001)),
            I2pWatchdogVerdict::Disconnect
        );
    }
}
