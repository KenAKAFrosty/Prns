use super::*;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, Pixel, Point};

const SECOND: DisplayDuration = match DisplayDuration::from_millis(1_000) {
    Ok(duration) => duration,
    Err(_) => panic!("one second is nonzero"),
};

fn retained_policy() -> PresentationPolicy {
    PresentationPolicy::RetainedEink(
        EinkPolicy::new(EinkPolicyConfiguration {
            telemetry_minimum: SECOND,
            refresh: EinkRefreshPolicy::Partial {
                maximum_consecutive: PartialRefreshLimit::new(2).unwrap(),
                full_maximum_age: DisplayDuration::from_millis(10_000).unwrap(),
            },
        })
        .unwrap(),
    )
}

fn coordinator(policy: PresentationPolicy) -> DisplayCoordinator {
    DisplayCoordinator::available(
        policy,
        UserBlankingPolicy::Available {
            initially_visible_at: MonotonicMillis::new(0),
            auto_off_after: SECOND,
            retry_backoff: SECOND,
        },
    )
}

fn draw_marker(coordinator: &mut DisplayCoordinator, x: i32) {
    coordinator
        .candidate_mut()
        .unwrap()
        .draw_iter([Pixel(Point::new(x, 0), BinaryColor::On)])
        .unwrap();
}

fn present(
    coordinator: &mut DisplayCoordinator,
    now: u64,
    urgency: PresentationUrgency,
) -> PresentationAttempt {
    match coordinator
        .plan_presentation(MonotonicMillis::new(now), urgency)
        .unwrap()
    {
        PresentationDecision::Present(attempt) => attempt,
        PresentationDecision::Unchanged | PresentationDecision::DeferredUntil(_) => {
            panic!("expected a presentation")
        }
    }
}

fn complete_presentation(
    coordinator: &mut DisplayCoordinator,
    attempt: PresentationAttempt,
    completed_at: u64,
    outcome: PresentationOutcome,
) {
    coordinator
        .complete_presentation(attempt, MonotonicMillis::new(completed_at), outcome)
        .unwrap();
}

fn complete_blanking(
    coordinator: &mut DisplayCoordinator,
    completed_at: u64,
    result: BlankingResult,
    buffer_retention: BufferRetention,
) -> BlankingDecision {
    coordinator
        .complete_blanking(
            MonotonicMillis::new(completed_at),
            BlankingOutcome {
                result,
                buffer_retention,
            },
        )
        .unwrap()
}

#[test]
fn policy_inputs_reject_zero_and_inverted_limits() {
    assert_eq!(DisplayDuration::from_millis(0), Err(ZeroDuration));
    assert_eq!(PartialRefreshLimit::new(0), Err(ZeroPartialRefreshLimit));
    assert_eq!(
        EinkPolicy::new(EinkPolicyConfiguration {
            telemetry_minimum: DisplayDuration::from_millis(2_000).unwrap(),
            refresh: EinkRefreshPolicy::Partial {
                maximum_consecutive: PartialRefreshLimit::new(1).unwrap(),
                full_maximum_age: SECOND,
            },
        }),
        Err(EinkPolicyError::TelemetryMinimumExceedsFullMaximumAge)
    );
}

#[test]
fn full_only_retained_policy_never_selects_partial_refresh() {
    let policy = PresentationPolicy::RetainedEink(
        EinkPolicy::new(EinkPolicyConfiguration {
            telemetry_minimum: SECOND,
            refresh: EinkRefreshPolicy::FullOnly,
        })
        .unwrap(),
    );
    let mut coordinator = coordinator(policy);
    let first = present(&mut coordinator, 0, PresentationUrgency::Immediate);
    assert_eq!(first.refresh_kind(), RefreshKind::Full);
    complete_presentation(&mut coordinator, first, 1, PresentationOutcome::Succeeded);

    draw_marker(&mut coordinator, 1);
    let changed = present(&mut coordinator, 2, PresentationUrgency::Immediate);
    assert_eq!(changed.refresh_kind(), RefreshKind::Full);
}

#[test]
fn immediate_presentations_use_the_canonical_frame() {
    let mut coordinator = coordinator(PresentationPolicy::Immediate);
    draw_marker(&mut coordinator, 3);
    let attempt = present(&mut coordinator, 0, PresentationUrgency::Immediate);
    assert_eq!(attempt.refresh_kind(), RefreshKind::Immediate);
    assert!(coordinator
        .candidate_frame(&attempt)
        .unwrap()
        .pixel_is_on(Point::new(3, 0)));
    complete_presentation(&mut coordinator, attempt, 1, PresentationOutcome::Succeeded);
}

