// The station health task samples every two seconds. A full driver restart is expensive and the
// vendor driver can hold TX capacity for several samples during otherwise healthy coexistence
// bursts, so require roughly 18 seconds of uninterrupted hard evidence before escalating.
const HARD_STALL_WINDOWS_BEFORE_RECOVERY: u8 = 9;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StationDataPathWindow {
    ReceiveProgress,
    TransmitWithoutReceive,
    TransmitCapacityBlocked,
    TransmitSubmissionStalled,
    NoProgress,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DriverRestartCause {
    TransmitCapacityBlocked,
    TransmitSubmissionStalled,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StationDataPathAction {
    Continue,
    RestartDriver {
        count: usize,
        cause: DriverRestartCause,
    },
}

pub(crate) struct StationDataPathRecovery {
    transmit_capacity_blocked_windows: u8,
    transmit_submission_stalled_windows: u8,
    driver_restarts: usize,
}

impl StationDataPathRecovery {
    pub(crate) const fn new() -> Self {
        Self {
            transmit_capacity_blocked_windows: 0,
            transmit_submission_stalled_windows: 0,
            driver_restarts: 0,
        }
    }

    pub(crate) fn observe(&mut self, window: StationDataPathWindow) -> StationDataPathAction {
        match window {
            StationDataPathWindow::ReceiveProgress => {
                self.transmit_capacity_blocked_windows = 0;
                self.transmit_submission_stalled_windows = 0;
                StationDataPathAction::Continue
            }
            StationDataPathWindow::TransmitWithoutReceive => {
                // An RX counter can prove progress, but its absence cannot prove failure. A quiet
                // LAN, bursty peer announcements, multicast filtering, and AP buffering all
                // legitimately produce TX-only windows. Restarting from this asymmetry was the
                // source of the periodic disconnect/route-retirement loop.
                self.transmit_capacity_blocked_windows = 0;
                self.transmit_submission_stalled_windows = 0;
                StationDataPathAction::Continue
            }
            StationDataPathWindow::TransmitCapacityBlocked => {
                self.transmit_submission_stalled_windows = 0;
                self.transmit_capacity_blocked_windows =
                    self.transmit_capacity_blocked_windows.saturating_add(1);
                if self.transmit_capacity_blocked_windows < HARD_STALL_WINDOWS_BEFORE_RECOVERY {
                    return StationDataPathAction::Continue;
                }
                self.transmit_capacity_blocked_windows = 0;
                self.driver_restarts = self.driver_restarts.saturating_add(1);
                StationDataPathAction::RestartDriver {
                    count: self.driver_restarts,
                    cause: DriverRestartCause::TransmitCapacityBlocked,
                }
            }
            StationDataPathWindow::TransmitSubmissionStalled => {
                self.transmit_capacity_blocked_windows = 0;
                self.transmit_submission_stalled_windows =
                    self.transmit_submission_stalled_windows.saturating_add(1);
                if self.transmit_submission_stalled_windows < HARD_STALL_WINDOWS_BEFORE_RECOVERY {
                    return StationDataPathAction::Continue;
                }
                self.transmit_submission_stalled_windows = 0;
                self.driver_restarts = self.driver_restarts.saturating_add(1);
                StationDataPathAction::RestartDriver {
                    count: self.driver_restarts,
                    cause: DriverRestartCause::TransmitSubmissionStalled,
                }
            }
            StationDataPathWindow::NoProgress => {
                self.transmit_capacity_blocked_windows = 0;
                self.transmit_submission_stalled_windows = 0;
                StationDataPathAction::Continue
            }
        }
    }

    pub(crate) fn station_unavailable(&mut self) {
        self.transmit_capacity_blocked_windows = 0;
        self.transmit_submission_stalled_windows = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_silence_never_restarts_the_driver() {
        let mut recovery = StationDataPathRecovery::new();

        for _ in 0..64 {
            assert_eq!(
                recovery.observe(StationDataPathWindow::TransmitWithoutReceive),
                StationDataPathAction::Continue
            );
        }
    }

    #[test]
    fn brief_transmit_capacity_backpressure_does_not_restart_the_driver() {
        let mut recovery = StationDataPathRecovery::new();

        for _ in 0..HARD_STALL_WINDOWS_BEFORE_RECOVERY - 1 {
            assert_eq!(
                recovery.observe(StationDataPathWindow::TransmitCapacityBlocked),
                StationDataPathAction::Continue
            );
        }
    }

    #[test]
    fn persistent_transmit_capacity_block_restarts_the_driver() {
        let mut recovery = StationDataPathRecovery::new();

        for _ in 0..HARD_STALL_WINDOWS_BEFORE_RECOVERY - 1 {
            assert_eq!(
                recovery.observe(StationDataPathWindow::TransmitCapacityBlocked),
                StationDataPathAction::Continue
            );
        }
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitCapacityBlocked),
            StationDataPathAction::RestartDriver {
                count: 1,
                cause: DriverRestartCause::TransmitCapacityBlocked,
            }
        );
    }

    #[test]
    fn persistent_transmit_submission_stall_restarts_the_driver() {
        let mut recovery = StationDataPathRecovery::new();

        for _ in 0..HARD_STALL_WINDOWS_BEFORE_RECOVERY - 1 {
            assert_eq!(
                recovery.observe(StationDataPathWindow::TransmitSubmissionStalled),
                StationDataPathAction::Continue
            );
        }
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitSubmissionStalled),
            StationDataPathAction::RestartDriver {
                count: 1,
                cause: DriverRestartCause::TransmitSubmissionStalled,
            }
        );
    }

    #[test]
    fn unrelated_window_clears_transmit_capacity_evidence() {
        let mut recovery = StationDataPathRecovery::new();

        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitCapacityBlocked),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::NoProgress),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitCapacityBlocked),
            StationDataPathAction::Continue
        );
    }

    #[test]
    fn unavailable_station_clears_hard_stall_evidence() {
        let mut recovery = StationDataPathRecovery::new();

        for _ in 0..HARD_STALL_WINDOWS_BEFORE_RECOVERY - 1 {
            assert_eq!(
                recovery.observe(StationDataPathWindow::TransmitCapacityBlocked),
                StationDataPathAction::Continue
            );
        }
        recovery.station_unavailable();
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitCapacityBlocked),
            StationDataPathAction::Continue
        );
    }

    #[test]
    fn receive_progress_clears_transmit_submission_stall_evidence() {
        let mut recovery = StationDataPathRecovery::new();

        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitSubmissionStalled),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::ReceiveProgress),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitSubmissionStalled),
            StationDataPathAction::Continue
        );
    }
}
