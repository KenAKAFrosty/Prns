const CONNECTION_FAILURES_BEFORE_SCAN: u8 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessPoint {
    pub(crate) bssid: [u8; 6],
    pub(crate) channel: u8,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConnectionTarget {
    Direct,
    Pinned(AccessPoint),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StationAttempt {
    Connect(ConnectionTarget),
    Scan,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConnectionFailure {
    NetworkNotFound,
    Authentication,
    Timeout,
    Driver,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScanFailure {
    Timeout,
    Driver,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AttemptOutcome {
    Connected(AccessPoint),
    ConnectionFailed(ConnectionFailure),
    ScanCompleted(Option<AccessPoint>),
    ScanFailed(ScanFailure),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StationYield {
    MonitorLink,
    RetryDelay,
}

pub(crate) struct StationRecovery {
    pinned: Option<AccessPoint>,
    connection_failures: u8,
}

impl StationRecovery {
    pub(crate) const fn new() -> Self {
        Self {
            pinned: None,
            connection_failures: 0,
        }
    }

    pub(crate) fn next_attempt(&mut self) -> StationAttempt {
        if self.connection_failures >= CONNECTION_FAILURES_BEFORE_SCAN {
            self.connection_failures = 0;
            return StationAttempt::Scan;
        }
        match self.pinned.as_ref() {
            Some(access_point) => {
                StationAttempt::Connect(ConnectionTarget::Pinned(access_point.clone()))
            }
            None => StationAttempt::Connect(ConnectionTarget::Direct),
        }
    }

    pub(crate) fn complete(&mut self, outcome: AttemptOutcome) -> StationYield {
        match outcome {
            AttemptOutcome::Connected(access_point) => {
                self.pinned = Some(access_point);
                self.connection_failures = 0;
                StationYield::MonitorLink
            }
            AttemptOutcome::ConnectionFailed(_) => {
                self.connection_failures = self.connection_failures.saturating_add(1);
                StationYield::RetryDelay
            }
            AttemptOutcome::ScanCompleted(access_point) => {
                self.pinned = access_point;
                self.connection_failures = 0;
                StationYield::RetryDelay
            }
            AttemptOutcome::ScanFailed(_) => {
                self.pinned = None;
                self.connection_failures = 0;
                StationYield::RetryDelay
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access_point(channel: u8) -> AccessPoint {
        AccessPoint {
            bssid: [channel; 6],
            channel,
        }
    }

    fn fail(recovery: &mut StationRecovery, failure: ConnectionFailure) {
        assert_eq!(
            recovery.complete(AttemptOutcome::ConnectionFailed(failure)),
            StationYield::RetryDelay
        );
    }

    #[test]
    fn first_attempt_connects_directly() {
        let mut recovery = StationRecovery::new();

        assert_eq!(
            recovery.next_attempt(),
            StationAttempt::Connect(ConnectionTarget::Direct)
        );
    }

    #[test]
    fn successful_connection_pins_reconnect() {
        let mut recovery = StationRecovery::new();
        let selected = access_point(6);

        assert_eq!(
            recovery.complete(AttemptOutcome::Connected(selected.clone())),
            StationYield::MonitorLink
        );
        assert_eq!(
            recovery.next_attempt(),
            StationAttempt::Connect(ConnectionTarget::Pinned(selected))
        );
    }

    #[test]
    fn fallback_scan_is_bounded_by_repeated_connection_failures() {
        let mut recovery = StationRecovery::new();

        for _ in 0..CONNECTION_FAILURES_BEFORE_SCAN {
            assert_eq!(
                recovery.next_attempt(),
                StationAttempt::Connect(ConnectionTarget::Direct)
            );
            fail(&mut recovery, ConnectionFailure::Driver);
        }
        assert_eq!(recovery.next_attempt(), StationAttempt::Scan);
        assert_eq!(
            recovery.complete(AttemptOutcome::ScanCompleted(Some(access_point(11)))),
            StationYield::RetryDelay
        );
        assert_eq!(
            recovery.next_attempt(),
            StationAttempt::Connect(ConnectionTarget::Pinned(access_point(11)))
        );
    }

    #[test]
    fn absent_ssid_returns_to_direct_connection_after_bounded_scan() {
        let mut recovery = StationRecovery::new();

        for _ in 0..CONNECTION_FAILURES_BEFORE_SCAN {
            let _ = recovery.next_attempt();
            fail(&mut recovery, ConnectionFailure::NetworkNotFound);
        }
        assert_eq!(recovery.next_attempt(), StationAttempt::Scan);
        assert_eq!(
            recovery.complete(AttemptOutcome::ScanCompleted(None)),
            StationYield::RetryDelay
        );
        assert_eq!(
            recovery.next_attempt(),
            StationAttempt::Connect(ConnectionTarget::Direct)
        );
    }

    #[test]
    fn authentication_failure_delays_before_retry() {
        let mut recovery = StationRecovery::new();

        fail(&mut recovery, ConnectionFailure::Authentication);
        assert_eq!(
            recovery.next_attempt(),
            StationAttempt::Connect(ConnectionTarget::Direct)
        );
    }

    #[test]
    fn connection_timeout_delays_before_retry() {
        let mut recovery = StationRecovery::new();

        fail(&mut recovery, ConnectionFailure::Timeout);
        assert_eq!(
            recovery.next_attempt(),
            StationAttempt::Connect(ConnectionTarget::Direct)
        );
    }

    #[test]
    fn scan_errors_delay_and_clear_a_stale_pin() {
        let mut recovery = StationRecovery::new();
        let selected = access_point(1);
        let _ = recovery.complete(AttemptOutcome::Connected(selected));
        for _ in 0..CONNECTION_FAILURES_BEFORE_SCAN {
            let _ = recovery.next_attempt();
            fail(&mut recovery, ConnectionFailure::Driver);
        }
        assert_eq!(recovery.next_attempt(), StationAttempt::Scan);
        assert_eq!(
            recovery.complete(AttemptOutcome::ScanFailed(ScanFailure::Timeout)),
            StationYield::RetryDelay
        );
        assert_eq!(
            recovery.next_attempt(),
            StationAttempt::Connect(ConnectionTarget::Direct)
        );
    }

    #[test]
    fn repeated_recovery_can_repin_and_return_to_direct_connection() {
        let mut recovery = StationRecovery::new();

        let _ = recovery.complete(AttemptOutcome::Connected(access_point(1)));
        for _ in 0..CONNECTION_FAILURES_BEFORE_SCAN {
            let _ = recovery.next_attempt();
            fail(&mut recovery, ConnectionFailure::Driver);
        }
        assert_eq!(recovery.next_attempt(), StationAttempt::Scan);
        let _ = recovery.complete(AttemptOutcome::ScanCompleted(Some(access_point(6))));
        assert_eq!(
            recovery.next_attempt(),
            StationAttempt::Connect(ConnectionTarget::Pinned(access_point(6)))
        );
        for _ in 0..CONNECTION_FAILURES_BEFORE_SCAN {
            fail(&mut recovery, ConnectionFailure::Driver);
            if recovery.connection_failures < CONNECTION_FAILURES_BEFORE_SCAN {
                let _ = recovery.next_attempt();
            }
        }
        assert_eq!(recovery.next_attempt(), StationAttempt::Scan);
        let _ = recovery.complete(AttemptOutcome::ScanCompleted(None));
        assert_eq!(
            recovery.next_attempt(),
            StationAttempt::Connect(ConnectionTarget::Direct)
        );
    }

    #[test]
    fn every_error_outcome_requires_a_retry_delay() {
        let outcomes = [
            AttemptOutcome::ConnectionFailed(ConnectionFailure::NetworkNotFound),
            AttemptOutcome::ConnectionFailed(ConnectionFailure::Authentication),
            AttemptOutcome::ConnectionFailed(ConnectionFailure::Timeout),
            AttemptOutcome::ConnectionFailed(ConnectionFailure::Driver),
            AttemptOutcome::ScanCompleted(None),
            AttemptOutcome::ScanFailed(ScanFailure::Timeout),
            AttemptOutcome::ScanFailed(ScanFailure::Driver),
        ];

        for outcome in outcomes {
            let mut recovery = StationRecovery::new();
            assert_eq!(recovery.complete(outcome), StationYield::RetryDelay);
        }
    }
}
