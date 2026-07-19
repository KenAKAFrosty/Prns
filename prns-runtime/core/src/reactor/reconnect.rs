use core::time::Duration;

const MILLIS_PER_SECOND: u128 = 1_000;
const NANOS_PER_MILLI: u32 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectPolicy {
    initial_delay: Duration,
    maximum_delay: Duration,
    stable_reset_after: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconnectPolicyError {
    ZeroInitialDelay,
    MaximumBelowInitial,
    ZeroStableReset,
    SubMillisecondDelay,
    DelayOutOfRange,
}

impl ReconnectPolicy {
    pub const STANDARD: Self = Self {
        initial_delay: Duration::from_millis(250),
        maximum_delay: Duration::from_secs(5),
        stable_reset_after: Duration::from_secs(30),
    };

    pub fn new(
        initial_delay: Duration,
        maximum_delay: Duration,
        stable_reset_after: Duration,
    ) -> Result<Self, ReconnectPolicyError> {
        validate_millis(initial_delay)?;
        validate_millis(maximum_delay)?;
        validate_millis(stable_reset_after)?;
        if initial_delay.is_zero() {
            return Err(ReconnectPolicyError::ZeroInitialDelay);
        }
        if maximum_delay < initial_delay {
            return Err(ReconnectPolicyError::MaximumBelowInitial);
        }
        if maximum_delay.as_millis() > u128::from(u64::MAX) * 2 / 3 {
            return Err(ReconnectPolicyError::DelayOutOfRange);
        }
        if stable_reset_after.is_zero() {
            return Err(ReconnectPolicyError::ZeroStableReset);
        }
        Ok(Self {
            initial_delay,
            maximum_delay,
            stable_reset_after,
        })
    }

    #[must_use]
    pub const fn initial_delay(self) -> Duration {
        self.initial_delay
    }

    #[must_use]
    pub const fn maximum_delay(self) -> Duration {
        self.maximum_delay
    }

    #[must_use]
    pub const fn stable_reset_after(self) -> Duration {
        self.stable_reset_after
    }

    #[must_use]
    pub const fn schedule(self) -> ReconnectSchedule {
        ReconnectSchedule {
            policy: self,
            failed_attempts: 0,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct ReconnectSchedule {
    policy: ReconnectPolicy,
    failed_attempts: u32,
}

impl ReconnectSchedule {
    #[must_use]
    pub fn next_delay(&mut self, mut fill_entropy: impl FnMut(&mut [u8])) -> Duration {
        let nominal_millis = self.nominal_delay().as_millis() as u64;
        let mut entropy = [0u8; 8];
        fill_entropy(&mut entropy);
        let draw = u64::from_le_bytes(entropy);
        let offset = ((u128::from(draw) * (u128::from(nominal_millis) + 1)) >> 64) as u64;
        let jittered_millis = nominal_millis / 2 + offset;
        self.failed_attempts = self.failed_attempts.saturating_add(1);
        Duration::from_millis(jittered_millis)
    }

    pub fn record_connection_lifetime(&mut self, lifetime: Duration) {
        if lifetime >= self.policy.stable_reset_after {
            self.failed_attempts = 0;
        }
    }

    #[must_use]
    pub fn nominal_delay(&self) -> Duration {
        let initial_millis = self.policy.initial_delay.as_millis() as u64;
        let maximum_millis = self.policy.maximum_delay.as_millis() as u64;
        let multiplier = 1u64.checked_shl(self.failed_attempts).unwrap_or(u64::MAX);
        Duration::from_millis(
            initial_millis
                .saturating_mul(multiplier)
                .min(maximum_millis),
        )
    }
}

fn validate_millis(duration: Duration) -> Result<(), ReconnectPolicyError> {
    if !duration.subsec_nanos().is_multiple_of(NANOS_PER_MILLI) {
        return Err(ReconnectPolicyError::SubMillisecondDelay);
    }
    let millis = u128::from(duration.as_secs())
        .checked_mul(MILLIS_PER_SECOND)
        .and_then(|millis| millis.checked_add(u128::from(duration.subsec_millis())))
        .ok_or(ReconnectPolicyError::DelayOutOfRange)?;
    if millis > u128::from(u64::MAX) {
        return Err(ReconnectPolicyError::DelayOutOfRange);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_schedule_grows_to_the_nominal_plateau() {
        let mut schedule = ReconnectPolicy::STANDARD.schedule();
        let nominal = (0..8)
            .map(|_| {
                let delay = schedule.nominal_delay();
                let _ = schedule.next_delay(|bytes| bytes.fill(0));
                delay
            })
            .collect::<std::vec::Vec<_>>();
        assert_eq!(
            nominal,
            std::vec![
                Duration::from_millis(250),
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(5),
                Duration::from_secs(5),
                Duration::from_secs(5),
            ]
        );
    }

    #[test]
    fn standard_plateau_jitter_spans_two_and_a_half_to_seven_and_a_half_seconds() {
        let mut low = ReconnectPolicy::STANDARD.schedule();
        let mut high = ReconnectPolicy::STANDARD.schedule();
        for _ in 0..5 {
            let _ = low.next_delay(|bytes| bytes.fill(0));
            let _ = high.next_delay(|bytes| bytes.fill(u8::MAX));
        }
        assert_eq!(
            low.next_delay(|bytes| bytes.fill(0)),
            Duration::from_millis(2_500)
        );
        assert_eq!(
            high.next_delay(|bytes| bytes.fill(u8::MAX)),
            Duration::from_millis(7_500)
        );
    }

    #[test]
    fn only_a_stable_connection_resets_the_schedule() {
        let mut schedule = ReconnectPolicy::STANDARD.schedule();
        let _ = schedule.next_delay(|bytes| bytes.fill(0));
        let _ = schedule.next_delay(|bytes| bytes.fill(0));
        schedule.record_connection_lifetime(Duration::from_secs(29));
        assert_eq!(schedule.nominal_delay(), Duration::from_secs(1));
        schedule.record_connection_lifetime(Duration::from_secs(30));
        assert_eq!(schedule.nominal_delay(), Duration::from_millis(250));
    }

    #[test]
    fn invalid_policies_are_typed() {
        assert_eq!(
            ReconnectPolicy::new(
                Duration::ZERO,
                Duration::from_secs(5),
                Duration::from_secs(30)
            ),
            Err(ReconnectPolicyError::ZeroInitialDelay)
        );
        assert_eq!(
            ReconnectPolicy::new(
                Duration::from_secs(2),
                Duration::from_secs(1),
                Duration::from_secs(30)
            ),
            Err(ReconnectPolicyError::MaximumBelowInitial)
        );
        assert_eq!(
            ReconnectPolicy::new(
                Duration::from_millis(1),
                Duration::from_secs(1),
                Duration::ZERO
            ),
            Err(ReconnectPolicyError::ZeroStableReset)
        );
        assert_eq!(
            ReconnectPolicy::new(
                Duration::from_nanos(1),
                Duration::from_secs(1),
                Duration::from_secs(30)
            ),
            Err(ReconnectPolicyError::SubMillisecondDelay)
        );
        assert_eq!(
            ReconnectPolicy::new(
                Duration::from_millis(1),
                Duration::from_millis(u64::MAX),
                Duration::from_secs(30)
            ),
            Err(ReconnectPolicyError::DelayOutOfRange)
        );
    }
}
