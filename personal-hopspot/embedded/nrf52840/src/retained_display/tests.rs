use embassy_futures::block_on;
use personal_hopspot_core::display::{
    DisplayDuration, EinkPolicyConfiguration, EinkRefreshPolicy, PartialRefreshLimit,
};
use personal_hopspot_core::{
    AccessPointState, BleGroupEditor, GnssAvailability, InterfaceMenuDetails, ScreenContent,
    SharedInstanceConfigExport, UiConfiguration, UiNotice, UiState, UserBlanking,
};

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FakeError;

struct FakeDisplay {
    fail_next_presentation: bool,
    fail_next_recovery: bool,
    fail_next_sleep: bool,
    present_calls: u8,
    recover_calls: u8,
    sleep_calls: u8,
    last_refresh: Option<RetainedRefresh>,
}

impl FakeDisplay {
    const fn new() -> Self {
        Self {
            fail_next_presentation: false,
            fail_next_recovery: false,
            fail_next_sleep: false,
            present_calls: 0,
            recover_calls: 0,
            sleep_calls: 0,
            last_refresh: None,
        }
    }
}

impl RetainedDisplayDevice for FakeDisplay {
    type Error = FakeError;

    async fn present(
        &mut self,
        _frame: &Frame,
        refresh: RetainedRefresh,
    ) -> Result<(), Self::Error> {
        self.present_calls += 1;
        self.last_refresh = Some(refresh);
        if core::mem::take(&mut self.fail_next_presentation) {
            Err(FakeError)
        } else {
            Ok(())
        }
    }

    async fn recover(&mut self) -> Result<(), Self::Error> {
        self.recover_calls += 1;
        if core::mem::take(&mut self.fail_next_recovery) {
            Err(FakeError)
        } else {
            Ok(())
        }
    }

    async fn deep_sleep(&mut self) -> Result<(), Self::Error> {
        self.sleep_calls += 1;
        if core::mem::take(&mut self.fail_next_sleep) {
            Err(FakeError)
        } else {
            Ok(())
        }
    }
}

fn policy() -> EinkPolicy {
    EinkPolicy::new(EinkPolicyConfiguration {
        telemetry_minimum: DisplayDuration::from_millis(1_000).unwrap(),
        refresh: EinkRefreshPolicy::Partial {
            maximum_consecutive: PartialRefreshLimit::new(2).unwrap(),
            full_maximum_age: DisplayDuration::from_millis(10_000).unwrap(),
        },
    })
    .unwrap()
}

fn ui_state() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: personal_rns::storage::DisplayedStorageLimits::DYNAMIC,
        user_blanking: UserBlanking::unavailable(),
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: SharedInstanceConfigExport::Unavailable,
        gnss: GnssAvailability::Unavailable,
        ble_group_editor: BleGroupEditor::Unavailable,
    })
}

fn render_input<'a>(state: &'a UiState, details: &'a InterfaceMenuDetails) -> RenderInput<'a, 'a> {
    RenderInput {
        content: ScreenContent {
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
fn retained_frames_are_exact_and_telemetry_is_deferred() {
    let mut runtime = BoardDisplay::initialized(FakeDisplay::new()).into_runtime(policy());
    let mut state = ui_state();
    let details = InterfaceMenuDetails::empty();

    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(0),
            PresentationUrgency::Immediate,
            || MonotonicMillis::new(1),
        )),
        Ok(RetainedPresentation::Presented)
    );
    assert_eq!(runtime.device().last_refresh, Some(RetainedRefresh::Full));
    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(2),
            PresentationUrgency::Telemetry,
            || MonotonicMillis::new(3),
        )),
        Ok(RetainedPresentation::Unchanged)
    );

    state.show_notice(UiNotice::Announcing);
    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(500),
            PresentationUrgency::Telemetry,
            || MonotonicMillis::new(501),
        )),
        Ok(RetainedPresentation::DeferredUntil(MonotonicMillis::new(
            1_001
        )))
    );
    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(1_001),
            PresentationUrgency::Telemetry,
            || MonotonicMillis::new(1_002),
        )),
        Ok(RetainedPresentation::Presented)
    );
    assert_eq!(
        runtime.device().last_refresh,
        Some(RetainedRefresh::Partial)
    );
}

