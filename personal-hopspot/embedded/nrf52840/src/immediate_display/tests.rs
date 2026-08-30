use embassy_futures::block_on;
use personal_hopspot_core::display::{
    BlankingResult, BufferRetention, DisplayVisibility, PresentationOutcome,
};

use super::*;

struct FakeDisplay {
    presentation_outcome: PresentationOutcome,
    blanking_result: BlankingResult,
    buffer_retention: BufferRetention,
    present_calls: u8,
    blank_calls: u8,
    restore_calls: u8,
}

impl FakeDisplay {
    const fn succeeding() -> Self {
        Self {
            presentation_outcome: PresentationOutcome::Succeeded,
            blanking_result: BlankingResult::Succeeded,
            buffer_retention: BufferRetention::Preserved,
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

    async fn apply_blanking(&mut self, command: BlankingCommand) -> BlankingOutcome {
        match command {
            BlankingCommand::Blank => self.blank_calls += 1,
            BlankingCommand::Restore => self.restore_calls += 1,
        }
        BlankingOutcome {
            result: core::mem::replace(&mut self.blanking_result, BlankingResult::Succeeded),
            buffer_retention: self.buffer_retention,
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
        ble_group_editor: personal_hopspot_core::BleGroupEditor::Unavailable,
    })
}

#[test]
fn failed_immediate_presentation_retries_the_latest_frame() {
    let mut device = FakeDisplay::succeeding();
    device.presentation_outcome = PresentationOutcome::Failed;
    let mut runtime = BoardDisplay::initialized(device).into_runtime(MonotonicMillis::new(0));
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
fn successful_blank_and_wake_commit_visibility_after_hardware_completion() {
    let mut runtime =
        BoardDisplay::initialized(FakeDisplay::succeeding()).into_runtime(MonotonicMillis::new(0));

    runtime
        .schedule_blanking(MonotonicMillis::new(1), DisplayBlankReason::DisplayOnly)
        .unwrap();
    assert_eq!(
        block_on(runtime.poll_blanking(MonotonicMillis::new(1), || MonotonicMillis::new(2))),
        Ok(BlankingDecision::Settled)
    );
    assert_eq!(runtime.visibility(), DisplayVisibility::Blanked);
    assert_eq!(runtime.device().blank_calls, 1);

    assert_eq!(
        block_on(runtime.button_pressed(MonotonicMillis::new(3), || MonotonicMillis::new(4))),
        Ok(DisplayButtonOutcome::WakeAndConsume)
    );
    assert_eq!(runtime.visibility(), DisplayVisibility::Visible);
    assert_eq!(runtime.device().restore_calls, 1);
}

#[test]
fn failed_blank_and_wake_retry_without_claiming_the_target_visibility() {
    let mut device = FakeDisplay::succeeding();
    device.blanking_result = BlankingResult::Failed;
    let mut runtime = BoardDisplay::initialized(device).into_runtime(MonotonicMillis::new(0));

    runtime
        .schedule_blanking(MonotonicMillis::new(1), DisplayBlankReason::DisplayOnly)
        .unwrap();
    assert_eq!(
        block_on(runtime.poll_blanking(MonotonicMillis::new(1), || MonotonicMillis::new(2))),
        Ok(BlankingDecision::RetryAt(MonotonicMillis::new(502)))
    );
    assert_eq!(runtime.visibility(), DisplayVisibility::Visible);
    assert_eq!(
        block_on(runtime.poll_blanking(MonotonicMillis::new(502), || MonotonicMillis::new(503))),
        Ok(BlankingDecision::Settled)
    );
    assert_eq!(runtime.visibility(), DisplayVisibility::Blanked);

    runtime.device_mut().blanking_result = BlankingResult::Failed;
    assert_eq!(
        block_on(runtime.button_pressed(MonotonicMillis::new(504), || MonotonicMillis::new(505))),
        Ok(DisplayButtonOutcome::WakeAndConsume)
    );
    assert_eq!(runtime.visibility(), DisplayVisibility::Blanked);
    assert_eq!(
        block_on(
            runtime.poll_blanking(MonotonicMillis::new(1_005), || MonotonicMillis::new(1_006))
        ),
        Ok(BlankingDecision::Settled)
    );
    assert_eq!(runtime.visibility(), DisplayVisibility::Visible);
}

#[test]
fn initialization_failure_cannot_advertise_user_blanking() {
    let runtime = BoardDisplay::initialization_failed(FakeDisplay::succeeding())
        .into_runtime(MonotonicMillis::new(0));

    assert!(!runtime.user_blanking().is_available());
    assert_eq!(runtime.visibility(), DisplayVisibility::Unavailable);
}

#[test]
fn shipping_auto_off_blanks_after_exactly_sixty_seconds() {
    let mut runtime =
        BoardDisplay::initialized(FakeDisplay::succeeding()).into_runtime(MonotonicMillis::new(0));

    assert_eq!(
        block_on(runtime.poll_blanking(MonotonicMillis::new(59_999), || {
            MonotonicMillis::new(59_999)
        })),
        Ok(BlankingDecision::Settled)
    );
    assert_eq!(runtime.visibility(), DisplayVisibility::Visible);
    assert_eq!(
        block_on(runtime.poll_blanking(MonotonicMillis::new(60_000), || {
            MonotonicMillis::new(60_001)
        })),
        Ok(BlankingDecision::Settled)
    );
    assert_eq!(runtime.visibility(), DisplayVisibility::Blanked);
}
