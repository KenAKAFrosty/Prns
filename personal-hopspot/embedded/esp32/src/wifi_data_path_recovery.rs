const EVIDENCE_WINDOWS_BEFORE_RECOVERY: u8 = 2;

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
    ReceiveStalled,
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
    stalled_windows: u8,
    transmit_capacity_blocked_windows: u8,
    transmit_submission_stalled_windows: u8,
    driver_restarts: usize,
}

impl StationDataPathRecovery {
    pub(crate) const fn new() -> Self {
        Self {
            stalled_windows: 0,
            transmit_capacity_blocked_windows: 0,
            transmit_submission_stalled_windows: 0,
            driver_restarts: 0,
        }
    }

    pub(crate) fn observe(&mut self, window: StationDataPathWindow) -> StationDataPathAction {
        match window {
            StationDataPathWindow::ReceiveProgress => {
                self.stalled_windows = 0;
                self.transmit_capacity_blocked_windows = 0;
                self.transmit_submission_stalled_windows = 0;
                StationDataPathAction::Continue
            }
            StationDataPathWindow::TransmitWithoutReceive => {
                self.transmit_capacity_blocked_windows = 0;
                self.transmit_submission_stalled_windows = 0;
                self.stalled_windows = self.stalled_windows.saturating_add(1);
                if self.stalled_windows < EVIDENCE_WINDOWS_BEFORE_RECOVERY {
                    return StationDataPathAction::Continue;
                }
                self.stalled_windows = 0;
                self.driver_restarts = self.driver_restarts.saturating_add(1);
                StationDataPathAction::RestartDriver {
                    count: self.driver_restarts,
                    cause: DriverRestartCause::ReceiveStalled,
                }
            }
            StationDataPathWindow::TransmitCapacityBlocked => {
                self.stalled_windows = 0;
                self.transmit_submission_stalled_windows = 0;
                self.transmit_capacity_blocked_windows =
                    self.transmit_capacity_blocked_windows.saturating_add(1);
                if self.transmit_capacity_blocked_windows < EVIDENCE_WINDOWS_BEFORE_RECOVERY {
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
                self.stalled_windows = 0;
                self.transmit_capacity_blocked_windows = 0;
                self.transmit_submission_stalled_windows =
                    self.transmit_submission_stalled_windows.saturating_add(1);
                if self.transmit_submission_stalled_windows < EVIDENCE_WINDOWS_BEFORE_RECOVERY {
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
        self.stalled_windows = 0;
        self.transmit_capacity_blocked_windows = 0;
        self.transmit_submission_stalled_windows = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarts_driver_after_two_consecutive_receive_stalled_windows() {
        let mut recovery = StationDataPathRecovery::new();

        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitWithoutReceive),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitWithoutReceive),
            StationDataPathAction::RestartDriver {
                count: 1,
                cause: DriverRestartCause::ReceiveStalled,
            }
        );
    }

    #[test]
    fn receive_progress_clears_stall_evidence() {
        let mut recovery = StationDataPathRecovery::new();

        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitWithoutReceive),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::ReceiveProgress),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitWithoutReceive),
            StationDataPathAction::Continue
        );
    }

    #[test]
    fn idle_window_preserves_stall_evidence() {
        let mut recovery = StationDataPathRecovery::new();

        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitWithoutReceive),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::NoProgress),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitWithoutReceive),
            StationDataPathAction::RestartDriver {
                count: 1,
                cause: DriverRestartCause::ReceiveStalled,
            }
        );
    }

    #[test]
    fn unavailable_station_clears_pending_restart() {
        let mut recovery = StationDataPathRecovery::new();

        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitWithoutReceive),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitWithoutReceive),
            StationDataPathAction::RestartDriver {
                count: 1,
                cause: DriverRestartCause::ReceiveStalled,
            }
        );
        recovery.station_unavailable();
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitWithoutReceive),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitWithoutReceive),
            StationDataPathAction::RestartDriver {
                count: 2,
                cause: DriverRestartCause::ReceiveStalled,
            }
        );
    }

    #[test]
    fn restarts_driver_after_two_transmit_capacity_blocked_windows() {
        let mut recovery = StationDataPathRecovery::new();

        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitCapacityBlocked),
            StationDataPathAction::Continue
        );
        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitCapacityBlocked),
            StationDataPathAction::RestartDriver {
                count: 1,
                cause: DriverRestartCause::TransmitCapacityBlocked,
            }
        );
    }

    #[test]
    fn restarts_driver_after_two_transmit_submission_stalled_windows() {
        let mut recovery = StationDataPathRecovery::new();

        assert_eq!(
            recovery.observe(StationDataPathWindow::TransmitSubmissionStalled),
            StationDataPathAction::Continue
        );
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
