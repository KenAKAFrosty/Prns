const EVIDENCE_WINDOWS_BEFORE_RECOVERY: u8 = 2;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StationReceptionWindow {
    ReceiveProgress,
    TransmitWithoutReceive,
    TransmitCapacityBlocked,
    NoProgress,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StationReceptionAction {
    Continue,
    Reassert { count: usize },
    RestartDriver { count: usize },
}

pub(crate) struct StationReceptionRecovery {
    stalled_windows: u8,
    transmit_capacity_blocked_windows: u8,
    awaiting_reassertion_result: bool,
    reassertions: usize,
    driver_restarts: usize,
}

impl StationReceptionRecovery {
    pub(crate) const fn new() -> Self {
        Self {
            stalled_windows: 0,
            transmit_capacity_blocked_windows: 0,
            awaiting_reassertion_result: false,
            reassertions: 0,
            driver_restarts: 0,
        }
    }

    pub(crate) fn observe(&mut self, window: StationReceptionWindow) -> StationReceptionAction {
        match window {
            StationReceptionWindow::ReceiveProgress => {
                self.stalled_windows = 0;
                self.transmit_capacity_blocked_windows = 0;
                self.awaiting_reassertion_result = false;
                StationReceptionAction::Continue
            }
            StationReceptionWindow::TransmitWithoutReceive => {
                self.transmit_capacity_blocked_windows = 0;
                self.stalled_windows = self.stalled_windows.saturating_add(1);
                if self.stalled_windows < EVIDENCE_WINDOWS_BEFORE_RECOVERY {
                    return StationReceptionAction::Continue;
                }
                self.stalled_windows = 0;
                if self.awaiting_reassertion_result {
                    self.awaiting_reassertion_result = false;
                    self.driver_restarts = self.driver_restarts.saturating_add(1);
                    return StationReceptionAction::RestartDriver {
                        count: self.driver_restarts,
                    };
                }
                self.awaiting_reassertion_result = true;
                self.reassertions = self.reassertions.saturating_add(1);
                StationReceptionAction::Reassert {
                    count: self.reassertions,
                }
            }
            StationReceptionWindow::TransmitCapacityBlocked => {
                self.stalled_windows = 0;
                self.awaiting_reassertion_result = false;
                self.transmit_capacity_blocked_windows =
                    self.transmit_capacity_blocked_windows.saturating_add(1);
                if self.transmit_capacity_blocked_windows < EVIDENCE_WINDOWS_BEFORE_RECOVERY {
                    return StationReceptionAction::Continue;
                }
                self.transmit_capacity_blocked_windows = 0;
                self.driver_restarts = self.driver_restarts.saturating_add(1);
                StationReceptionAction::RestartDriver {
                    count: self.driver_restarts,
                }
            }
            StationReceptionWindow::NoProgress => {
                self.transmit_capacity_blocked_windows = 0;
                StationReceptionAction::Continue
            }
        }
    }

    pub(crate) fn station_unavailable(&mut self) {
        self.stalled_windows = 0;
        self.transmit_capacity_blocked_windows = 0;
        self.awaiting_reassertion_result = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasserts_after_two_consecutive_stalled_windows() {
        let mut recovery = StationReceptionRecovery::new();

        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Reassert { count: 1 }
        );
    }

    #[test]
    fn receive_progress_clears_stall_evidence() {
        let mut recovery = StationReceptionRecovery::new();

        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::ReceiveProgress),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Continue
        );
    }

    #[test]
    fn idle_window_preserves_stall_evidence() {
        let mut recovery = StationReceptionRecovery::new();

        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::NoProgress),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Reassert { count: 1 }
        );
    }

    #[test]
    fn restarts_driver_when_reassertion_does_not_restore_reception() {
        let mut recovery = StationReceptionRecovery::new();

        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Reassert { count: 1 }
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::RestartDriver { count: 1 }
        );
    }

    #[test]
    fn unavailable_station_clears_pending_restart() {
        let mut recovery = StationReceptionRecovery::new();

        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Reassert { count: 1 }
        );
        recovery.station_unavailable();
        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitWithoutReceive),
            StationReceptionAction::Reassert { count: 2 }
        );
    }

    #[test]
    fn restarts_driver_after_two_transmit_capacity_blocked_windows() {
        let mut recovery = StationReceptionRecovery::new();

        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitCapacityBlocked),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitCapacityBlocked),
            StationReceptionAction::RestartDriver { count: 1 }
        );
    }

    #[test]
    fn unrelated_window_clears_transmit_capacity_evidence() {
        let mut recovery = StationReceptionRecovery::new();

        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitCapacityBlocked),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::NoProgress),
            StationReceptionAction::Continue
        );
        assert_eq!(
            recovery.observe(StationReceptionWindow::TransmitCapacityBlocked),
            StationReceptionAction::Continue
        );
    }
}
