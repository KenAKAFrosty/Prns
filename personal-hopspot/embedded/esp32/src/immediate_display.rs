use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, Pixel, Point};
use personal_hopspot_core::display::{
    BlankingCommand, BlankingDecision, BlankingError, BlankingOutcome, DisplayAutoOff,
    DisplayBlankReason, DisplayButtonOutcome, DisplayCoordinator, DisplayDuration,
    DisplayVisibility, MonotonicMillis, PresentationDecision, PresentationError,
    PresentationOutcome, PresentationPolicy, PresentationUrgency, UserBlankingPolicy,
    DEFAULT_AUTO_OFF,
};
use personal_hopspot_core::face_64x128::{Frame, RenderInput, HEIGHT, WIDTH};
use personal_hopspot_core::UserBlanking;

use crate::display_runtime::{
    ImmediateBoardDisplay, S3BoardDisplay, S3DisplayRuntime, S3Presentation,
};

const BLANKING_RETRY_BACKOFF_DURATION: DisplayDuration = match DisplayDuration::from_millis(500) {
    Ok(duration) => duration,
    Err(_) => panic!("the display retry backoff is nonzero"),
};

impl<D: ImmediateDisplayDevice> S3BoardDisplay for ImmediateBoardDisplay<D> {
    type Runtime = ImmediateDisplayRuntime<D>;

    fn into_runtime(self, now: MonotonicMillis) -> ImmediateDisplayRuntime<D> {
        match self {
            Self::Initialized(device) => ImmediateDisplayRuntime::available(device, now),
            Self::InitializationFailed(device) => ImmediateDisplayRuntime::unavailable(device),
        }
    }
}

impl<D: ImmediateDisplayDevice> S3DisplayRuntime for ImmediateDisplayRuntime<D> {
    type PresentationError = ImmediatePresentationError;

    fn user_blanking(&self) -> UserBlanking {
        ImmediateDisplayRuntime::user_blanking(self)
    }

    fn visibility(&self) -> DisplayVisibility {
        ImmediateDisplayRuntime::visibility(self)
    }

