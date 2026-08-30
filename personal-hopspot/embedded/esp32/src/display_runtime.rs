use core::fmt::Debug;

use personal_hopspot_core::display::{
    BlankingDecision, BlankingError, DisplayAutoOff, DisplayBlankReason, DisplayButtonOutcome,
    DisplayCoordinator, DisplayVisibility, EinkPolicy, MonotonicMillis, PresentationDecision,
    PresentationError, PresentationOutcome, PresentationPolicy, PresentationUrgency, RefreshKind,
    UserBlankingPolicy,
};
use personal_hopspot_core::face_64x128::{Frame, RenderInput};
use personal_hopspot_core::UserBlanking;

pub(crate) enum ImmediateBoardDisplay<D> {
    Initialized(D),
    InitializationFailed(D),
}

impl<D> ImmediateBoardDisplay<D> {
    pub(crate) const fn initialized(device: D) -> Self {
        Self::Initialized(device)
    }

    pub(crate) const fn initialization_failed(device: D) -> Self {
        Self::InitializationFailed(device)
    }
}

pub(crate) enum RetainedBoardDisplay<D> {
    Initialized { device: D, policy: EinkPolicy },
    InitializationFailed(D),
}

impl<D> RetainedBoardDisplay<D> {
    pub(crate) const fn initialized(device: D, policy: EinkPolicy) -> Self {
        Self::Initialized { device, policy }
    }

