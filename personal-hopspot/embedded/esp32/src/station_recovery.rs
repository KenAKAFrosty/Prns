const FIRST_2_4_GHZ_CHANNEL: u8 = 1;
const LAST_2_4_GHZ_CHANNEL: u8 = 13;

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProtectedChannel(u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiscoveryScope {
    FullBand,
    #[cfg(test)]
    Protected(ProtectedChannel),
}

impl DiscoveryScope {
    #[cfg(test)]
    pub(crate) fn protected(channel: u8) -> Option<Self> {
        (FIRST_2_4_GHZ_CHANNEL..=LAST_2_4_GHZ_CHANNEL)
            .contains(&channel)
            .then_some(Self::Protected(ProtectedChannel(channel)))
    }

    const fn first_channel(self) -> u8 {
        match self {
            Self::FullBand => FIRST_2_4_GHZ_CHANNEL,
            #[cfg(test)]
            Self::Protected(channel) => channel.0,
        }
    }

    fn permits(self, channel: u8) -> bool {
        match self {
            Self::FullBand => (FIRST_2_4_GHZ_CHANNEL..=LAST_2_4_GHZ_CHANNEL).contains(&channel),
            #[cfg(test)]
            Self::Protected(protected) => channel == protected.0,
        }
    }

    fn ends_sweep(self, channel: u8) -> bool {
        match self {
            Self::FullBand => channel == LAST_2_4_GHZ_CHANNEL,
            #[cfg(test)]
            Self::Protected(protected) => channel == protected.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessPoint {
    pub(crate) bssid: [u8; 6],
    pub(crate) channel: u8,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScanAttempt {
    channel: u8,
    starts_sweep: bool,
    ends_sweep: bool,
}

impl ScanAttempt {
    pub(crate) fn channel(&self) -> u8 {
        self.channel
    }

    pub(crate) fn starts_sweep(&self) -> bool {
        self.starts_sweep
    }

    pub(crate) fn ends_sweep(&self) -> bool {
        self.ends_sweep
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
    discovery_scope: DiscoveryScope,
    next_retry_delay: RecoveryDelay,
}

impl StationRecovery {
    pub(crate) const fn new(discovery_scope: DiscoveryScope) -> Self {
        Self {
            phase: StationPhase::Discover(discovery_scope.first_channel()),
            discovery_scope,
            next_retry_delay: RecoveryDelay::TwoSeconds,
        }
    }

    pub(crate) fn set_discovery_scope(&mut self, discovery_scope: DiscoveryScope) {
        if self.discovery_scope == discovery_scope {
            return;
        }
        let first_channel = discovery_scope.first_channel();
        let phase = core::mem::replace(&mut self.phase, StationPhase::Active);
        self.phase = match phase {
            StationPhase::Discover(_) => StationPhase::Discover(first_channel),
            StationPhase::Pinned(access_point)
                if !discovery_scope.permits(access_point.channel) =>
            {
                StationPhase::Discover(first_channel)
            }
            phase => phase,
        };
        self.discovery_scope = discovery_scope;
    }

    pub(crate) fn begin_attempt(&mut self) -> Option<StationAttempt> {
        let phase = core::mem::replace(&mut self.phase, StationPhase::Active);
        match phase {
            StationPhase::Discover(channel) => Some(StationAttempt::Scan(ScanAttempt {
                channel,
                starts_sweep: channel == self.discovery_scope.first_channel(),
                ends_sweep: self.discovery_scope.ends_sweep(channel),
            })),
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
            ScanOutcome::Found(access_point)
                if self.discovery_scope.permits(access_point.channel) =>
            {
                self.phase = StationPhase::Pinned(access_point);
                self.reset_retry_delay();
                StationYield::Continue
            }
            ScanOutcome::Found(_) | ScanOutcome::NotFound | ScanOutcome::Failed(_) => {
                if !attempt.ends_sweep {
                    self.phase = StationPhase::Discover(attempt.channel + 1);
                    StationYield::InterChannel
                } else {
                    self.phase = StationPhase::Discover(self.discovery_scope.first_channel());
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
                self.phase = StationPhase::Discover(self.discovery_scope.first_channel());
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

    fn full_band_recovery() -> StationRecovery {
        StationRecovery::new(DiscoveryScope::FullBand)
    }

    fn protected_recovery(channel: u8) -> StationRecovery {
        StationRecovery::new(DiscoveryScope::protected(channel).unwrap())
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
        let mut recovery = full_band_recovery();
        let attempt = scan(&mut recovery);

        assert_eq!(attempt.channel(), FIRST_2_4_GHZ_CHANNEL);
        assert!(attempt.starts_sweep());
        assert!(!attempt.ends_sweep());
    }

    #[test]
    fn only_one_operation_can_be_active() {
        let mut recovery = full_band_recovery();
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
        let mut recovery = full_band_recovery();

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
        let mut recovery = full_band_recovery();

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
    fn protected_discovery_only_probes_its_channel() {
        let mut recovery = protected_recovery(6);

        for delay in [RecoveryDelay::TwoSeconds, RecoveryDelay::TenSeconds] {
            let attempt = scan(&mut recovery);
            assert_eq!(attempt.channel(), 6);
            assert!(attempt.starts_sweep());
            assert!(attempt.ends_sweep());
            assert_eq!(
                recovery.finish_scan(attempt, ScanOutcome::NotFound),
                StationYield::Retry(delay)
            );
        }
    }

    #[test]
    fn protected_discovery_uses_bounded_retry_delays() {
        let mut recovery = protected_recovery(11);

        for delay in [
            RecoveryDelay::TwoSeconds,
            RecoveryDelay::TenSeconds,
            RecoveryDelay::ThirtySeconds,
            RecoveryDelay::TwoMinutes,
            RecoveryDelay::FiveMinutes,
            RecoveryDelay::FiveMinutes,
        ] {
            let attempt = scan(&mut recovery);
            assert_eq!(attempt.channel(), 11);
            assert_eq!(
                recovery.finish_scan(attempt, ScanOutcome::NotFound),
                StationYield::Retry(delay)
            );
        }
    }

    #[test]
    fn protected_discovery_connects_to_a_same_channel_result() {
        let mut recovery = protected_recovery(6);
        let selected = access_point(6);

        discover(&mut recovery, selected.clone());
        let attempt = connect(&mut recovery);
        assert_eq!(attempt.access_point(), &selected);
        assert_eq!(
            recovery.finish_connection(attempt, ConnectionOutcome::Connected(selected.clone())),
            StationYield::MonitorLink
        );
        assert_eq!(connect(&mut recovery).access_point(), &selected);
    }

    #[test]
    fn protected_discovery_rejects_an_off_channel_result() {
        let mut recovery = protected_recovery(6);
        let attempt = scan(&mut recovery);

        assert_eq!(
            recovery.finish_scan(attempt, ScanOutcome::Found(access_point(11))),
            StationYield::Retry(RecoveryDelay::TwoSeconds)
        );
        assert_eq!(scan(&mut recovery).channel(), 6);
    }

    #[test]
    fn protected_scan_cancellation_preserves_the_channel() {
        let mut recovery = protected_recovery(6);
        let attempt = scan(&mut recovery);

        assert_eq!(
            recovery.finish_scan(attempt, ScanOutcome::Cancelled),
            StationYield::Disabled
        );
        recovery.resume_now();
        assert_eq!(scan(&mut recovery).channel(), 6);
    }

    #[test]
    fn full_band_discovery_is_restored_after_protection_ends() {
        let mut recovery = protected_recovery(6);
        let attempt = scan(&mut recovery);
        assert_eq!(
            recovery.finish_scan(attempt, ScanOutcome::NotFound),
            StationYield::Retry(RecoveryDelay::TwoSeconds)
        );

        recovery.set_discovery_scope(DiscoveryScope::FullBand);

        let attempt = scan(&mut recovery);
        assert_eq!(attempt.channel(), FIRST_2_4_GHZ_CHANNEL);
        assert_eq!(
            recovery.finish_scan(attempt, ScanOutcome::NotFound),
            StationYield::InterChannel
        );
        assert_eq!(scan(&mut recovery).channel(), FIRST_2_4_GHZ_CHANNEL + 1);
    }

    #[test]
    fn a_changed_protected_channel_discards_an_off_channel_pin() {
        let mut recovery = protected_recovery(6);
        discover(&mut recovery, access_point(6));

        recovery.set_discovery_scope(DiscoveryScope::protected(11).unwrap());

        assert_eq!(scan(&mut recovery).channel(), 11);
    }

    #[test]
    fn protected_discovery_rejects_non_24ghz_channels() {
        assert_eq!(DiscoveryScope::protected(0), None);
        assert_eq!(DiscoveryScope::protected(14), None);
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
        let mut recovery = full_band_recovery();
        let selected = access_point(6);

        discover(&mut recovery, selected.clone());

        assert_eq!(connect(&mut recovery).access_point(), &selected);
    }

    #[test]
    fn successful_connection_pins_reconnect_and_resets_backoff() {
        let mut recovery = full_band_recovery();
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
        let mut recovery = full_band_recovery();
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
        let mut recovery = full_band_recovery();
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
        let mut recovery = full_band_recovery();
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
            let mut recovery = full_band_recovery();
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
            let mut recovery = full_band_recovery();
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
        let mut recovery = full_band_recovery();
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
        let mut recovery = full_band_recovery();
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
        let mut recovery = full_band_recovery();
        complete_empty_sweep(&mut recovery, RecoveryDelay::TwoSeconds);
        complete_empty_sweep(&mut recovery, RecoveryDelay::TenSeconds);

        recovery.resume_now();

        complete_empty_sweep(&mut recovery, RecoveryDelay::TwoSeconds);
    }
}
