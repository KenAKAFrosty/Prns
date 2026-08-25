use personal_hopspot_core as screen;

/// A missing blanking state means the available display does not support user
/// blanking, not that the display is absent or dark.
pub(crate) const fn display_is_visible(
    available: bool,
    blanking_visibility: Option<screen::DisplayVisibility>,
) -> bool {
    available
        && match blanking_visibility {
            Some(screen::DisplayVisibility::Blanked) => false,
            Some(screen::DisplayVisibility::Visible) | None => true,
        }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum S3PresentationStateError {
    Busy,
    Planner(screen::presentation::PlannerError),
    Feedback(screen::presentation::FeedbackError),
    Exact(screen::presentation::ExactPresentationError),
}

pub(crate) enum S3PresentationDecision<A> {
    Unchanged,
    DeferredUntil(screen::presentation::MonotonicMillis),
    Present(A),
}

/// Platform-local ownership seam used by the shared S3 face loop.
///
/// Immediate displays keep one working frame and always ask the planner to
/// present it. Retained displays use the core exact state directly, preserving
/// its two-frame transaction without requiring another S3 runtime.
pub(crate) trait S3PresentationState {
    type Attempt;

    fn working_mut(&mut self) -> Result<&mut screen::face_64x128::Frame, S3PresentationStateError>;
    fn plan(
        &mut self,
        now: screen::presentation::MonotonicMillis,
        urgency: screen::presentation::PresentationUrgency,
    ) -> Result<S3PresentationDecision<Self::Attempt>, S3PresentationStateError>;
    fn candidate<'a>(&'a self, attempt: &'a Self::Attempt) -> &'a screen::face_64x128::Frame;
    fn kind(&self, attempt: &Self::Attempt) -> screen::presentation::RefreshKind;
    fn attempt_succeeded(
        &mut self,
        attempt: Self::Attempt,
        completed_at: screen::presentation::MonotonicMillis,
    ) -> Result<(), S3PresentationStateError>;
    fn attempt_failed(
        &mut self,
        attempt: Self::Attempt,
        completed_at: screen::presentation::MonotonicMillis,
    ) -> Result<(), S3PresentationStateError>;
}

pub(crate) struct ImmediatePresentationState {
    working: screen::face_64x128::Frame,
    planner: screen::presentation::RefreshPlanner,
}

impl ImmediatePresentationState {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            working: screen::face_64x128::Frame::new(),
            planner: screen::presentation::RefreshPlanner::new(
                screen::presentation::PresentationPolicy::ImmediateDisplay(
                    screen::presentation::ImmediateDisplayPolicy::new(
                        screen::presentation::RetryBackoff::NextRenderOpportunity,
                    ),
                ),
            ),
        }
    }
}

impl S3PresentationState for ImmediatePresentationState {
    type Attempt = screen::presentation::RefreshAttempt;

    fn working_mut(&mut self) -> Result<&mut screen::face_64x128::Frame, S3PresentationStateError> {
        Ok(&mut self.working)
    }

    fn plan(
        &mut self,
        now: screen::presentation::MonotonicMillis,
        urgency: screen::presentation::PresentationUrgency,
    ) -> Result<S3PresentationDecision<Self::Attempt>, S3PresentationStateError> {
        match self
            .planner
            .plan(now, urgency)
            .map_err(S3PresentationStateError::Planner)?
        {
            screen::presentation::RefreshDecision::Present(attempt) => {
                Ok(S3PresentationDecision::Present(attempt))
            }
            screen::presentation::RefreshDecision::DeferredUntil(deadline) => {
                Ok(S3PresentationDecision::DeferredUntil(deadline))
            }
        }
    }

    fn candidate<'a>(&'a self, _attempt: &'a Self::Attempt) -> &'a screen::face_64x128::Frame {
        &self.working
    }

    fn kind(&self, attempt: &Self::Attempt) -> screen::presentation::RefreshKind {
        attempt.kind()
    }

