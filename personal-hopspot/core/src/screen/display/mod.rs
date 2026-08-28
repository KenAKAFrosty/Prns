mod blanking;
mod presentation;
mod time;

pub use blanking::{
    BlankingCommand, BlankingDecision, BlankingError, BlankingOutcome, BlankingResult,
    BufferRetention, ButtonDecision, DisplayAutoOff, DisplayBlankReason, DisplayButtonOutcome,
    DisplayVisibility, UserBlankingPolicy,
};
pub use presentation::{
    EinkPolicy, EinkPolicyConfiguration, EinkPolicyError, EinkRefreshPolicy, PresentationAttempt,
    PresentationDecision, PresentationError, PresentationOutcome, PresentationPolicy,
    PresentationUrgency, RefreshKind,
};
pub use time::{
    DisplayDuration, MonotonicMillis, PartialRefreshLimit, ZeroDuration, ZeroPartialRefreshLimit,
};

use blanking::BlankingCapability;
use presentation::Presenter;

use super::face_64x128::{self, Frame, RenderInput};
use super::UserBlanking;

const DEFAULT_AUTO_OFF_MILLISECONDS: u64 = 60_000;
pub const DEFAULT_AUTO_OFF: DisplayDuration =
    match DisplayDuration::from_millis(DEFAULT_AUTO_OFF_MILLISECONDS) {
        Ok(duration) => duration,
        Err(_) => panic!("the default display auto-off duration is nonzero"),
    };

pub struct DisplayCoordinator {
    state: DisplayState,
}

#[allow(
    clippy::large_enum_variant,
    reason = "the no_std coordinator owns its fixed retained frame buffers without allocation"
)]
enum DisplayState {
    Unavailable,
    Available(AvailableDisplay),
}

struct AvailableDisplay {
    presenter: Presenter,
    blanking: BlankingCapability,
}

impl DisplayCoordinator {
    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            state: DisplayState::Unavailable,
        }
    }

    #[must_use]
    pub fn available(presentation: PresentationPolicy, user_blanking: UserBlankingPolicy) -> Self {
        Self {
            state: DisplayState::Available(AvailableDisplay {
                presenter: Presenter::new(presentation),
                blanking: BlankingCapability::from_policy(user_blanking),
            }),
        }
    }

    #[must_use]
    pub const fn user_blanking(&self) -> UserBlanking {
        match &self.state {
            DisplayState::Unavailable => UserBlanking::unavailable(),
            DisplayState::Available(available) => available.blanking.user_blanking(),
        }
    }

    #[must_use]
    pub const fn visibility(&self) -> DisplayVisibility {
        match &self.state {
            DisplayState::Unavailable => DisplayVisibility::Unavailable,
            DisplayState::Available(available) => available.blanking.visibility(),
        }
    }

    pub fn render(&mut self, input: RenderInput<'_, '_>) -> Result<(), PresentationError> {
        let available = self.available_mut()?;
        face_64x128::render(available.presenter.candidate_mut()?, input);
        Ok(())
    }

    pub fn plan_presentation(
        &mut self,
        now: MonotonicMillis,
        urgency: PresentationUrgency,
    ) -> Result<PresentationDecision, PresentationError> {
        let available = self.available_mut()?;
        if !available.blanking.presentation_is_allowed() {
            return Err(PresentationError::DisplayNotVisible);
        }
        available.presenter.plan(now, urgency)
    }

    pub fn complete_presentation(
        &mut self,
        attempt: PresentationAttempt,
        completed_at: MonotonicMillis,
        outcome: PresentationOutcome,
    ) -> Result<(), PresentationError> {
        self.available_mut()?
            .presenter
            .complete(attempt, completed_at, outcome)
    }

    pub fn candidate_frame(
        &self,
        attempt: &PresentationAttempt,
    ) -> Result<&Frame, PresentationError> {
        self.available_ref()?.presenter.pending_frame(attempt)
    }

    pub fn invalidate_presentation(&mut self) -> Result<(), PresentationError> {
        self.available_mut()?.presenter.invalidate();
        Ok(())
    }

    pub fn tick_blanking(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        self.ensure_blanking_operation_allowed()?;
        self.blanking_mut()?.tick(now)
    }

    pub fn request_visible(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        self.ensure_blanking_operation_allowed()?;
        self.blanking_mut()?.request_visible(now)
    }

    pub fn request_blanked(
        &mut self,
        reason: DisplayBlankReason,
        now: MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        self.ensure_blanking_operation_allowed()?;
        self.blanking_mut()?.request_blanked(reason, now)
    }

    pub fn schedule_blanking(
        &mut self,
        at: MonotonicMillis,
        reason: DisplayBlankReason,
    ) -> Result<(), BlankingError> {
        self.blanking_mut()?.schedule(at, reason)
    }

    pub fn button_pressed(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<ButtonDecision, BlankingError> {
        match &mut self.state {
            DisplayState::Unavailable => Ok(ButtonDecision::forwarded()),
            DisplayState::Available(available) => available.blanking.button_pressed(now),
        }
    }

    pub fn toggle_auto_off(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<DisplayAutoOff, BlankingError> {
        self.blanking_mut()?.toggle_auto_off(now)
    }

    pub fn complete_blanking(
        &mut self,
        completed_at: MonotonicMillis,
        outcome: BlankingOutcome,
    ) -> Result<BlankingDecision, BlankingError> {
        let available = self.available_for_blanking_mut()?;
        let decision = available
            .blanking
            .state_mut()?
            .complete(completed_at, outcome.result)?;
        if outcome.buffer_retention == BufferRetention::Lost {
            available.presenter.invalidate();
        }
        Ok(decision)
    }

    fn available_ref(&self) -> Result<&AvailableDisplay, PresentationError> {
        match &self.state {
            DisplayState::Unavailable => Err(PresentationError::DisplayUnavailable),
            DisplayState::Available(available) => Ok(available),
        }
    }

    fn available_mut(&mut self) -> Result<&mut AvailableDisplay, PresentationError> {
        match &mut self.state {
            DisplayState::Unavailable => Err(PresentationError::DisplayUnavailable),
            DisplayState::Available(available) => Ok(available),
        }
    }

    fn available_for_blanking_mut(&mut self) -> Result<&mut AvailableDisplay, BlankingError> {
        match &mut self.state {
            DisplayState::Unavailable => Err(BlankingError::DisplayUnavailable),
            DisplayState::Available(available) => Ok(available),
        }
    }

    fn blanking_mut(&mut self) -> Result<&mut blanking::BlankingState, BlankingError> {
        self.available_for_blanking_mut()?.blanking.state_mut()
    }

    fn ensure_blanking_operation_allowed(&self) -> Result<(), BlankingError> {
        match &self.state {
            DisplayState::Unavailable => Err(BlankingError::DisplayUnavailable),
            DisplayState::Available(AvailableDisplay {
                blanking: BlankingCapability::Unavailable,
                ..
            }) => Err(BlankingError::UserBlankingUnavailable),
            DisplayState::Available(available) if available.presenter.has_pending_attempt() => {
                Err(BlankingError::PresentationInFlight)
            }
            DisplayState::Available(_) => Ok(()),
        }
    }

    #[cfg(test)]
    fn candidate_mut(&mut self) -> Result<&mut Frame, PresentationError> {
        self.available_mut()?.presenter.candidate_mut()
    }
}

#[cfg(test)]
mod tests;