    async fn render_and_present(
        &mut self,
        input: RenderInput<'_, '_>,
        planned_at: MonotonicMillis,
        _urgency: PresentationUrgency,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<S3Presentation, Self::PresentationError> {
        ImmediateDisplayRuntime::render_and_present(self, input, planned_at, completed_at).map(
            |presentation| match presentation {
                ImmediatePresentation::Unavailable => S3Presentation::Unavailable,
                ImmediatePresentation::Withheld => S3Presentation::Withheld,
                ImmediatePresentation::Presented => S3Presentation::Presented,
                ImmediatePresentation::Failed => S3Presentation::Failed,
            },
        )
    }

    fn poll_blanking(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        ImmediateDisplayRuntime::poll_blanking(self, now, completed_at)
    }

    fn schedule_blanking(
        &mut self,
        at: MonotonicMillis,
        reason: DisplayBlankReason,
    ) -> Result<(), BlankingError> {
        ImmediateDisplayRuntime::schedule_blanking(self, at, reason)
    }

    fn button_pressed(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<DisplayButtonOutcome, BlankingError> {
        ImmediateDisplayRuntime::button_pressed(self, now, completed_at)
    }

    fn request_visible(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        ImmediateDisplayRuntime::request_visible(self, now, completed_at)
    }

    fn toggle_auto_off(&mut self, now: MonotonicMillis) -> Result<DisplayAutoOff, BlankingError> {
        ImmediateDisplayRuntime::toggle_auto_off(self, now)
    }
}

pub(crate) trait ImmediateDisplayDevice {
    fn present(&mut self, frame: &Frame) -> PresentationOutcome;
    fn apply_blanking(&mut self, command: BlankingCommand) -> BlankingOutcome;
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
                    retry_backoff: BLANKING_RETRY_BACKOFF_DURATION,
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

    pub(crate) fn poll_blanking(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        if self.visibility() == DisplayVisibility::Unavailable {
            return Ok(BlankingDecision::Settled);
        }
        let decision = self.coordinator.tick_blanking(now)?;
        self.apply_blanking(decision, completed_at)
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

    pub(crate) fn button_pressed(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<DisplayButtonOutcome, BlankingError> {
        let decision = self.coordinator.button_pressed(now)?;
        let outcome = decision.outcome();
        self.apply_blanking(decision.blanking(), completed_at)?;
        Ok(outcome)
    }

    pub(crate) fn request_visible(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        if self.visibility() == DisplayVisibility::Unavailable {
            return Ok(BlankingDecision::Settled);
        }
        let decision = self.coordinator.request_visible(now)?;
        self.apply_blanking(decision, completed_at)
    }

    pub(crate) fn toggle_auto_off(
        &mut self,
        now: MonotonicMillis,
    ) -> Result<DisplayAutoOff, BlankingError> {
        self.coordinator.toggle_auto_off(now)
    }

    fn apply_blanking(
        &mut self,
        decision: BlankingDecision,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        let BlankingDecision::Apply(command) = decision else {
            return Ok(decision);
        };
        let outcome = self.device.apply_blanking(command);
        self.coordinator.complete_blanking(completed_at(), outcome)
    }

    #[cfg(test)]
    const fn device(&self) -> &D {
        &self.device
    }
}

pub(crate) fn draw_canonical_frame<D>(display: &mut D, frame: &Frame) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    display.draw_iter((0..HEIGHT).flat_map(|y| {
        (0..WIDTH).map(move |x| {
            let point = Point::new(x as i32, y as i32);
            let color = if frame.pixel_is_on(point) {
                BinaryColor::On
            } else {
                BinaryColor::Off
            };
            Pixel(point, color)
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_hopspot_core::display::{
        BlankingResult, BufferRetention, DisplayVisibility, PresentationOutcome,
    };

    struct FakeDisplay {
        presentation_outcome: PresentationOutcome,
        blanking_result: BlankingResult,
        present_calls: u8,
        blank_calls: u8,
        restore_calls: u8,
    }

    impl FakeDisplay {
        const fn succeeding() -> Self {
            Self {
                presentation_outcome: PresentationOutcome::Succeeded,
                blanking_result: BlankingResult::Succeeded,
                present_calls: 0,
                blank_calls: 0,
                restore_calls: 0,
            }
        }
    }

    impl ImmediateDisplayDevice for FakeDisplay {
        fn present(&mut self, _frame: &Frame) -> PresentationOutcome {
            self.present_calls += 1;
            core::mem::replace(
                &mut self.presentation_outcome,
                PresentationOutcome::Succeeded,
            )
        }

        fn apply_blanking(&mut self, command: BlankingCommand) -> BlankingOutcome {
            match command {
                BlankingCommand::Blank => self.blank_calls += 1,
                BlankingCommand::Restore => self.restore_calls += 1,
            }
            BlankingOutcome {
                result: core::mem::replace(&mut self.blanking_result, BlankingResult::Succeeded),
                buffer_retention: BufferRetention::Preserved,
            }
        }
    }

    fn render_input<'a>(
        state: &'a personal_hopspot_core::UiState,
        details: &'a personal_hopspot_core::InterfaceMenuDetails,
    ) -> RenderInput<'a, 'static> {
        RenderInput {
            content: personal_hopspot_core::ScreenContent {
                cards: &[],
                local_docs: None,
            },
            battery: personal_hopspot_core::PowerSnapshot::UNKNOWN,
            gnss: None,
            state,
            interface_menu_details: details,
        }
    }

    fn ui_state(user_blanking: UserBlanking) -> personal_hopspot_core::UiState {
        personal_hopspot_core::UiState::new(personal_hopspot_core::UiConfiguration {
            storage_limits: personal_rns::storage::DisplayedStorageLimits::DYNAMIC,
            user_blanking,
            access_point: personal_hopspot_core::AccessPointState::Unsupported,
            shared_instance_config_export:
                personal_hopspot_core::SharedInstanceConfigExport::Unavailable,
            gnss: personal_hopspot_core::GnssAvailability::Unavailable,
        })
    }

    #[test]
    fn failed_immediate_presentation_retries_the_latest_canonical_frame() {
        let mut device = FakeDisplay::succeeding();
        device.presentation_outcome = PresentationOutcome::Failed;
        let mut runtime =
            ImmediateBoardDisplay::initialized(device).into_runtime(MonotonicMillis::new(0));
        let state = ui_state(runtime.user_blanking());
        let details = personal_hopspot_core::InterfaceMenuDetails::empty();

        assert_eq!(
            runtime.render_and_present(
                render_input(&state, &details),
                MonotonicMillis::new(1),
                || MonotonicMillis::new(2),
            ),
            Ok(ImmediatePresentation::Failed)
        );
        assert_eq!(
            runtime.render_and_present(
                render_input(&state, &details),
                MonotonicMillis::new(3),
                || MonotonicMillis::new(4),
            ),
            Ok(ImmediatePresentation::Presented)
        );
        assert_eq!(runtime.device().present_calls, 2);
    }

    #[test]
    fn canonical_draw_copies_on_and_off_pixels_into_the_controller_surface() {
        let mut source = Frame::new();
        source
            .draw_iter([
                Pixel(Point::new(3, 7), BinaryColor::On),
                Pixel(Point::new(63, 127), BinaryColor::On),
            ])
            .unwrap();
        let mut controller = Frame::new();

        draw_canonical_frame(&mut controller, &source).unwrap();

        assert!(controller == source);
    }

    #[test]
    fn successful_blank_and_wake_change_visibility_after_hardware_completion() {
        let mut runtime = ImmediateBoardDisplay::initialized(FakeDisplay::succeeding())
            .into_runtime(MonotonicMillis::new(0));

        assert!(runtime.user_blanking().is_available());
        runtime
            .schedule_blanking(MonotonicMillis::new(1), DisplayBlankReason::DisplayOnly)
            .unwrap();
        assert_eq!(
            runtime.poll_blanking(MonotonicMillis::new(1), || MonotonicMillis::new(2)),
            Ok(BlankingDecision::Settled)
        );
        assert_eq!(runtime.visibility(), DisplayVisibility::Blanked);
        assert_eq!(runtime.device().blank_calls, 1);

        assert_eq!(
            runtime.button_pressed(MonotonicMillis::new(3), || MonotonicMillis::new(4)),
            Ok(DisplayButtonOutcome::WakeAndConsume)
        );
        assert_eq!(runtime.visibility(), DisplayVisibility::Visible);
        assert_eq!(runtime.device().restore_calls, 1);
    }

    #[test]
    fn shipping_auto_off_blanks_after_exactly_sixty_seconds() {
        let mut runtime = ImmediateBoardDisplay::initialized(FakeDisplay::succeeding())
            .into_runtime(MonotonicMillis::new(0));

        assert_eq!(
            runtime.poll_blanking(MonotonicMillis::new(59_999), || MonotonicMillis::new(
                59_999
            )),
            Ok(BlankingDecision::Settled)
        );
        assert_eq!(runtime.visibility(), DisplayVisibility::Visible);
        assert_eq!(
            runtime.poll_blanking(MonotonicMillis::new(60_000), || MonotonicMillis::new(
                60_001
            )),
            Ok(BlankingDecision::Settled)
        );
        assert_eq!(runtime.visibility(), DisplayVisibility::Blanked);
    }

    #[test]
    fn failed_blank_and_wake_retry_without_claiming_the_target_visibility() {
        let mut display = FakeDisplay::succeeding();
        display.blanking_result = BlankingResult::Failed;
        let mut runtime =
            ImmediateBoardDisplay::initialized(display).into_runtime(MonotonicMillis::new(0));

        runtime
            .schedule_blanking(MonotonicMillis::new(1), DisplayBlankReason::DisplayOnly)
            .unwrap();
        assert_eq!(
            runtime.poll_blanking(MonotonicMillis::new(1), || MonotonicMillis::new(2)),
            Ok(BlankingDecision::RetryAt(MonotonicMillis::new(502)))
        );
        assert_eq!(runtime.visibility(), DisplayVisibility::Visible);
        assert_eq!(
            runtime.poll_blanking(MonotonicMillis::new(502), || MonotonicMillis::new(503)),
            Ok(BlankingDecision::Settled)
        );
        assert_eq!(runtime.visibility(), DisplayVisibility::Blanked);

        runtime.device.blanking_result = BlankingResult::Failed;
        assert_eq!(
            runtime.button_pressed(MonotonicMillis::new(504), || MonotonicMillis::new(505)),
            Ok(DisplayButtonOutcome::WakeAndConsume)
        );
        assert_eq!(runtime.visibility(), DisplayVisibility::Blanked);
        assert_eq!(
            runtime.poll_blanking(MonotonicMillis::new(1_005), || MonotonicMillis::new(1_006)),
            Ok(BlankingDecision::Settled)
        );
        assert_eq!(runtime.visibility(), DisplayVisibility::Visible);
    }

    #[test]
    fn unavailable_hardware_cannot_advertise_user_blanking() {
        let runtime = ImmediateBoardDisplay::initialization_failed(FakeDisplay::succeeding())
            .into_runtime(MonotonicMillis::new(0));

        assert!(!runtime.user_blanking().is_available());
        assert_eq!(runtime.visibility(), DisplayVisibility::Unavailable);
    }
}