#[test]
fn retained_presentations_defer_telemetry_and_recover_full_after_failure() {
    let mut coordinator = coordinator(retained_policy());
    let first = present(&mut coordinator, 0, PresentationUrgency::Immediate);
    assert_eq!(first.refresh_kind(), RefreshKind::Full);
    complete_presentation(&mut coordinator, first, 1, PresentationOutcome::Succeeded);

    draw_marker(&mut coordinator, 1);
    assert_eq!(
        coordinator
            .plan_presentation(MonotonicMillis::new(500), PresentationUrgency::Telemetry)
            .unwrap(),
        PresentationDecision::DeferredUntil(MonotonicMillis::new(1_001))
    );
    let partial = present(&mut coordinator, 500, PresentationUrgency::Immediate);
    assert_eq!(partial.refresh_kind(), RefreshKind::Partial);
    complete_presentation(&mut coordinator, partial, 501, PresentationOutcome::Failed);
    let recovery = present(&mut coordinator, 502, PresentationUrgency::Telemetry);
    assert_eq!(recovery.refresh_kind(), RefreshKind::Full);
}

#[test]
fn unchanged_nonempty_retained_frame_is_not_presented_twice() {
    let mut coordinator = coordinator(retained_policy());
    draw_marker(&mut coordinator, 3);
    let first = present(&mut coordinator, 0, PresentationUrgency::Immediate);
    complete_presentation(&mut coordinator, first, 1, PresentationOutcome::Succeeded);
    assert_eq!(
        coordinator
            .plan_presentation(MonotonicMillis::new(2), PresentationUrgency::Immediate)
            .unwrap(),
        PresentationDecision::Unchanged
    );
}

#[test]
fn blanking_changes_visibility_only_after_success_and_retries_failure() {
    let mut coordinator = coordinator(PresentationPolicy::Immediate);
    assert_eq!(
        coordinator.tick_blanking(MonotonicMillis::new(1_000)),
        Ok(BlankingDecision::Apply(BlankingCommand::Blank))
    );
    assert_eq!(coordinator.visibility(), DisplayVisibility::Visible);
    assert_eq!(
        complete_blanking(
            &mut coordinator,
            1_001,
            BlankingResult::Failed,
            BufferRetention::Preserved,
        ),
        BlankingDecision::RetryAt(MonotonicMillis::new(2_001))
    );
    assert_eq!(coordinator.visibility(), DisplayVisibility::Visible);
    assert_eq!(
        coordinator.tick_blanking(MonotonicMillis::new(2_001)),
        Ok(BlankingDecision::Apply(BlankingCommand::Blank))
    );
    complete_blanking(
        &mut coordinator,
        2_002,
        BlankingResult::Succeeded,
        BufferRetention::Preserved,
    );
    assert_eq!(coordinator.visibility(), DisplayVisibility::Blanked);
}

#[test]
fn scheduled_system_sleep_supersedes_auto_off() {
    let mut coordinator = coordinator(PresentationPolicy::Immediate);
    coordinator
        .schedule_blanking(MonotonicMillis::new(2_000), DisplayBlankReason::SystemSleep)
        .unwrap();
    assert_eq!(
        coordinator.tick_blanking(MonotonicMillis::new(1_000)),
        Ok(BlankingDecision::Settled)
    );
    assert_eq!(
        coordinator.tick_blanking(MonotonicMillis::new(2_000)),
        Ok(BlankingDecision::Apply(BlankingCommand::Blank))
    );
    complete_blanking(
        &mut coordinator,
        2_001,
        BlankingResult::Succeeded,
        BufferRetention::Preserved,
    );
    let button = coordinator
        .button_pressed(MonotonicMillis::new(2_002))
        .unwrap();
    assert_eq!(button.outcome(), DisplayButtonOutcome::ForwardToUi);
}

#[test]
fn auto_off_toggle_disarms_and_rearms_the_deadline() {
    let mut coordinator = coordinator(PresentationPolicy::Immediate);
    assert_eq!(
        coordinator.toggle_auto_off(MonotonicMillis::new(0)),
        Ok(DisplayAutoOff::Disabled)
    );
    assert_eq!(
        coordinator.tick_blanking(MonotonicMillis::new(1_000)),
        Ok(BlankingDecision::Settled)
    );
    assert_eq!(
        coordinator.toggle_auto_off(MonotonicMillis::new(1_001)),
        Ok(DisplayAutoOff::Enabled)
    );
    assert_eq!(
        coordinator.tick_blanking(MonotonicMillis::new(2_001)),
        Ok(BlankingDecision::Apply(BlankingCommand::Blank))
    );
}

