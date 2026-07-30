const FIRST_2_4_GHZ_CHANNEL: u8 = 1;
const LAST_2_4_GHZ_CHANNEL: u8 = 13;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessPoint {
    pub(crate) bssid: [u8; 6],
    pub(crate) channel: u8,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScanAttempt {
    channel: u8,
}

impl ScanAttempt {
    pub(crate) fn channel(&self) -> u8 {
        self.channel
    }

    pub(crate) fn starts_sweep(&self) -> bool {
        self.channel == FIRST_2_4_GHZ_CHANNEL
    }

    pub(crate) fn ends_sweep(&self) -> bool {
        self.channel == LAST_2_4_GHZ_CHANNEL
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ConnectionAttempt {
    access_point: AccessPoint,
}

impl ConnectionAttempt {
    pub(crate) fn access_point(&self) -> &AccessPoint {
        &self.access_point
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StationAttempt {
    Scan(ScanAttempt),
    Connect(ConnectionAttempt),
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
pub(crate) enum ScanOutcome {
    Found(AccessPoint),
    NotFound,
    Failed(ScanFailure),
    Cancelled,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConnectionOutcome {
    Connected(AccessPoint),
    Failed(ConnectionFailure),
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryDelay {
    TwoSeconds,
    TenSeconds,
    ThirtySeconds,
    TwoMinutes,
    FiveMinutes,
}

impl RecoveryDelay {
    pub(crate) fn seconds(self) -> u64 {
        match self {
            Self::TwoSeconds => 2,
            Self::TenSeconds => 10,
            Self::ThirtySeconds => 30,
            Self::TwoMinutes => 120,
            Self::FiveMinutes => 300,
        }
    }

    fn following(self) -> Self {
        match self {
            Self::TwoSeconds => Self::TenSeconds,
            Self::TenSeconds => Self::ThirtySeconds,
            Self::ThirtySeconds => Self::TwoMinutes,
            Self::TwoMinutes | Self::FiveMinutes => Self::FiveMinutes,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StationYield {
    Continue,
    InterChannel,
    Retry(RecoveryDelay),
    MonitorLink,
    Disabled,
}

enum StationPhase {
    Discover(u8),
    Pinned(AccessPoint),
    Active,
}

pub(crate) struct StationRecovery {
    phase: StationPhase,
    next_retry_delay: RecoveryDelay,
}

impl StationRecovery {
    pub(crate) const fn new() -> Self {
        Self {
            phase: StationPhase::Discover(FIRST_2_4_GHZ_CHANNEL),
            next_retry_delay: RecoveryDelay::TwoSeconds,
        }
    }

    pub(crate) fn begin_attempt(&mut self) -> Option<StationAttempt> {
        let phase = core::mem::replace(&mut self.phase, StationPhase::Active);
        match phase {
            StationPhase::Discover(channel) => Some(StationAttempt::Scan(ScanAttempt { channel })),
            StationPhase::Pinned(access_point) => {
                Some(StationAttempt::Connect(ConnectionAttempt { access_point }))
            }
            StationPhase::Active => None,
        }
    }

    pub(crate) fn finish_scan(
        &mut self,
        attempt: ScanAttempt,
        outcome: ScanOutcome,
    ) -> StationYield {
        debug_assert!(matches!(self.phase, StationPhase::Active));
        match outcome {
            ScanOutcome::Found(access_point) => {
                self.phase = StationPhase::Pinned(access_point);
                self.reset_retry_delay();
                StationYield::Continue
            }
            ScanOutcome::NotFound | ScanOutcome::Failed(_) => {
                if attempt.channel < LAST_2_4_GHZ_CHANNEL {
                    self.phase = StationPhase::Discover(attempt.channel + 1);
                    StationYield::InterChannel
                } else {
                    self.phase = StationPhase::Discover(FIRST_2_4_GHZ_CHANNEL);
                    StationYield::Retry(self.take_retry_delay())
                }
            }
            ScanOutcome::Cancelled => {
                self.phase = StationPhase::Discover(attempt.channel);
                StationYield::Disabled
            }
        }
    }

    pub(crate) fn finish_connection(
        &mut self,
        attempt: ConnectionAttempt,
        outcome: ConnectionOutcome,
    ) -> StationYield {
        debug_assert!(matches!(self.phase, StationPhase::Active));
        match outcome {
            ConnectionOutcome::Connected(access_point) => {
                self.phase = StationPhase::Pinned(access_point);
                self.reset_retry_delay();
                StationYield::MonitorLink
            }
            ConnectionOutcome::Failed(ConnectionFailure::Authentication) => {
                self.phase = StationPhase::Pinned(attempt.access_point);
                StationYield::Retry(self.take_retry_delay())
            }
            ConnectionOutcome::Failed(
                ConnectionFailure::NetworkNotFound
                | ConnectionFailure::Timeout
                | ConnectionFailure::Driver,
            ) => {
                self.phase = StationPhase::Discover(FIRST_2_4_GHZ_CHANNEL);
                StationYield::Retry(self.take_retry_delay())
            }
            ConnectionOutcome::Cancelled => {
                self.phase = StationPhase::Pinned(attempt.access_point);
                StationYield::Disabled
            }
        }
    }

    pub(crate) fn resume_now(&mut self) {
        self.reset_retry_delay();
    }

    fn take_retry_delay(&mut self) -> RecoveryDelay {
        let delay = self.next_retry_delay;
        self.next_retry_delay = delay.following();
        delay
    }

    fn reset_retry_delay(&mut self) {
        self.next_retry_delay = RecoveryDelay::TwoSeconds;
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

    fn scan(recovery: &mut StationRecovery) -> ScanAttempt {
        let Some(StationAttempt::Scan(attempt)) = recovery.begin_attempt() else {
            panic!("expected scan");
        };
        attempt
    }

    fn connect(recovery: &mut StationRecovery) -> ConnectionAttempt {
        let Some(StationAttempt::Connect(attempt)) = recovery.begin_attempt() else {
            panic!("expected connection");
        };
        attempt
    }

    fn discover(recovery: &mut StationRecovery, selected: AccessPoint) {
        let attempt = scan(recovery);
        assert_eq!(
            recovery.finish_scan(attempt, ScanOutcome::Found(selected)),
            StationYield::Continue
        );
    }

    fn complete_empty_sweep(recovery: &mut StationRecovery, expected_delay: RecoveryDelay) {
        for channel in FIRST_2_4_GHZ_CHANNEL..LAST_2_4_GHZ_CHANNEL {
            let attempt = scan(recovery);
            assert_eq!(attempt.channel(), channel);
            assert_eq!(
                recovery.finish_scan(attempt, ScanOutcome::NotFound),
                StationYield::InterChannel
            );
        }
        let attempt = scan(recovery);
        assert_eq!(attempt.channel(), LAST_2_4_GHZ_CHANNEL);
        assert!(attempt.ends_sweep());
        assert_eq!(
            recovery.finish_scan(attempt, ScanOutcome::NotFound),
            StationYield::Retry(expected_delay)
        );
    }

    #[test]
    fn discovery_starts_on_first_channel() {
        let mut recovery = StationRecovery::new();
        let attempt = scan(&mut recovery);

        assert_eq!(attempt.channel(), FIRST_2_4_GHZ_CHANNEL);
        assert!(attempt.starts_sweep());
        assert!(!attempt.ends_sweep());
    }

    #[test]
    fn only_one_operation_can_be_active() {
        let mut recovery = StationRecovery::new();
        let attempt = scan(&mut recovery);

        assert_eq!(recovery.begin_attempt(), None);
        assert_eq!(
            recovery.finish_scan(attempt, ScanOutcome::NotFound),
            StationYield::InterChannel
        );
        assert_eq!(scan(&mut recovery).channel(), FIRST_2_4_GHZ_CHANNEL + 1);
    }

    #[test]
    fn discovery_advances_one_channel_at_a_time() {
        let mut recovery = StationRecovery::new();

        for channel in FIRST_2_4_GHZ_CHANNEL..LAST_2_4_GHZ_CHANNEL {
            let attempt = scan(&mut recovery);
            assert_eq!(attempt.channel(), channel);
            assert_eq!(
                recovery.finish_scan(attempt, ScanOutcome::NotFound),
                StationYield::InterChannel
            );
        }
    }

    #[test]
    fn discovery_uses_bounded_exponential_retry_delays() {
        let mut recovery = StationRecovery::new();

        for delay in [
            RecoveryDelay::TwoSeconds,
            RecoveryDelay::TenSeconds,
            RecoveryDelay::ThirtySeconds,
            RecoveryDelay::TwoMinutes,
            RecoveryDelay::FiveMinutes,
            RecoveryDelay::FiveMinutes,
        ] {
            complete_empty_sweep(&mut recovery, delay);
        }
    }

    #[test]
    fn recovery_delays_have_exact_durations() {
        assert_eq!(RecoveryDelay::TwoSeconds.seconds(), 2);
        assert_eq!(RecoveryDelay::TenSeconds.seconds(), 10);
        assert_eq!(RecoveryDelay::ThirtySeconds.seconds(), 30);
        assert_eq!(RecoveryDelay::TwoMinutes.seconds(), 120);
        assert_eq!(RecoveryDelay::FiveMinutes.seconds(), 300);
    }

    #[test]
    fn discovered_access_point_is_the_only_connection_target() {
        let mut recovery = StationRecovery::new();
        let selected = access_point(6);

        discover(&mut recovery, selected.clone());

        assert_eq!(connect(&mut recovery).access_point(), &selected);
    }

    #[test]
    fn successful_connection_pins_reconnect_and_resets_backoff() {
        let mut recovery = StationRecovery::new();
        complete_empty_sweep(&mut recovery, RecoveryDelay::TwoSeconds);
        let selected = access_point(11);
        discover(&mut recovery, selected.clone());
        let attempt = connect(&mut recovery);

        assert_eq!(
            recovery.finish_connection(attempt, ConnectionOutcome::Connected(selected.clone())),
            StationYield::MonitorLink
        );
        assert_eq!(connect(&mut recovery).access_point(), &selected);
    }

    #[test]
    fn authentication_failure_retries_the_same_access_point() {
        let mut recovery = StationRecovery::new();
        let selected = access_point(1);
        discover(&mut recovery, selected.clone());

        for delay in [
            RecoveryDelay::TwoSeconds,
            RecoveryDelay::TenSeconds,
            RecoveryDelay::ThirtySeconds,
            RecoveryDelay::TwoMinutes,
            RecoveryDelay::FiveMinutes,
            RecoveryDelay::FiveMinutes,
        ] {
            let attempt = connect(&mut recovery);
            assert_eq!(attempt.access_point(), &selected);
            assert_eq!(
                recovery.finish_connection(
                    attempt,
                    ConnectionOutcome::Failed(ConnectionFailure::Authentication)
                ),
                StationYield::Retry(delay)
            );
        }
    }

    #[test]
    fn successful_connection_resets_retry_backoff() {
        let mut recovery = StationRecovery::new();
        let selected = access_point(6);
        discover(&mut recovery, selected.clone());
        let attempt = connect(&mut recovery);
        assert_eq!(
            recovery.finish_connection(
                attempt,
                ConnectionOutcome::Failed(ConnectionFailure::Authentication)
            ),
            StationYield::Retry(RecoveryDelay::TwoSeconds)
        );
        let attempt = connect(&mut recovery);
        assert_eq!(
            recovery.finish_connection(
                attempt,
                ConnectionOutcome::Failed(ConnectionFailure::Authentication)
            ),
            StationYield::Retry(RecoveryDelay::TenSeconds)
        );
        let attempt = connect(&mut recovery);
        assert_eq!(
            recovery.finish_connection(attempt, ConnectionOutcome::Connected(selected)),
            StationYield::MonitorLink
        );
        let attempt = connect(&mut recovery);
        assert_eq!(
            recovery.finish_connection(
                attempt,
                ConnectionOutcome::Failed(ConnectionFailure::Authentication)
            ),
            StationYield::Retry(RecoveryDelay::TwoSeconds)
        );
    }

    #[test]
    fn unavailable_pin_returns_to_channel_discovery() {
        let mut recovery = StationRecovery::new();
        discover(&mut recovery, access_point(6));
        let attempt = connect(&mut recovery);

        assert_eq!(
            recovery.finish_connection(
                attempt,
                ConnectionOutcome::Failed(ConnectionFailure::NetworkNotFound)
            ),
            StationYield::Retry(RecoveryDelay::TwoSeconds)
        );
        assert_eq!(scan(&mut recovery).channel(), FIRST_2_4_GHZ_CHANNEL);
    }

    #[test]
    fn timeout_and_driver_failure_return_to_discovery() {
        for failure in [ConnectionFailure::Timeout, ConnectionFailure::Driver] {
            let mut recovery = StationRecovery::new();
            discover(&mut recovery, access_point(6));
            let attempt = connect(&mut recovery);

            assert_eq!(
                recovery.finish_connection(attempt, ConnectionOutcome::Failed(failure)),
                StationYield::Retry(RecoveryDelay::TwoSeconds)
            );
            assert_eq!(scan(&mut recovery).channel(), FIRST_2_4_GHZ_CHANNEL);
        }
    }

    #[test]
    fn scan_failures_advance_instead_of_sticking() {
        for failure in [ScanFailure::Timeout, ScanFailure::Driver] {
            let mut recovery = StationRecovery::new();
            let attempt = scan(&mut recovery);

            assert_eq!(
                recovery.finish_scan(attempt, ScanOutcome::Failed(failure)),
                StationYield::InterChannel
            );
            assert_eq!(scan(&mut recovery).channel(), FIRST_2_4_GHZ_CHANNEL + 1);
        }
    }

    #[test]
    fn scan_cancellation_retries_the_same_channel_after_enable() {
        let mut recovery = StationRecovery::new();
        let attempt = scan(&mut recovery);

        assert_eq!(
            recovery.finish_scan(attempt, ScanOutcome::Cancelled),
            StationYield::Disabled
        );
        recovery.resume_now();
        assert_eq!(scan(&mut recovery).channel(), FIRST_2_4_GHZ_CHANNEL);
    }

    #[test]
    fn connection_cancellation_preserves_the_pin() {
        let mut recovery = StationRecovery::new();
        let selected = access_point(11);
        discover(&mut recovery, selected.clone());
        let attempt = connect(&mut recovery);

        assert_eq!(
            recovery.finish_connection(attempt, ConnectionOutcome::Cancelled),
            StationYield::Disabled
        );
        recovery.resume_now();
        assert_eq!(connect(&mut recovery).access_point(), &selected);
    }

    #[test]
    fn manual_resume_resets_retry_backoff() {
        let mut recovery = StationRecovery::new();
        complete_empty_sweep(&mut recovery, RecoveryDelay::TwoSeconds);
        complete_empty_sweep(&mut recovery, RecoveryDelay::TenSeconds);

        recovery.resume_now();

        complete_empty_sweep(&mut recovery, RecoveryDelay::TwoSeconds);
    }
}
