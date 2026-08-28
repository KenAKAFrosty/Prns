use personal_hopspot_core::display::{
    BlankingCommand, BlankingDecision, BlankingError, BlankingOutcome, DisplayAutoOff,
    DisplayBlankReason, DisplayButtonOutcome, DisplayCoordinator, DisplayDuration,
    DisplayVisibility, MonotonicMillis, PresentationDecision, PresentationError,
    PresentationOutcome, PresentationPolicy, PresentationUrgency, UserBlankingPolicy,
    DEFAULT_AUTO_OFF,
};
use personal_hopspot_core::face_64x128::{Frame, RenderInput};
use personal_hopspot_core::UserBlanking;

const BLANKING_RETRY_BACKOFF: DisplayDuration = match DisplayDuration::from_millis(500) {
    Ok(duration) => duration,
    Err(_) => panic!("the display retry backoff is nonzero"),
};

pub(crate) enum BoardDisplay<D> {
    Initialized(D),
    InitializationFailed(D),
}

impl<D> BoardDisplay<D> {
    pub(crate) const fn initialized(device: D) -> Self {
        Self::Initialized(device)
    }

    pub(crate) const fn initialization_failed(device: D) -> Self {
        Self::InitializationFailed(device)
    }

    pub(crate) fn into_runtime(self, now: MonotonicMillis) -> ImmediateDisplayRuntime<D>
    where
        D: ImmediateDisplayDevice,
    {
        match self {
            Self::Initialized(device) => ImmediateDisplayRuntime::available(device, now),
            Self::InitializationFailed(device) => ImmediateDisplayRuntime::unavailable(device),
        }
    }
}

pub(crate) trait ImmediateDisplayDevice {
    fn present(&mut self, frame: &Frame) -> PresentationOutcome;
    async fn apply_blanking(&mut self, command: BlankingCommand) -> BlankingOutcome;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImmediatePresentation {
    Unavailable,
    Withheld,
    Presented,
    Failed,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ImmediatePresentationError {
    Coordinator(PresentationError),
    NonImmediateDecision,
}

impl From<PresentationError> for ImmediatePresentationError {
    fn from(error: PresentationError) -> Self {
        Self::Coordinator(error)
    }
}

pub(crate) struct ImmediateDisplayRuntime<D> {
    device: D,
    coordinator: DisplayCoordinator,
}

impl<D: ImmediateDisplayDevice> ImmediateDisplayRuntime<D> {
    fn available(device: D, now: MonotonicMillis) -> Self {
        Self {
            device,
            coordinator: DisplayCoordinator::available(
                PresentationPolicy::Immediate,
                UserBlankingPolicy::Available {
                    initially_visible_at: now,
                    auto_off_after: DEFAULT_AUTO_OFF,
                    retry_backoff: BLANKING_RETRY_BACKOFF,
                },
            ),
        }
    }

    const fn unavailable(device: D) -> Self {
        Self {
            device,
            coordinator: DisplayCoordinator::unavailable(),
        }
    }

    pub(crate) const fn user_blanking(&self) -> UserBlanking {
        self.coordinator.user_blanking()
    }

    pub(crate) const fn visibility(&self) -> DisplayVisibility {
        self.coordinator.visibility()
    }

    pub(crate) fn render_and_present(
        &mut self,
        input: RenderInput<'_, '_>,
        planned_at: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<ImmediatePresentation, ImmediatePresentationError> {
        match self.visibility() {
            DisplayVisibility::Unavailable => return Ok(ImmediatePresentation::Unavailable),
            DisplayVisibility::Blanked => return Ok(ImmediatePresentation::Withheld),
            DisplayVisibility::Visible => {}
        }
        self.coordinator.render(input)?;
        let decision = match self
            .coordinator
            .plan_presentation(planned_at, PresentationUrgency::Immediate)
        {
            Ok(decision) => decision,
            Err(PresentationError::DisplayNotVisible) => {
                return Ok(ImmediatePresentation::Withheld);
            }
            Err(error) => return Err(error.into()),
        };
        match decision {
            PresentationDecision::Present(attempt) => {
                let outcome = self
                    .device
                    .present(self.coordinator.candidate_frame(&attempt)?);
                let result = match outcome {
                    PresentationOutcome::Succeeded => ImmediatePresentation::Presented,
                    PresentationOutcome::Failed => ImmediatePresentation::Failed,
                };
                self.coordinator
                    .complete_presentation(attempt, completed_at(), outcome)?;
                Ok(result)
            }
            PresentationDecision::Unchanged | PresentationDecision::DeferredUntil(_) => {
                Err(ImmediatePresentationError::NonImmediateDecision)
            }
        }
    }

    pub(crate) async fn poll_blanking(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        if self.visibility() == DisplayVisibility::Unavailable {
            return Ok(BlankingDecision::Settled);
        }
        let decision = self.coordinator.tick_blanking(now)?;
        self.apply_blanking(decision, completed_at).await
    }

    pub(crate) fn schedule_blanking(
        &mut self,
        at: MonotonicMillis,
        reason: DisplayBlankReason,
    ) -> Result<(), BlankingError> {
        if self.visibility() == DisplayVisibility::Unavailable {
            return Ok(());
        }
        self.coordinator.schedule_blanking(at, reason)
    }

    pub(crate) async fn button_pressed(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<DisplayButtonOutcome, BlankingError> {
        let decision = self.coordinator.button_pressed(now)?;
        let outcome = decision.outcome();
        self.apply_blanking(decision.blanking(), completed_at)
            .await?;
        Ok(outcome)
    }

    pub(crate) async fn request_visible(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        if self.visibility() == DisplayVisibility::Unavailable {
            return Ok(BlankingDecision::Settled);
        }
        let decision = self.coordinator.request_visible(now)?;
        self.apply_blanking(decision, completed_at).await
    }

    pub(crate) fn toggle_auto_off(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<DisplayAutoOff, BlankingError> {
        self.coordinator.toggle_auto_off(now)
    }

    async fn apply_blanking(
        &mut self,
        decision: BlankingDecision,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        let BlankingDecision::Apply(command) = decision else {
            return Ok(decision);
        };
        let outcome = self.device.apply_blanking(command).await;
        self.coordinator.complete_blanking(completed_at(), outcome)
    }

    #[cfg(test)]
    const fn device(&self) -> &D {
        &self.device
    }

    #[cfg(test)]
    fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }
}

#[cfg(test)]
mod tests;