#[test]
fn display_only_blank_consumes_exactly_the_wake_press() {
    let mut coordinator = coordinator(PresentationPolicy::Immediate);
    let decision = coordinator
        .request_blanked(DisplayBlankReason::DisplayOnly, MonotonicMillis::new(1))
        .unwrap();
    assert_eq!(decision, BlankingDecision::Apply(BlankingCommand::Blank));
    complete_blanking(
        &mut coordinator,
        2,
        BlankingResult::Succeeded,
        BufferRetention::Preserved,
    );
    let button = coordinator.button_pressed(MonotonicMillis::new(3)).unwrap();
    assert_eq!(button.outcome(), DisplayButtonOutcome::WakeAndConsume);
    assert_eq!(
        button.blanking(),
        BlankingDecision::Apply(BlankingCommand::Restore)
    );
}

#[test]
fn lost_controller_buffer_forces_retained_recovery() {
    let mut coordinator = coordinator(retained_policy());
    let first = present(&mut coordinator, 0, PresentationUrgency::Immediate);
    complete_presentation(&mut coordinator, first, 1, PresentationOutcome::Succeeded);
    coordinator
        .request_blanked(DisplayBlankReason::DisplayOnly, MonotonicMillis::new(2))
        .unwrap();
    complete_blanking(
        &mut coordinator,
        3,
        BlankingResult::Succeeded,
        BufferRetention::Lost,
    );
    assert_eq!(
        coordinator.request_visible(MonotonicMillis::new(4)),
        Ok(BlankingDecision::Apply(BlankingCommand::Restore))
    );
    complete_blanking(
        &mut coordinator,
        5,
        BlankingResult::Succeeded,
        BufferRetention::Lost,
    );
    let recovery = present(&mut coordinator, 6, PresentationUrgency::Telemetry);
    assert_eq!(recovery.refresh_kind(), RefreshKind::Full);
}

#[test]
fn retained_partial_budget_and_full_age_force_full_refreshes() {
    let mut coordinator = coordinator(retained_policy());
    let first = present(&mut coordinator, 0, PresentationUrgency::Immediate);
    complete_presentation(&mut coordinator, first, 1, PresentationOutcome::Succeeded);

    for (marker, planned, completed) in [(1, 2, 3), (2, 4, 5)] {
        draw_marker(&mut coordinator, marker);
        let partial = present(&mut coordinator, planned, PresentationUrgency::Immediate);
        assert_eq!(partial.refresh_kind(), RefreshKind::Partial);
        complete_presentation(
            &mut coordinator,
            partial,
            completed,
            PresentationOutcome::Succeeded,
        );
    }

    draw_marker(&mut coordinator, 3);
    let budget_cleanup = present(&mut coordinator, 6, PresentationUrgency::Immediate);
    assert_eq!(budget_cleanup.refresh_kind(), RefreshKind::Full);
    complete_presentation(
        &mut coordinator,
        budget_cleanup,
        7,
        PresentationOutcome::Succeeded,
    );

    draw_marker(&mut coordinator, 4);
    let age_cleanup = present(&mut coordinator, 10_007, PresentationUrgency::Immediate);
    assert_eq!(age_cleanup.refresh_kind(), RefreshKind::Full);
}

#[test]
fn time_reversal_is_rejected_by_presentation_and_blanking() {
    let mut coordinator = coordinator(PresentationPolicy::Immediate);
    let first = present(&mut coordinator, 100, PresentationUrgency::Immediate);
    complete_presentation(&mut coordinator, first, 101, PresentationOutcome::Succeeded);
    assert_eq!(
        coordinator.plan_presentation(MonotonicMillis::new(100), PresentationUrgency::Immediate),
        Err(PresentationError::TimeWentBackward)
    );

    assert_eq!(
        coordinator.tick_blanking(MonotonicMillis::new(200)),
        Ok(BlankingDecision::Settled)
    );
    assert_eq!(
        coordinator.tick_blanking(MonotonicMillis::new(199)),
        Err(BlankingError::TimeWentBackward)
    );
}

