//! Local volatile-frame lifetime metadata. Nothing in this module changes wire bytes.
use crate::engine::InstantMillis;

/// A frame borrowed from a lane, with its original local monotonic enqueue time.
/// The producer and consumer must use the same clock domain.
#[derive(Clone, Copy, Debug)]
pub struct OutboundFrame<'a> {
    pub bytes: &'a [u8],
    pub enqueued_at: Option<InstantMillis>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboundQueueDropReason {
    Expired,
    Disabled,
    PreResume,
    MissingTime,
    ClockRollback,
}

/// Maximum total residence in local volatile queues, including active contention.
/// A consumer may impose a shorter active timeout as an additional bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutboundQueuePolicy {
    max_age_millis: u64,
}

impl OutboundQueuePolicy {
    /// Conservative default for constrained volatile interface queues.
    pub const DEFAULT: Self = Self {
        max_age_millis: 120_000,
    };
    pub const fn new(max_age_millis: u64) -> Option<Self> {
        if max_age_millis == 0 {
            None
        } else {
            Some(Self { max_age_millis })
        }
    }

    pub const fn max_age_millis(self) -> u64 {
        self.max_age_millis
    }

    /// Missing/future timestamps fail closed. Equal resume ticks are rejected
    /// conservatively since the clock cannot order events inside one tick.
    pub fn check(
        self,
        enqueued_at: Option<InstantMillis>,
        now: InstantMillis,
        resume_after: Option<InstantMillis>,
    ) -> Result<u64, OutboundQueueDropReason> {
        let enqueued_at = enqueued_at.ok_or(OutboundQueueDropReason::MissingTime)?;
        let age = now
            .0
            .checked_sub(enqueued_at.0)
            .ok_or(OutboundQueueDropReason::ClockRollback)?;
        if resume_after.is_some_and(|cutoff| enqueued_at.0 <= cutoff.0) {
            return Err(OutboundQueueDropReason::PreResume);
        }
        if age >= self.max_age_millis {
            return Err(OutboundQueueDropReason::Expired);
        }
        Ok(age)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_policy_is_fail_closed_at_clock_and_lifetime_boundaries() {
        let policy = OutboundQueuePolicy::new(120_000).unwrap();
        assert_eq!(
            policy.check(None, InstantMillis(0), None),
            Err(OutboundQueueDropReason::MissingTime)
        );
        assert_eq!(
            policy.check(Some(InstantMillis(2)), InstantMillis(1), None),
            Err(OutboundQueueDropReason::ClockRollback)
        );
        assert_eq!(
            policy.check(Some(InstantMillis(1)), InstantMillis(120_000), None),
            Ok(119_999)
        );
        assert_eq!(
            policy.check(Some(InstantMillis(1)), InstantMillis(120_001), None),
            Err(OutboundQueueDropReason::Expired)
        );
        assert_eq!(
            policy.check(
                Some(InstantMillis(10)),
                InstantMillis(10),
                Some(InstantMillis(10))
            ),
            Err(OutboundQueueDropReason::PreResume)
        );
        assert_eq!(
            policy.check(
                Some(InstantMillis(11)),
                InstantMillis(11),
                Some(InstantMillis(10))
            ),
            Ok(0)
        );
        assert_eq!(
            policy.check(
                Some(InstantMillis(u64::MAX - 1)),
                InstantMillis(u64::MAX),
                None
            ),
            Ok(1)
        );
        assert!(OutboundQueuePolicy::new(0).is_none());
    }
}