    fn attempt_succeeded(
        &mut self,
        attempt: Self::Attempt,
        completed_at: screen::presentation::MonotonicMillis,
    ) -> Result<(), S3PresentationStateError> {
        self.planner
            .attempt_succeeded(attempt, completed_at)
            .map_err(S3PresentationStateError::Feedback)
    }

    fn attempt_failed(
        &mut self,
        attempt: Self::Attempt,
        completed_at: screen::presentation::MonotonicMillis,
    ) -> Result<(), S3PresentationStateError> {
        self.planner
            .attempt_failed(attempt, completed_at)
            .map_err(S3PresentationStateError::Feedback)
    }
}

impl S3PresentationState
    for screen::presentation::ExactPresentationState<screen::face_64x128::Frame>
{
    type Attempt = screen::presentation::PresentationAttempt<screen::face_64x128::Frame>;

    fn working_mut(&mut self) -> Result<&mut screen::face_64x128::Frame, S3PresentationStateError> {
        screen::presentation::ExactPresentationState::working_mut(self)
            .map_err(|_| S3PresentationStateError::Busy)
    }

    fn plan(
        &mut self,
        now: screen::presentation::MonotonicMillis,
        urgency: screen::presentation::PresentationUrgency,
    ) -> Result<S3PresentationDecision<Self::Attempt>, S3PresentationStateError> {
        match screen::presentation::ExactPresentationState::plan(self, now, urgency)
            .map_err(S3PresentationStateError::Exact)?
        {
            screen::presentation::ExactPresentationDecision::Unchanged => {
                Ok(S3PresentationDecision::Unchanged)
            }
            screen::presentation::ExactPresentationDecision::DeferredUntil(deadline) => {
                Ok(S3PresentationDecision::DeferredUntil(deadline))
            }
            screen::presentation::ExactPresentationDecision::Present(attempt) => {
                Ok(S3PresentationDecision::Present(attempt))
            }
        }
    }

    fn candidate<'a>(&'a self, attempt: &'a Self::Attempt) -> &'a screen::face_64x128::Frame {
        attempt.candidate()
    }

    fn kind(&self, attempt: &Self::Attempt) -> screen::presentation::RefreshKind {
        attempt.kind()
    }

    fn attempt_succeeded(
        &mut self,
        attempt: Self::Attempt,
        completed_at: screen::presentation::MonotonicMillis,
    ) -> Result<(), S3PresentationStateError> {
        screen::presentation::ExactPresentationState::attempt_succeeded(self, attempt, completed_at)
            .map_err(S3PresentationStateError::Exact)
    }

    fn attempt_failed(
        &mut self,
        attempt: Self::Attempt,
        completed_at: screen::presentation::MonotonicMillis,
    ) -> Result<(), S3PresentationStateError> {
        screen::presentation::ExactPresentationState::attempt_failed(self, attempt, completed_at)
            .map_err(S3PresentationStateError::Exact)
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::prelude::{DrawTarget, Pixel, Point};

    use super::*;

    #[test]
    fn available_display_without_user_blanking_remains_visible() {
        assert!(display_is_visible(true, None));
    }

    #[test]
    fn availability_and_confirmed_blanking_both_gate_rendering() {
        assert!(!display_is_visible(false, None));
        assert!(display_is_visible(
            true,
            Some(screen::DisplayVisibility::Visible)
        ));
        assert!(!display_is_visible(
            true,
            Some(screen::DisplayVisibility::Blanked)
        ));
    }

    #[test]
    fn immediate_owner_keeps_one_working_candidate_and_immediate_kind() {
        let mut state = ImmediatePresentationState::new();
        state
            .working_mut()
            .unwrap()
            .draw_iter([Pixel(
                Point::new(1, 2),
                embedded_graphics::pixelcolor::BinaryColor::On,
            )])
            .unwrap();

        let attempt = match state
            .plan(
                screen::presentation::MonotonicMillis::new(0),
                screen::presentation::PresentationUrgency::Immediate,
            )
            .unwrap()
        {
            S3PresentationDecision::Present(attempt) => attempt,
            S3PresentationDecision::Unchanged | S3PresentationDecision::DeferredUntil(_) => {
                panic!("an immediate owner presents every opportunity")
            }
        };
        assert_eq!(
            state.kind(&attempt),
            screen::presentation::RefreshKind::ImmediateDisplay
        );
        assert!(state.candidate(&attempt).pixel_is_on(Point::new(1, 2)));
        state
            .attempt_succeeded(attempt, screen::presentation::MonotonicMillis::new(1))
            .unwrap();
        let retry = match state
            .plan(
                screen::presentation::MonotonicMillis::new(2),
                screen::presentation::PresentationUrgency::Telemetry,
            )
            .unwrap()
        {
            S3PresentationDecision::Present(attempt) => attempt,
            S3PresentationDecision::Unchanged | S3PresentationDecision::DeferredUntil(_) => {
                panic!("an immediate owner presents every opportunity")
            }
        };
        state
            .attempt_failed(retry, screen::presentation::MonotonicMillis::new(3))
            .unwrap();
    }

    #[test]
    fn retained_owner_maps_exact_candidates_and_unchanged_decisions() {
        let policy = screen::presentation::PresentationPolicy::RetainedFullWaveformOnly(
            screen::presentation::RetainedFullWaveformOnlyPolicy::new(
                screen::presentation::PresentationSpacing::OperationCompletionOnly,
                screen::presentation::NonZeroDuration::new(30_000).unwrap(),
                screen::presentation::RetryBackoff::NextRenderOpportunity,
            ),
        );
        let mut state = screen::presentation::ExactPresentationState::new(
            screen::face_64x128::Frame::new(),
            screen::face_64x128::Frame::new(),
            policy,
        );
        S3PresentationState::working_mut(&mut state)
            .unwrap()
            .draw_iter([Pixel(
                Point::new(3, 4),
                embedded_graphics::pixelcolor::BinaryColor::On,
            )])
            .unwrap();

        let attempt = match S3PresentationState::plan(
            &mut state,
            screen::presentation::MonotonicMillis::new(0),
            screen::presentation::PresentationUrgency::Immediate,
        )
        .unwrap()
        {
            S3PresentationDecision::Present(attempt) => attempt,
            S3PresentationDecision::Unchanged | S3PresentationDecision::DeferredUntil(_) => {
                panic!("the first retained frame must be presented")
            }
        };
        assert_eq!(
            S3PresentationState::kind(&state, &attempt),
            screen::presentation::RefreshKind::RetainedFullWaveform
        );
        assert!(S3PresentationState::candidate(&state, &attempt).pixel_is_on(Point::new(3, 4)));
        S3PresentationState::attempt_succeeded(
            &mut state,
            attempt,
            screen::presentation::MonotonicMillis::new(1),
        )
        .unwrap();

        S3PresentationState::working_mut(&mut state)
            .unwrap()
            .draw_iter([Pixel(
                Point::new(3, 4),
                embedded_graphics::pixelcolor::BinaryColor::On,
            )])
            .unwrap();
        assert!(matches!(
            S3PresentationState::plan(
                &mut state,
                screen::presentation::MonotonicMillis::new(2),
                screen::presentation::PresentationUrgency::Telemetry,
            )
            .unwrap(),
            S3PresentationDecision::Unchanged
        ));
        S3PresentationState::working_mut(&mut state)
            .unwrap()
            .draw_iter([Pixel(
                Point::new(5, 6),
                embedded_graphics::pixelcolor::BinaryColor::On,
            )])
            .unwrap();
        let deadline = match S3PresentationState::plan(
            &mut state,
            screen::presentation::MonotonicMillis::new(2),
            screen::presentation::PresentationUrgency::Telemetry,
        )
        .unwrap()
        {
            S3PresentationDecision::DeferredUntil(deadline) => deadline,
            S3PresentationDecision::Unchanged | S3PresentationDecision::Present(_) => {
                panic!("changed telemetry must respect retained spacing")
            }
        };
        assert_eq!(deadline.get(), 30_001);
    }
}
