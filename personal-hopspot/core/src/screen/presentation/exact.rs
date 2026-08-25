//! Exact value tracking and selective two-buffer ownership around physical presentation attempts.

use super::planner::{
    FeedbackError, MonotonicMillis, PlannerError, PresentationPolicy, PresentationUrgency,
    RefreshAttempt, RefreshDecision, RefreshKind, RefreshPlanner,
};

#[derive(Debug, Eq, PartialEq)]
pub enum ExactFrameState<F> {
    Unknown(UnknownReason),
    Known(F),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnknownReason {
    FirstPresentation,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameChange {
    Unchanged,
    Changed,
}

pub struct ExactFrameTracker<F> {
    state: ExactFrameState<F>,
}

impl<F: Eq> ExactFrameTracker<F> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: ExactFrameState::Unknown(UnknownReason::FirstPresentation),
        }
    }

    #[must_use]
    pub const fn state(&self) -> &ExactFrameState<F> {
        &self.state
    }

    #[must_use]
    pub fn compare(&self, candidate: &F) -> FrameChange {
        match &self.state {
            ExactFrameState::Known(known) if known == candidate => FrameChange::Unchanged,
            ExactFrameState::Unknown(_) | ExactFrameState::Known(_) => FrameChange::Changed,
        }
    }

    pub fn commit(&mut self, frame: F) -> Option<F> {
        match core::mem::replace(&mut self.state, ExactFrameState::Known(frame)) {
            ExactFrameState::Known(previous) => Some(previous),
            ExactFrameState::Unknown(_) => None,
        }
    }

    pub fn invalidate(&mut self) -> Option<F> {
        match core::mem::replace(
            &mut self.state,
            ExactFrameState::Unknown(UnknownReason::RecoveryRequired),
        ) {
            ExactFrameState::Known(previous) => Some(previous),
            ExactFrameState::Unknown(_) => None,
        }
    }

    pub(super) fn take_known(&mut self) -> Option<F> {
        match core::mem::replace(
            &mut self.state,
            ExactFrameState::Unknown(UnknownReason::RecoveryRequired),
        ) {
            ExactFrameState::Known(previous) => Some(previous),
            ExactFrameState::Unknown(reason) => {
                self.state = ExactFrameState::Unknown(reason);
                None
            }
        }
    }
}

impl<F: Eq> Default for ExactFrameTracker<F> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentationBusy;

#[derive(Debug, Eq, PartialEq)]
pub enum ExactPresentationDecision<F> {
    Unchanged,
    DeferredUntil(MonotonicMillis),
    Present(PresentationAttempt<F>),
}

#[derive(Debug, Eq, PartialEq)]
pub struct PresentationAttempt<F> {
    refresh: RefreshAttempt,
    candidate: F,
}

impl<F> PresentationAttempt<F> {
    #[must_use]
    pub const fn candidate(&self) -> &F {
        &self.candidate
    }