    pub(crate) const fn initialization_failed(device: D) -> Self {
        Self::InitializationFailed(device)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum S3Presentation {
    Unavailable,
    Withheld,
    Presented,
    Unchanged,
    DeferredUntil(MonotonicMillis),
    Failed,
}

pub(crate) trait S3BoardDisplay {
    type Runtime: S3DisplayRuntime;

    fn into_runtime(self, now: MonotonicMillis) -> Self::Runtime;
}

#[allow(async_fn_in_trait)]
pub(crate) trait RetainedDisplayDevice {
    async fn present(&mut self, frame: &Frame, refresh: RefreshKind) -> PresentationOutcome;
}

impl<D: RetainedDisplayDevice> S3BoardDisplay for RetainedBoardDisplay<D> {
    type Runtime = RetainedDisplayRuntime<D>;

    fn into_runtime(self, _now: MonotonicMillis) -> Self::Runtime {
        match self {
            Self::Initialized { device, policy } => {
                RetainedDisplayRuntime::available(device, policy)
            }
            Self::InitializationFailed(device) => RetainedDisplayRuntime::unavailable(device),
        }
    }
}

#[allow(async_fn_in_trait)]
pub(crate) trait S3DisplayRuntime {
    type PresentationError: Debug;

    fn user_blanking(&self) -> UserBlanking;
    fn visibility(&self) -> DisplayVisibility;

    async fn render_and_present(
        &mut self,
        input: RenderInput<'_, '_>,
        planned_at: MonotonicMillis,
        urgency: PresentationUrgency,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<S3Presentation, Self::PresentationError>;

    fn poll_blanking(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError>;

    fn schedule_blanking(
        &mut self,
        at: MonotonicMillis,
        reason: DisplayBlankReason,
    ) -> Result<(), BlankingError>;

    fn button_pressed(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<DisplayButtonOutcome, BlankingError>;

    fn request_visible(
        &mut self,
        now: MonotonicMillis,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError>;

    fn toggle_auto_off(&mut self, now: MonotonicMillis) -> Result<DisplayAutoOff, BlankingError>;
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum RetainedPresentationError {
    Coordinator(PresentationError),
    NonRetainedRefresh(RefreshKind),
}

impl From<PresentationError> for RetainedPresentationError {
    fn from(error: PresentationError) -> Self {
        Self::Coordinator(error)
    }
}

pub(crate) struct RetainedDisplayRuntime<D> {
    device: D,
    coordinator: DisplayCoordinator,
}

impl<D: RetainedDisplayDevice> RetainedDisplayRuntime<D> {
    fn available(device: D, policy: EinkPolicy) -> Self {
        Self {
            device,
            coordinator: DisplayCoordinator::available(
                PresentationPolicy::RetainedEink(policy),
                UserBlankingPolicy::Unavailable,
            ),
        }
    }

    const fn unavailable(device: D) -> Self {
        Self {
            device,
            coordinator: DisplayCoordinator::unavailable(),
        }
    }

    #[cfg(test)]
    const fn device(&self) -> &D {
        &self.device
    }
}

impl<D: RetainedDisplayDevice> S3DisplayRuntime for RetainedDisplayRuntime<D> {
    type PresentationError = RetainedPresentationError;

    fn user_blanking(&self) -> UserBlanking {
        self.coordinator.user_blanking()
    }

    fn visibility(&self) -> DisplayVisibility {
        self.coordinator.visibility()
    }

    async fn render_and_present(
        &mut self,
        input: RenderInput<'_, '_>,
        planned_at: MonotonicMillis,
        urgency: PresentationUrgency,
        completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<S3Presentation, Self::PresentationError> {
        match self.visibility() {
            DisplayVisibility::Unavailable => return Ok(S3Presentation::Unavailable),
            DisplayVisibility::Blanked => return Ok(S3Presentation::Withheld),
            DisplayVisibility::Visible => {}
        }
        self.coordinator.render(input)?;
        match self.coordinator.plan_presentation(planned_at, urgency)? {
            PresentationDecision::Unchanged => Ok(S3Presentation::Unchanged),
            PresentationDecision::DeferredUntil(deadline) => {
                Ok(S3Presentation::DeferredUntil(deadline))
            }
            PresentationDecision::Present(attempt) => {
                let refresh = attempt.refresh_kind();
                if refresh == RefreshKind::Immediate {
                    self.coordinator.complete_presentation(
                        attempt,
                        completed_at(),
                        PresentationOutcome::Failed,
                    )?;
                    return Err(RetainedPresentationError::NonRetainedRefresh(refresh));
                }
                let outcome = self
                    .device
                    .present(self.coordinator.candidate_frame(&attempt)?, refresh)
                    .await;
                let presentation = match outcome {
                    PresentationOutcome::Succeeded => S3Presentation::Presented,
                    PresentationOutcome::Failed => S3Presentation::Failed,
                };
                self.coordinator
                    .complete_presentation(attempt, completed_at(), outcome)?;
                Ok(presentation)
            }
        }
    }

    fn poll_blanking(
        &mut self,
        _now: MonotonicMillis,
        _completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        Ok(BlankingDecision::Settled)
    }

    fn schedule_blanking(
        &mut self,
        _at: MonotonicMillis,
        _reason: DisplayBlankReason,
    ) -> Result<(), BlankingError> {
        Ok(())
    }

    fn button_pressed(
        &mut self,
        _now: MonotonicMillis,
        _completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<DisplayButtonOutcome, BlankingError> {
        Ok(DisplayButtonOutcome::ForwardToUi)
    }

    fn request_visible(
        &mut self,
        _now: MonotonicMillis,
        _completed_at: impl FnOnce() -> MonotonicMillis,
    ) -> Result<BlankingDecision, BlankingError> {
        Ok(BlankingDecision::Settled)
    }

    fn toggle_auto_off(&mut self, _now: MonotonicMillis) -> Result<DisplayAutoOff, BlankingError> {
        Err(BlankingError::UserBlankingUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use embassy_futures::block_on;
    use personal_hopspot_core::display::{
        DisplayDuration, EinkPolicyConfiguration, EinkRefreshPolicy,
    };

    use super::*;

    struct FakeRetainedDisplay {
        fail_next: bool,
        present_calls: u8,
        last_refresh: Option<RefreshKind>,
    }

    impl FakeRetainedDisplay {
        const fn succeeding() -> Self {
            Self {
                fail_next: false,
                present_calls: 0,
                last_refresh: None,
            }
        }
    }

    impl RetainedDisplayDevice for FakeRetainedDisplay {
        async fn present(&mut self, _frame: &Frame, refresh: RefreshKind) -> PresentationOutcome {
            self.present_calls += 1;
            self.last_refresh = Some(refresh);
            if core::mem::take(&mut self.fail_next) {
                PresentationOutcome::Failed
            } else {
                PresentationOutcome::Succeeded
            }
        }
    }

    fn policy() -> EinkPolicy {
        EinkPolicy::new(EinkPolicyConfiguration {
            telemetry_minimum: DisplayDuration::from_millis(30_000).unwrap(),
            refresh: EinkRefreshPolicy::FullOnly,
        })
        .unwrap()
    }

    fn ui_state() -> personal_hopspot_core::UiState {
        personal_hopspot_core::UiState::new(personal_hopspot_core::UiConfiguration {
            storage_limits: personal_rns::storage::DisplayedStorageLimits::DYNAMIC,
            user_blanking: UserBlanking::unavailable(),
            access_point: personal_hopspot_core::AccessPointState::Unsupported,
            shared_instance_config_export:
                personal_hopspot_core::SharedInstanceConfigExport::Unavailable,
            gnss: personal_hopspot_core::GnssAvailability::Unavailable,
            ble_group_editor: personal_hopspot_core::BleGroupEditor::Unavailable,
        })
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

    #[test]
    fn retained_runtime_owns_exact_comparison_and_full_only_refreshes() {
        let mut runtime =
            RetainedBoardDisplay::initialized(FakeRetainedDisplay::succeeding(), policy())
                .into_runtime(MonotonicMillis::new(0));
        let mut state = ui_state();
        let details = personal_hopspot_core::InterfaceMenuDetails::empty();

        assert_eq!(
            block_on(runtime.render_and_present(
                render_input(&state, &details),
                MonotonicMillis::new(1),
                PresentationUrgency::Immediate,
                || MonotonicMillis::new(2),
            )),
            Ok(S3Presentation::Presented)
        );
        assert_eq!(runtime.device().last_refresh, Some(RefreshKind::Full));
        assert_eq!(
            block_on(runtime.render_and_present(
                render_input(&state, &details),
                MonotonicMillis::new(3),
                PresentationUrgency::Telemetry,
                || MonotonicMillis::new(4),
            )),
            Ok(S3Presentation::Unchanged)
        );
        assert_eq!(runtime.device().present_calls, 1);

        state.show_notice(personal_hopspot_core::UiNotice::Awake);
        assert_eq!(
            block_on(runtime.render_and_present(
                render_input(&state, &details),
                MonotonicMillis::new(3),
                PresentationUrgency::Telemetry,
                || MonotonicMillis::new(4),
            )),
            Ok(S3Presentation::DeferredUntil(MonotonicMillis::new(30_002)))
        );
        assert_eq!(runtime.device().present_calls, 1);
    }

    #[test]
    fn failed_retained_refresh_loses_knowledge_and_retries_full() {
        let mut runtime =
            RetainedBoardDisplay::initialized(FakeRetainedDisplay::succeeding(), policy())
                .into_runtime(MonotonicMillis::new(0));
        let mut state = ui_state();
        let details = personal_hopspot_core::InterfaceMenuDetails::empty();
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(1),
            PresentationUrgency::Immediate,
            || MonotonicMillis::new(2),
        ))
        .unwrap();

        state.show_notice(personal_hopspot_core::UiNotice::Awake);
        runtime.device.fail_next = true;
        assert_eq!(
            block_on(runtime.render_and_present(
                render_input(&state, &details),
                MonotonicMillis::new(3),
                PresentationUrgency::Immediate,
                || MonotonicMillis::new(4),
            )),
            Ok(S3Presentation::Failed)
        );
        assert_eq!(
            block_on(runtime.render_and_present(
                render_input(&state, &details),
                MonotonicMillis::new(5),
                PresentationUrgency::Telemetry,
                || MonotonicMillis::new(6),
            )),
            Ok(S3Presentation::Presented)
        );
        assert_eq!(runtime.device().last_refresh, Some(RefreshKind::Full));
    }

    #[test]
    fn retained_hardware_cannot_advertise_or_apply_user_blanking() {
        let mut runtime =
            RetainedBoardDisplay::initialized(FakeRetainedDisplay::succeeding(), policy())
                .into_runtime(MonotonicMillis::new(0));

        assert!(!runtime.user_blanking().is_available());
        assert_eq!(runtime.visibility(), DisplayVisibility::Visible);
        assert_eq!(
            runtime.button_pressed(MonotonicMillis::new(1), || MonotonicMillis::new(2)),
            Ok(DisplayButtonOutcome::ForwardToUi)
        );
        assert_eq!(
            runtime.schedule_blanking(MonotonicMillis::new(3), DisplayBlankReason::SystemSleep,),
            Ok(())
        );
        assert_eq!(
            runtime.poll_blanking(MonotonicMillis::new(4), || MonotonicMillis::new(5)),
            Ok(BlankingDecision::Settled)
        );
        assert_eq!(
            runtime.request_visible(MonotonicMillis::new(6), || MonotonicMillis::new(7)),
            Ok(BlankingDecision::Settled)
        );
        assert_eq!(
            runtime.toggle_auto_off(MonotonicMillis::new(8)),
            Err(BlankingError::UserBlankingUnavailable)
        );
        assert_eq!(runtime.visibility(), DisplayVisibility::Visible);
    }

    #[test]
    fn initialization_failure_closes_the_retained_display_surface() {
        let runtime =
            RetainedBoardDisplay::initialization_failed(FakeRetainedDisplay::succeeding())
                .into_runtime(MonotonicMillis::new(0));

        assert_eq!(runtime.visibility(), DisplayVisibility::Unavailable);
        assert!(!runtime.user_blanking().is_available());
    }
}