#[test]
fn system_sleep_forwards_its_wake_press_and_restores_transactionally() {
    let mut coordinator = coordinator(PresentationPolicy::Immediate);
    assert_eq!(
        coordinator.request_blanked(DisplayBlankReason::SystemSleep, MonotonicMillis::new(1)),
        Ok(BlankingDecision::Apply(BlankingCommand::Blank))
    );
    complete_blanking(
        &mut coordinator,
        2,
        BlankingResult::Succeeded,
        BufferRetention::Preserved,
    );
    let button = coordinator.button_pressed(MonotonicMillis::new(3)).unwrap();
    assert_eq!(button.outcome(), DisplayButtonOutcome::ForwardToUi);
    assert_eq!(button.blanking(), BlankingDecision::Settled);
    assert_eq!(
        coordinator.request_visible(MonotonicMillis::new(4)),
        Ok(BlankingDecision::Apply(BlankingCommand::Restore))
    );
    assert_eq!(coordinator.visibility(), DisplayVisibility::Blanked);
    complete_blanking(
        &mut coordinator,
        5,
        BlankingResult::Succeeded,
        BufferRetention::Preserved,
    );
    assert_eq!(coordinator.visibility(), DisplayVisibility::Visible);
}

#[test]
fn controller_recovery_cancels_an_in_flight_attempt_and_requires_full_recovery() {
    let mut coordinator = coordinator(retained_policy());
    let first = present(&mut coordinator, 0, PresentationUrgency::Immediate);
    complete_presentation(&mut coordinator, first, 1, PresentationOutcome::Succeeded);
    draw_marker(&mut coordinator, 1);
    let invalidated = present(&mut coordinator, 2, PresentationUrgency::Immediate);
    coordinator.invalidate_presentation().unwrap();
    assert!(matches!(
        coordinator.candidate_frame(&invalidated),
        Err(PresentationError::MissingAttempt)
    ));
    let recovery = present(&mut coordinator, 3, PresentationUrgency::Telemetry);
    assert_eq!(recovery.refresh_kind(), RefreshKind::Full);
    assert!(matches!(
        coordinator.candidate_frame(&invalidated),
        Err(PresentationError::StaleAttempt)
    ));
    assert_eq!(
        coordinator.complete_presentation(
            invalidated,
            MonotonicMillis::new(4),
            PresentationOutcome::Succeeded,
        ),
        Err(PresentationError::StaleAttempt)
    );
    complete_presentation(
        &mut coordinator,
        recovery,
        4,
        PresentationOutcome::Succeeded,
    );
}

#[test]
fn blanking_and_presentation_attempts_are_serialized() {
    let mut coordinator = coordinator(PresentationPolicy::Immediate);
    let attempt = present(&mut coordinator, 0, PresentationUrgency::Immediate);
    assert!(matches!(
        coordinator.candidate_mut(),
        Err(PresentationError::AttemptInFlight)
    ));
    assert_eq!(
        coordinator.request_blanked(DisplayBlankReason::DisplayOnly, MonotonicMillis::new(1)),
        Err(BlankingError::PresentationInFlight)
    );
    complete_presentation(&mut coordinator, attempt, 1, PresentationOutcome::Succeeded);
    coordinator
        .request_blanked(DisplayBlankReason::DisplayOnly, MonotonicMillis::new(2))
        .unwrap();
    assert_eq!(
        coordinator.schedule_blanking(MonotonicMillis::new(3), DisplayBlankReason::DisplayOnly,),
        Err(BlankingError::OperationInFlight)
    );
    assert_eq!(
        coordinator.toggle_auto_off(MonotonicMillis::new(3)),
        Err(BlankingError::OperationInFlight)
    );
    assert_eq!(
        coordinator.plan_presentation(MonotonicMillis::new(2), PresentationUrgency::Immediate),
        Err(PresentationError::DisplayNotVisible)
    );
}

#[test]
fn visible_display_without_user_blanking_reports_that_exact_capability() {
    let coordinator = DisplayCoordinator::available(
        PresentationPolicy::Immediate,
        UserBlankingPolicy::Unavailable,
    );
    assert_eq!(coordinator.visibility(), DisplayVisibility::Visible);
    assert_eq!(coordinator.user_blanking(), UserBlanking::unavailable());
}

#[test]
fn unavailable_display_cannot_claim_user_blanking() {
    let mut coordinator = DisplayCoordinator::unavailable();
    assert_eq!(coordinator.visibility(), DisplayVisibility::Unavailable);
    assert_eq!(coordinator.user_blanking(), UserBlanking::unavailable());
    assert_eq!(
        coordinator.tick_blanking(MonotonicMillis::new(0)),
        Err(BlankingError::DisplayUnavailable)
    );
}