    #[must_use]
    pub const fn kind(&self) -> RefreshKind {
        self.refresh.kind()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExactPresentationError {
    Busy,
    InvariantViolation,
    Planner(PlannerError),
    Feedback(FeedbackError),
}

pub struct ExactPresentationState<F> {
    tracker: ExactFrameTracker<F>,
    planner: RefreshPlanner,
    working: Option<F>,
    spare: Option<F>,
    in_flight: bool,
}

impl<F: Eq> ExactPresentationState<F> {
    /// Construct a serialized owner with exactly one working and one spare value.
    #[must_use]
    pub fn new(working: F, spare: F, policy: PresentationPolicy) -> Self {
        Self {
            tracker: ExactFrameTracker::new(),
            planner: RefreshPlanner::new(policy),
            working: Some(working),
            spare: Some(spare),
            in_flight: false,
        }
    }

    pub fn working_mut(&mut self) -> Result<&mut F, PresentationBusy> {
        if self.in_flight {
            return Err(PresentationBusy);
        }
        self.working.as_mut().ok_or(PresentationBusy)
    }

    pub fn plan(
        &mut self,
        now: MonotonicMillis,
        urgency: PresentationUrgency,
    ) -> Result<ExactPresentationDecision<F>, ExactPresentationError> {
        if self.in_flight {
            return Err(ExactPresentationError::Busy);
        }
        let candidate = self
            .working
            .as_ref()
            .ok_or(ExactPresentationError::InvariantViolation)?;
        if self.tracker.compare(candidate) == FrameChange::Unchanged {
            return Ok(ExactPresentationDecision::Unchanged);
        }
        match self
            .planner
            .plan(now, urgency)
            .map_err(ExactPresentationError::Planner)?
        {
            RefreshDecision::DeferredUntil(deadline) => {
                Ok(ExactPresentationDecision::DeferredUntil(deadline))
            }
            RefreshDecision::Present(refresh) => {
                let candidate = self
                    .working
                    .take()
                    .ok_or(ExactPresentationError::InvariantViolation)?;
                let fallback = self
                    .tracker
                    .take_known()
                    .or_else(|| self.spare.take())
                    .ok_or(ExactPresentationError::InvariantViolation)?;
                self.spare = Some(fallback);
                self.in_flight = true;
                Ok(ExactPresentationDecision::Present(PresentationAttempt {
                    refresh,
                    candidate,
                }))
            }
        }
    }

    pub fn attempt_succeeded(
        &mut self,
        attempt: PresentationAttempt<F>,
        completed_at: MonotonicMillis,
    ) -> Result<(), ExactPresentationError> {
        if !self.in_flight {
            return Err(ExactPresentationError::Feedback(
                FeedbackError::MissingAttempt,
            ));
        }
        self.planner
            .validate_feedback(&attempt.refresh, completed_at)
            .map_err(ExactPresentationError::Feedback)?;
        let PresentationAttempt { refresh, candidate } = attempt;
        self.planner
            .attempt_succeeded(refresh, completed_at)
            .map_err(ExactPresentationError::Feedback)?;
        self.working = Some(
            self.spare
                .take()
                .ok_or(ExactPresentationError::InvariantViolation)?,
        );
        if self.tracker.commit(candidate).is_some() {
            return Err(ExactPresentationError::InvariantViolation);
        }
        self.in_flight = false;
        Ok(())
    }

    pub fn attempt_failed(
        &mut self,
        attempt: PresentationAttempt<F>,
        completed_at: MonotonicMillis,
    ) -> Result<(), ExactPresentationError> {
        if !self.in_flight {
            return Err(ExactPresentationError::Feedback(
                FeedbackError::MissingAttempt,
            ));
        }
        self.planner
            .validate_feedback(&attempt.refresh, completed_at)
            .map_err(ExactPresentationError::Feedback)?;
        let PresentationAttempt { refresh, candidate } = attempt;
        self.planner
            .attempt_failed(refresh, completed_at)
            .map_err(ExactPresentationError::Feedback)?;
        self.working = Some(candidate);
        let _ = self.tracker.invalidate();
        self.in_flight = false;
        Ok(())
    }

    pub fn invalidate(&mut self) -> Result<(), ExactPresentationError> {
        if self.in_flight {
            return Err(ExactPresentationError::Busy);
        }
        self.require_refresh_recovery()?;
        if let Some(known) = self.tracker.invalidate() {
            if self.spare.replace(known).is_some() {
                return Err(ExactPresentationError::InvariantViolation);
            }
        }
        Ok(())
    }

    /// Require retained-controller recovery without discarding exact knowledge of visible ink.
    pub fn require_refresh_recovery(&mut self) -> Result<(), ExactPresentationError> {
        if self.in_flight {
            return Err(ExactPresentationError::Busy);
        }
        self.planner
            .invalidate()
            .map_err(ExactPresentationError::Planner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::presentation::{
        ImmediateDisplayPolicy, NonZeroDuration, PartialRefreshLimit, PresentationSpacing,
        RetainedPartialWaveformPolicy, RetryBackoff,
    };

    fn state() -> ExactPresentationState<[u8; 4]> {
        ExactPresentationState::new(
            [0; 4],
            [0; 4],
            PresentationPolicy::ImmediateDisplay(ImmediateDisplayPolicy::new(
                RetryBackoff::NextRenderOpportunity,
            )),
        )
    }

    #[test]
    fn exact_tracker_compares_complete_values_and_invalidates() {
        let mut tracker = ExactFrameTracker::new();
        assert_eq!(tracker.compare(&[1, 2]), FrameChange::Changed);
        assert_eq!(tracker.commit([1, 2]), None);
        assert_eq!(tracker.compare(&[1, 2]), FrameChange::Unchanged);
        assert_eq!(tracker.compare(&[1, 3]), FrameChange::Changed);
        assert_eq!(tracker.invalidate(), Some([1, 2]));
        assert_eq!(tracker.compare(&[1, 2]), FrameChange::Changed);
    }

    #[test]
    fn success_freezes_candidate_and_reuses_exactly_two_frames() {
        let mut state = state();
        *state.working_mut().unwrap() = [1; 4];
        let ExactPresentationDecision::Present(attempt) = state
            .plan(MonotonicMillis::new(0), PresentationUrgency::Immediate)
            .unwrap()
        else {
            panic!("first frame must present");
        };
        assert_eq!(attempt.candidate(), &[1; 4]);
        assert_eq!(state.working_mut(), Err(PresentationBusy));
        state
            .attempt_succeeded(attempt, MonotonicMillis::new(1))
            .unwrap();
        *state.working_mut().unwrap() = [1; 4];
        assert_eq!(
            state
                .plan(MonotonicMillis::new(2), PresentationUrgency::Telemetry,)
                .unwrap(),
            ExactPresentationDecision::Unchanged
        );
    }

    #[test]
    fn failure_restores_candidate_as_working_and_requires_an_attempt() {
        let mut state = state();
        *state.working_mut().unwrap() = [2; 4];
        let ExactPresentationDecision::Present(attempt) = state
            .plan(MonotonicMillis::new(0), PresentationUrgency::Immediate)
            .unwrap()
        else {
            panic!("first frame must present");
        };
        state
            .attempt_failed(attempt, MonotonicMillis::new(1))
            .unwrap();
        assert_eq!(state.working_mut().unwrap(), &[2; 4]);
        assert!(matches!(
            state
                .plan(MonotonicMillis::new(2), PresentationUrgency::Immediate,)
                .unwrap(),
            ExactPresentationDecision::Present(_)
        ));
    }

    #[test]
    fn controller_recovery_preserves_exact_ink_but_forces_the_next_changed_full_refresh() {
        let policy = PresentationPolicy::RetainedPartialWaveform(
            RetainedPartialWaveformPolicy::new(
                PresentationSpacing::OperationCompletionOnly,
                NonZeroDuration::new(1).unwrap(),
                PartialRefreshLimit::new(4).unwrap(),
                NonZeroDuration::new(100).unwrap(),
                RetryBackoff::NextRenderOpportunity,
            )
            .unwrap(),
        );
        let mut state = ExactPresentationState::new([0; 4], [0; 4], policy);
        *state.working_mut().unwrap() = [1; 4];
        let ExactPresentationDecision::Present(first) = state
            .plan(MonotonicMillis::new(0), PresentationUrgency::Immediate)
            .unwrap()
        else {
            panic!("first retained frame must present");
        };
        assert_eq!(first.kind(), RefreshKind::RetainedFullWaveform);
        state
            .attempt_succeeded(first, MonotonicMillis::new(1))
            .unwrap();

        state.require_refresh_recovery().unwrap();
        *state.working_mut().unwrap() = [1; 4];
        assert_eq!(
            state
                .plan(MonotonicMillis::new(2), PresentationUrgency::Immediate)
                .unwrap(),
            ExactPresentationDecision::Unchanged
        );
        *state.working_mut().unwrap() = [2; 4];
        let ExactPresentationDecision::Present(recovery) = state
            .plan(MonotonicMillis::new(3), PresentationUrgency::Immediate)
            .unwrap()
        else {
            panic!("changed retained frame must recover");
        };
        assert_eq!(recovery.kind(), RefreshKind::RetainedFullWaveform);
    }
}
