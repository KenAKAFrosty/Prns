use super::*;
use crate::remote_control::RemoteControlPairingCompleted;

pub(super) fn prepared(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
) -> RemoteControlPairingAttemptId {
    let attempt_id = authorizing(state, fixture);
    assert_eq!(
        state.prepare_authorization(
            attempt_id,
            &fixture.target_signer,
            AUTHORIZATION_PERSISTED_AT
        ),
        PrepareRemoteControlTargetPairingAuthorizationOutcome::Prepared { attempt_id }
    );
    attempt_id
}

#[test]
fn preparation_is_inert_outside_authorization() {
    let setups: [fn(
        &mut RemoteControlTargetPairingState,
        &TargetPairingFixture,
    ) -> RemoteControlPairingAttemptId; 5] = [
        offer_prepared,
        awaiting_both,
        awaiting_target_approval,
        awaiting_controller_commit,
        completing,
    ];
    let fixture = TargetPairingFixture::new();
    for setup in setups {
        let mut expected = RemoteControlTargetPairingState::default();
        let mut actual = RemoteControlTargetPairingState::default();
        let attempt_id = setup(&mut expected, &fixture);
        assert_eq!(setup(&mut actual, &fixture), attempt_id);
        assert_eq!(
            actual.prepare_authorization(
                attempt_id,
                &fixture.target_signer,
                AUTHORIZATION_PERSISTED_AT
            ),
            PrepareRemoteControlTargetPairingAuthorizationOutcome::NoAuthorizationOwed
        );
        assert_eq!(actual, expected);
    }
}

#[test]
fn preparation_holds_completion_until_storage_settles_even_after_deadline_and_link_loss() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let attempt_id = authorizing(&mut state, &fixture);
    let prepared = PrepareRemoteControlTargetPairingAuthorizationOutcome::Prepared { attempt_id };
    assert_eq!(
        state.prepare_authorization(
            attempt_id,
            &fixture.target_signer,
            AUTHORIZATION_PERSISTED_AT
        ),
        prepared
    );
    assert!(matches!(
        state.view(),
        RemoteControlTargetPairingView::Authorizing(_)
    ));
    assert_eq!(
        state.expire(ATTEMPT_EXPIRES_AT),
        ExpireRemoteControlTargetPairingOutcome::FinalizationInProgress { attempt_id }
    );
    assert_eq!(
        state.close_link(fixture.link_id),
        CloseRemoteControlTargetPairingLinkOutcome::FinalizationInProgress { attempt_id }
    );
    assert_eq!(
        state.reject(attempt_id, ATTEMPT_EXPIRES_AT),
        RejectRemoteControlTargetPairingOutcome::FinalizationInProgress { attempt_id }
    );
    // An idempotent retry uses the retained signature, not another signing operation.
    assert_eq!(
        state.prepare_authorization(attempt_id, &signer(0x53), ATTEMPT_EXPIRES_AT),
        prepared
    );
    let completed = RemoteControlPairingCompleted::signed_by(
        &fixture.target_signer,
        fixture.prepared().transcript(),
    )
    .unwrap();
    assert_eq!(
        state.authorization_persisted(attempt_id, &signer(0x53), ATTEMPT_EXPIRES_AT),
        PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed {
            attempt_id,
            responder: fixture.commit(0x61).responder,
            completed,
        }
    );
    assert!(matches!(
        state.view(),
        RemoteControlTargetPairingView::Completing(_)
    ));
}

#[test]
fn preparation_rejects_the_exact_deadline_without_erasing_failure_settlement() {
    for now in [ATTEMPT_EXPIRES_AT, InstantMillis(ATTEMPT_EXPIRES_AT.0 + 1)] {
        let fixture = TargetPairingFixture::new();
        let mut state = RemoteControlTargetPairingState::default();
        let attempt_id = authorizing(&mut state, &fixture);
        assert_eq!(
            state.prepare_authorization(attempt_id, &fixture.target_signer, now),
            PrepareRemoteControlTargetPairingAuthorizationOutcome::DeadlineElapsed { attempt_id }
        );
        assert_eq!(
            state.authorization_failed(attempt_id),
            FailRemoteControlTargetPairingAuthorizationOutcome::Aborted {
                attempt_id,
                context: fixture.context(),
                responder: fixture.commit(0x61).responder,
            }
        );
        assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
    }
}

#[test]
fn preparation_admits_one_tick_before_deadline_and_can_abort_failed_storage() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let attempt_id = authorizing(&mut state, &fixture);
    assert_eq!(
        state.prepare_authorization(
            attempt_id,
            &fixture.target_signer,
            InstantMillis(ATTEMPT_EXPIRES_AT.0 - 1)
        ),
        PrepareRemoteControlTargetPairingAuthorizationOutcome::Prepared { attempt_id }
    );
    assert_eq!(
        state.authorization_failed(attempt_id),
        FailRemoteControlTargetPairingAuthorizationOutcome::Aborted {
            attempt_id,
            context: fixture.context(),
            responder: fixture.commit(0x61).responder,
        }
    );
    assert_eq!(
        state.prepare_authorization(
            attempt_id,
            &fixture.target_signer,
            AUTHORIZATION_PERSISTED_AT
        ),
        PrepareRemoteControlTargetPairingAuthorizationOutcome::NoAuthorizationOwed
    );
}

#[test]
fn mismatched_preparation_and_settlement_preserve_the_reserved_completion() {
    let fixture = TargetPairingFixture::new();
    let other = TargetPairingFixture::with_route(0x75, 0x86);
    let other_id = other.prepared().transcript().into();
    let mut state = RemoteControlTargetPairingState::default();
    let attempt_id = authorizing(&mut state, &fixture);
    for now in [AUTHORIZATION_PERSISTED_AT, ATTEMPT_EXPIRES_AT] {
        assert_eq!(
            state.prepare_authorization(attempt_id, &fixture.target_signer, now),
            PrepareRemoteControlTargetPairingAuthorizationOutcome::Prepared { attempt_id }
        );
        assert_eq!(
            state.prepare_authorization(other_id, &fixture.target_signer, now),
            PrepareRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
                prepared: other_id,
                active: attempt_id
            }
        );
        assert_eq!(
            state.authorization_failed(other_id),
            FailRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
                settled: other_id,
                active: attempt_id
            }
        );
        assert_eq!(
            state.authorization_persisted(other_id, &fixture.target_signer, now),
            PersistRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
                settled: other_id,
                active: attempt_id
            }
        );
    }
    assert!(matches!(
        state.authorization_persisted(attempt_id, &fixture.target_signer, ATTEMPT_EXPIRES_AT),
        PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed { .. }
    ));
}

#[test]
fn signing_failure_does_not_reserve_an_unsigned_completion_or_extend_admission() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let attempt_id = authorizing(&mut state, &fixture);
    let wrong_signer = signer(0x53);
    assert_eq!(state.prepare_authorization(attempt_id, &wrong_signer, AUTHORIZATION_PERSISTED_AT),
        PrepareRemoteControlTargetPairingAuthorizationOutcome::SigningFailed {
            attempt_id,
            error: crate::remote_control::RemoteControlPairingCompletionSigningError::TargetIdentityMismatch {
                expected: fixture.prepared().transcript().target().identity_hash(),
                found: wrong_signer.identity_hash(),
            },
        });
    assert_eq!(
        state.prepare_authorization(attempt_id, &fixture.target_signer, ATTEMPT_EXPIRES_AT),
        PrepareRemoteControlTargetPairingAuthorizationOutcome::DeadlineElapsed { attempt_id }
    );
}
