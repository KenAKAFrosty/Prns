//! Frame-independent refresh policy, absolute deadlines, and attempt feedback.

use core::num::{NonZeroU32, NonZeroU64};
use core::sync::atomic::{AtomicU32, Ordering};

static NEXT_PLANNER_ID: AtomicU32 = AtomicU32::new(1);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MonotonicMillis(u64);

impl MonotonicMillis {
    pub const MAX: Self = Self(u64::MAX);

    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    const fn saturating_add(self, duration: NonZeroDuration) -> Self {
        Self(self.0.saturating_add(duration.get()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZeroDuration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NonZeroDuration(NonZeroU64);

impl NonZeroDuration {
    pub const fn new(milliseconds: u64) -> Result<Self, ZeroDuration> {
        match NonZeroU64::new(milliseconds) {
            Some(value) => Ok(Self(value)),
            None => Err(ZeroDuration),
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZeroPartialRefreshLimit;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartialRefreshLimit(NonZeroU32);

impl PartialRefreshLimit {
    pub const fn new(limit: u32) -> Result<Self, ZeroPartialRefreshLimit> {
        match NonZeroU32::new(limit) {
            Some(value) => Ok(Self(value)),
            None => Err(ZeroPartialRefreshLimit),
        }
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationSpacing {
    OperationCompletionOnly,
    AtLeast(NonZeroDuration),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryBackoff {
    NextRenderOpportunity,
    AtLeast(NonZeroDuration),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImmediateDisplayPolicy {
    retry_backoff: RetryBackoff,
}

impl ImmediateDisplayPolicy {
    #[must_use]
    pub const fn new(retry_backoff: RetryBackoff) -> Self {
        Self { retry_backoff }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyError {
    TelemetryMinimumExceedsFullMaximumAge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedPartialWaveformPolicy {
    spacing: PresentationSpacing,
    telemetry_minimum: NonZeroDuration,
    partial_limit: PartialRefreshLimit,
    full_maximum_age: NonZeroDuration,
    retry_backoff: RetryBackoff,
}

impl RetainedPartialWaveformPolicy {
    pub const fn new(
        spacing: PresentationSpacing,
        telemetry_minimum: NonZeroDuration,
        partial_limit: PartialRefreshLimit,
        full_maximum_age: NonZeroDuration,
        retry_backoff: RetryBackoff,
    ) -> Result<Self, PolicyError> {
        if telemetry_minimum.get() > full_maximum_age.get() {
            return Err(PolicyError::TelemetryMinimumExceedsFullMaximumAge);
        }
        Ok(Self {
            spacing,
            telemetry_minimum,
            partial_limit,
            full_maximum_age,
            retry_backoff,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedFullWaveformOnlyPolicy {
    spacing: PresentationSpacing,
    telemetry_minimum: NonZeroDuration,
    retry_backoff: RetryBackoff,
}

impl RetainedFullWaveformOnlyPolicy {
    #[must_use]
    pub const fn new(
        spacing: PresentationSpacing,
        telemetry_minimum: NonZeroDuration,
        retry_backoff: RetryBackoff,
    ) -> Self {
        Self {
            spacing,
            telemetry_minimum,
            retry_backoff,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationPolicy {
    ImmediateDisplay(ImmediateDisplayPolicy),
    RetainedPartialWaveform(RetainedPartialWaveformPolicy),
    RetainedFullWaveformOnly(RetainedFullWaveformOnlyPolicy),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationUrgency {
    Immediate,
    Telemetry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshKind {
    ImmediateDisplay,
    RetainedFullWaveform,
    RetainedPartialWaveform,
}

#[derive(Debug, Eq, PartialEq)]
pub enum RefreshDecision {
    Present(RefreshAttempt),
    DeferredUntil(MonotonicMillis),
}

#[derive(Debug, Eq, PartialEq)]
pub struct RefreshAttempt {
    planner_id: u32,
    attempt_id: u64,
    planned_at: MonotonicMillis,
    kind: RefreshKind,
}

impl RefreshAttempt {
    #[must_use]
    pub const fn kind(&self) -> RefreshKind {
        self.kind
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannerError {
    AttemptInFlight,
    AttemptIdentityExhausted,
    TimeWentBackward,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedbackError {
    ForeignPlanner,
    MissingAttempt,
    StaleAttempt,
    TimeWentBackward,
}

pub struct RefreshPlanner {
    planner_id: u32,
    next_attempt_id: u64,
    policy: PresentationPolicy,
    in_flight: Option<u64>,
    last_attempt_completion: Option<MonotonicMillis>,
    last_successful_presentation: Option<MonotonicMillis>,
    last_successful_full_waveform: Option<MonotonicMillis>,
    successful_partials_since_full: u32,
    retry_not_before: Option<MonotonicMillis>,
    recovery_required: bool,
}

impl RefreshPlanner {
    #[must_use]
    pub fn new(policy: PresentationPolicy) -> Self {
        let planner_id = NEXT_PLANNER_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            planner_id,
            next_attempt_id: 1,
            policy,
            in_flight: None,
            last_attempt_completion: None,
            last_successful_presentation: None,
            last_successful_full_waveform: None,
            successful_partials_since_full: 0,
            retry_not_before: None,
            recovery_required: false,
        }
    }

    pub fn plan(
        &mut self,
        now: MonotonicMillis,
        urgency: PresentationUrgency,
    ) -> Result<RefreshDecision, PlannerError> {
        if self.in_flight.is_some() {
            return Err(PlannerError::AttemptInFlight);
        }
        if self.history_time().is_some_and(|history| now < history) {
            return Err(PlannerError::TimeWentBackward);
        }

        if let Some(deadline) = self.deadline(urgency) {
            if now < deadline {
                return Ok(RefreshDecision::DeferredUntil(deadline));
            }
        }

        let attempt_id = self.next_attempt_id;
        self.next_attempt_id = self
            .next_attempt_id
            .checked_add(1)
            .ok_or(PlannerError::AttemptIdentityExhausted)?;
        let attempt = RefreshAttempt {
            planner_id: self.planner_id,
            attempt_id,
            planned_at: now,
            kind: self.refresh_kind(now),
        };
        self.in_flight = Some(attempt_id);
        Ok(RefreshDecision::Present(attempt))
    }

    pub fn attempt_succeeded(
        &mut self,
        attempt: RefreshAttempt,
        completed_at: MonotonicMillis,
    ) -> Result<(), FeedbackError> {
        self.validate_feedback(&attempt, completed_at)?;
        self.in_flight = None;
        self.last_attempt_completion = Some(completed_at);
        self.last_successful_presentation = Some(completed_at);
        self.retry_not_before = None;
        match attempt.kind {
            RefreshKind::ImmediateDisplay => {}
            RefreshKind::RetainedFullWaveform => {
                self.last_successful_full_waveform = Some(completed_at);
                self.successful_partials_since_full = 0;
                self.recovery_required = false;
            }
            RefreshKind::RetainedPartialWaveform => {
                self.successful_partials_since_full =
                    self.successful_partials_since_full.saturating_add(1);
            }
        }
        Ok(())
    }

    pub fn attempt_failed(
        &mut self,
        attempt: RefreshAttempt,
        completed_at: MonotonicMillis,
    ) -> Result<(), FeedbackError> {
        self.validate_feedback(&attempt, completed_at)?;
        self.in_flight = None;
        self.last_attempt_completion = Some(completed_at);
        self.recovery_required = !matches!(attempt.kind, RefreshKind::ImmediateDisplay);
        self.retry_not_before = match self.retry_backoff() {
            RetryBackoff::NextRenderOpportunity => None,
            RetryBackoff::AtLeast(duration) => Some(completed_at.saturating_add(duration)),
        };
        Ok(())
    }

    pub fn invalidate(&mut self) -> Result<(), PlannerError> {
        if self.in_flight.is_some() {
            return Err(PlannerError::AttemptInFlight);
        }
        if !matches!(self.policy, PresentationPolicy::ImmediateDisplay(_)) {
            self.recovery_required = true;
        }
        Ok(())
    }

    pub(super) fn validate_feedback(
        &self,
        attempt: &RefreshAttempt,
        completed_at: MonotonicMillis,
    ) -> Result<(), FeedbackError> {
        if attempt.planner_id != self.planner_id {
            return Err(FeedbackError::ForeignPlanner);
        }
        let Some(in_flight) = self.in_flight else {
            return Err(FeedbackError::MissingAttempt);
        };
        if attempt.attempt_id != in_flight {
            return Err(FeedbackError::StaleAttempt);
        }
        if completed_at < attempt.planned_at
            || self
                .history_time()
                .is_some_and(|history| completed_at < history)
        {
            return Err(FeedbackError::TimeWentBackward);
        }
        Ok(())
    }

    fn history_time(&self) -> Option<MonotonicMillis> {
        [
            self.last_attempt_completion,
            self.last_successful_presentation,
            self.last_successful_full_waveform,
        ]
        .into_iter()
        .flatten()
        .max()
    }

    fn deadline(&self, urgency: PresentationUrgency) -> Option<MonotonicMillis> {
        let mut deadline = self.retry_not_before;
        let (spacing, telemetry_minimum) = match self.policy {
            PresentationPolicy::ImmediateDisplay(_) => (None, None),
            PresentationPolicy::RetainedPartialWaveform(policy) => {
                (Some(policy.spacing), Some(policy.telemetry_minimum))
            }
            PresentationPolicy::RetainedFullWaveformOnly(policy) => {
                (Some(policy.spacing), Some(policy.telemetry_minimum))
            }
        };
        if let (Some(PresentationSpacing::AtLeast(duration)), Some(last_attempt_completion)) =
            (spacing, self.last_attempt_completion)
        {
            deadline = maximum_deadline(
                deadline,
                Some(last_attempt_completion.saturating_add(duration)),
            );
        }
        if urgency == PresentationUrgency::Telemetry {
            if let (Some(duration), Some(last_success)) =
                (telemetry_minimum, self.last_successful_presentation)
            {
                deadline = maximum_deadline(deadline, Some(last_success.saturating_add(duration)));
            }
        }
        deadline
    }

    fn refresh_kind(&self, now: MonotonicMillis) -> RefreshKind {
        match self.policy {
            PresentationPolicy::ImmediateDisplay(_) => RefreshKind::ImmediateDisplay,
            PresentationPolicy::RetainedFullWaveformOnly(_) => RefreshKind::RetainedFullWaveform,
            PresentationPolicy::RetainedPartialWaveform(policy) => {
                let full_expired = self
                    .last_successful_full_waveform
                    .is_some_and(|full| now >= full.saturating_add(policy.full_maximum_age));
                if self.last_successful_full_waveform.is_none()
                    || self.recovery_required
                    || self.successful_partials_since_full >= policy.partial_limit.get()
                    || full_expired
                {
                    RefreshKind::RetainedFullWaveform
                } else {
                    RefreshKind::RetainedPartialWaveform
                }
            }
        }
    }

    const fn retry_backoff(&self) -> RetryBackoff {
        match self.policy {
            PresentationPolicy::ImmediateDisplay(policy) => policy.retry_backoff,
            PresentationPolicy::RetainedPartialWaveform(policy) => policy.retry_backoff,
            PresentationPolicy::RetainedFullWaveformOnly(policy) => policy.retry_backoff,
        }
    }
}

fn maximum_deadline(
    left: Option<MonotonicMillis>,
    right: Option<MonotonicMillis>,
) -> Option<MonotonicMillis> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retained_partial() -> PresentationPolicy {
        PresentationPolicy::RetainedPartialWaveform(
            RetainedPartialWaveformPolicy::new(
                PresentationSpacing::OperationCompletionOnly,
                NonZeroDuration::new(1).unwrap(),
                PartialRefreshLimit::new(2).unwrap(),
                NonZeroDuration::new(10).unwrap(),
                RetryBackoff::NextRenderOpportunity,
            )
            .unwrap(),
        )
    }

    fn present(
        planner: &mut RefreshPlanner,
        at: u64,
        urgency: PresentationUrgency,
    ) -> RefreshAttempt {
        let RefreshDecision::Present(attempt) =
            planner.plan(MonotonicMillis::new(at), urgency).unwrap()
        else {
            panic!("expected an attempt");
        };
        attempt
    }

    #[test]
    fn retained_partial_history_selects_full_partial_limit_age_and_recovery() {
        let mut planner = RefreshPlanner::new(retained_partial());
        let first = present(&mut planner, 0, PresentationUrgency::Immediate);
        assert_eq!(first.kind(), RefreshKind::RetainedFullWaveform);
        planner
            .attempt_succeeded(first, MonotonicMillis::new(0))
            .unwrap();

        for at in [1, 2] {
            let partial = present(&mut planner, at, PresentationUrgency::Immediate);
            assert_eq!(partial.kind(), RefreshKind::RetainedPartialWaveform);
            planner
                .attempt_succeeded(partial, MonotonicMillis::new(at))
                .unwrap();
        }
        let cleanup = present(&mut planner, 3, PresentationUrgency::Immediate);
        assert_eq!(cleanup.kind(), RefreshKind::RetainedFullWaveform);
        planner
            .attempt_succeeded(cleanup, MonotonicMillis::new(3))
            .unwrap();

        let aged = present(&mut planner, 13, PresentationUrgency::Immediate);
        assert_eq!(aged.kind(), RefreshKind::RetainedFullWaveform);
        planner
            .attempt_failed(aged, MonotonicMillis::new(13))
            .unwrap();
        let recovery = present(&mut planner, 14, PresentationUrgency::Immediate);
        assert_eq!(recovery.kind(), RefreshKind::RetainedFullWaveform);
    }

    #[test]
    fn telemetry_and_hardware_deadlines_return_the_latest_absolute_deadline() {
        let five = NonZeroDuration::new(5).unwrap();
        let ten = NonZeroDuration::new(10).unwrap();
        let policy =
            PresentationPolicy::RetainedFullWaveformOnly(RetainedFullWaveformOnlyPolicy::new(
                PresentationSpacing::AtLeast(ten),
                five,
                RetryBackoff::NextRenderOpportunity,
            ));
        let mut planner = RefreshPlanner::new(policy);
        let first = present(&mut planner, 1, PresentationUrgency::Immediate);
        planner
            .attempt_succeeded(first, MonotonicMillis::new(2))
            .unwrap();
        assert_eq!(
            planner.plan(MonotonicMillis::new(3), PresentationUrgency::Telemetry),
            Ok(RefreshDecision::DeferredUntil(MonotonicMillis::new(12)))
        );
        assert_eq!(
            planner.plan(MonotonicMillis::new(12), PresentationUrgency::Telemetry),
            Ok(RefreshDecision::Present(RefreshAttempt {
                planner_id: planner.planner_id,
                attempt_id: 2,
                planned_at: MonotonicMillis::new(12),
                kind: RefreshKind::RetainedFullWaveform,
            }))
        );
    }

    #[test]
    fn feedback_is_bound_to_one_planner_and_non_decreasing_time() {
        let policy = PresentationPolicy::ImmediateDisplay(ImmediateDisplayPolicy::new(
            RetryBackoff::NextRenderOpportunity,
        ));
        let mut first = RefreshPlanner::new(policy);
        let mut second = RefreshPlanner::new(policy);
        let attempt = present(&mut first, 10, PresentationUrgency::Immediate);
        assert_eq!(
            second.attempt_succeeded(attempt, MonotonicMillis::new(10)),
            Err(FeedbackError::ForeignPlanner)
        );
    }

    #[test]
    fn deadline_addition_saturates() {
        let policy = PresentationPolicy::ImmediateDisplay(ImmediateDisplayPolicy::new(
            RetryBackoff::AtLeast(NonZeroDuration::new(10).unwrap()),
        ));
        let mut planner = RefreshPlanner::new(policy);
        let attempt = present(&mut planner, u64::MAX - 5, PresentationUrgency::Immediate);
        planner
            .attempt_failed(attempt, MonotonicMillis::new(u64::MAX - 5))
            .unwrap();
        assert_eq!(
            planner.plan(
                MonotonicMillis::new(u64::MAX - 1),
                PresentationUrgency::Immediate,
            ),
            Ok(RefreshDecision::DeferredUntil(MonotonicMillis::MAX))
        );
    }
}