#[test]
fn failed_refresh_recovers_the_controller_and_forces_a_full_waveform() {
    let mut device = FakeDisplay::new();
    device.fail_next_presentation = true;
    let mut runtime = BoardDisplay::initialized(device).into_runtime(policy());
    let state = ui_state();
    let details = InterfaceMenuDetails::empty();

    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(0),
            PresentationUrgency::Immediate,
            || MonotonicMillis::new(1),
        )),
        Err(RetainedDisplayError::Device(FakeError))
    );
    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(2),
            PresentationUrgency::Telemetry,
            || MonotonicMillis::new(3),
        )),
        Ok(RetainedPresentation::Presented)
    );
    assert_eq!(runtime.device().recover_calls, 1);
    assert_eq!(runtime.device().last_refresh, Some(RetainedRefresh::Full));
}

#[test]
fn initialization_failure_remains_recoverable() {
    let mut runtime =
        BoardDisplay::initialization_failed(FakeDisplay::new()).into_runtime(policy());
    let state = ui_state();
    let details = InterfaceMenuDetails::empty();

    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(0),
            PresentationUrgency::Immediate,
            || MonotonicMillis::new(1),
        )),
        Ok(RetainedPresentation::Presented)
    );
    assert_eq!(runtime.device().recover_calls, 1);
    assert_eq!(runtime.device().last_refresh, Some(RetainedRefresh::Full));
}

#[test]
fn failed_recovery_retries_without_presenting_from_unknown_controller_state() {
    let mut device = FakeDisplay::new();
    device.fail_next_recovery = true;
    let mut runtime = BoardDisplay::initialization_failed(device).into_runtime(policy());
    let state = ui_state();
    let details = InterfaceMenuDetails::empty();

    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(0),
            PresentationUrgency::Immediate,
            || MonotonicMillis::new(1),
        )),
        Err(RetainedDisplayError::Device(FakeError))
    );
    assert_eq!(runtime.device().present_calls, 0);
    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(2),
            PresentationUrgency::Immediate,
            || MonotonicMillis::new(3),
        )),
        Ok(RetainedPresentation::Presented)
    );
    assert_eq!(runtime.device().recover_calls, 2);
    assert_eq!(runtime.device().last_refresh, Some(RetainedRefresh::Full));
}

#[test]
fn failed_deep_sleep_recovers_and_forces_a_full_waveform() {
    let mut runtime = BoardDisplay::initialized(FakeDisplay::new()).into_runtime(policy());
    let state = ui_state();
    let details = InterfaceMenuDetails::empty();

    block_on(runtime.render_and_present(
        render_input(&state, &details),
        MonotonicMillis::new(0),
        PresentationUrgency::Immediate,
        || MonotonicMillis::new(1),
    ))
    .unwrap();
    runtime.device.fail_next_sleep = true;
    assert_eq!(
        block_on(runtime.deep_sleep()),
        Err(RetainedDisplayError::Device(FakeError))
    );
    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(2),
            PresentationUrgency::Telemetry,
            || MonotonicMillis::new(3),
        )),
        Ok(RetainedPresentation::Presented)
    );
    assert_eq!(runtime.device().recover_calls, 1);
    assert_eq!(runtime.device().last_refresh, Some(RetainedRefresh::Full));
}

#[test]
fn deep_sleep_withholds_updates_until_recovery_and_then_requires_full() {
    let mut runtime = BoardDisplay::initialized(FakeDisplay::new()).into_runtime(policy());
    let mut state = ui_state();
    let details = InterfaceMenuDetails::empty();

    block_on(runtime.render_and_present(
        render_input(&state, &details),
        MonotonicMillis::new(0),
        PresentationUrgency::Immediate,
        || MonotonicMillis::new(1),
    ))
    .unwrap();
    block_on(runtime.deep_sleep()).unwrap();
    state.show_notice(UiNotice::Awake);
    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(2),
            PresentationUrgency::Immediate,
            || MonotonicMillis::new(3),
        )),
        Ok(RetainedPresentation::Sleeping)
    );
    block_on(runtime.wake()).unwrap();
    assert_eq!(
        block_on(runtime.render_and_present(
            render_input(&state, &details),
            MonotonicMillis::new(4),
            PresentationUrgency::Immediate,
            || MonotonicMillis::new(5),
        )),
        Ok(RetainedPresentation::Presented)
    );
    assert_eq!(runtime.device().sleep_calls, 1);
    assert_eq!(runtime.device().recover_calls, 1);
    assert_eq!(runtime.device().last_refresh, Some(RetainedRefresh::Full));
}
