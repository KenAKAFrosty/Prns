#![allow(clippy::panic, clippy::unwrap_used)]

use super::*;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::vault::IdentitySecretKey;
use crate::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity, RemoteControlPairingAttemptId,
    RemoteControlPairingAttemptTimeout, RemoteControlPairingBegin, RemoteControlPairingCommit,
    RemoteControlPairingContext, RemoteControlPairingIdentity, RemoteControlPairingInvitationCode,
    RemoteControlPairingOffer, RemoteControlPairingPermissions, RemoteControlPairingPreparedOffer,
    RemoteControlPairingSession, RemoteControlPairingWindow, RemoteControlRequestKind,
    RemoteControlRequestSet,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::units::{DurationMillis, InstantMillis};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

const PAIRING_OPENED_AT: InstantMillis = InstantMillis(1_000);
const PAIRING_EXPIRES_AT: InstantMillis = InstantMillis(20_000);
const ATTEMPT_STARTED_AT: InstantMillis = InstantMillis(2_000);
const ATTEMPT_EXPIRES_AT: InstantMillis = InstantMillis(7_000);
const ATTEMPT_TIMEOUT: DurationMillis = DurationMillis(5_000);
const AUTHORIZATION_PERSISTED_AT: InstantMillis = InstantMillis(3_000);

struct TargetPairingFixture {
    target_signer: InMemoryNodeIdentity,
    session: RemoteControlPairingSession,
    link_id: LinkId,
    begin: RemoteControlPairingBegin,
}

impl TargetPairingFixture {
    fn new() -> Self {
        Self::with_route(0x73, 0x84)
    }

    fn with_route(endpoint_fill: u8, link_fill: u8) -> Self {
        let session = RemoteControlPairingSession::new(
            RemoteControlPairingIdentity::new(IdentityHash::new(
                [endpoint_fill; TRUNCATED_HASH_BYTE_LEN],
            )),
            RemoteControlPairingWindow::new(PAIRING_OPENED_AT, PAIRING_EXPIRES_AT).unwrap(),
            permissions(),
            RemoteControlPairingAttemptTimeout::try_from(ATTEMPT_TIMEOUT).unwrap(),
            invitation_code().verifier(),
        );
        Self {
            target_signer: signer(0x52),
            begin: RemoteControlPairingBegin::new(
                controller(0x31),
                session.endpoint(),
                invitation_code(),
            ),
            session,
            link_id: LinkId::new([link_fill; TRUNCATED_HASH_BYTE_LEN]),
        }
    }

    fn context(&self) -> RemoteControlPairingContext {
        RemoteControlPairingContext::new(self.session.endpoint(), self.link_id)
    }

    fn with_window(opened_at: u64, expires_at: u64, timeout: u64) -> Self {
        let mut fixture = Self::new();
        fixture.session = RemoteControlPairingSession::new(
            RemoteControlPairingIdentity::new(fixture.session.identity().identity_hash()),
            RemoteControlPairingWindow::new(InstantMillis(opened_at), InstantMillis(expires_at))
                .unwrap(),
            permissions(),
            RemoteControlPairingAttemptTimeout::try_from(DurationMillis(timeout)).unwrap(),
            invitation_code().verifier(),
        );
        fixture
    }

    fn permissions(&self) -> &RemoteControlPairingPermissions {
        self.session.permissions()
    }

    fn attempt_timeout(&self) -> RemoteControlPairingAttemptTimeout {
        RemoteControlPairingAttemptTimeout::try_from(ATTEMPT_TIMEOUT).unwrap()
    }

    fn prepared(&self) -> RemoteControlPairingPreparedOffer {
        RemoteControlPairingPreparedOffer::new(
            &self.target_signer,
            self.context(),
            &self.begin,
            self.permissions().clone(),
            self.attempt_timeout(),
        )
    }

    fn commit(&self, request_fill: u8) -> RemoteControlTargetPairingCommitArrival {
        RemoteControlTargetPairingCommitArrival::new(
            RemoteControlPairingCommit::new(self.prepared().transcript()),
            responder(self.link_id, request_fill),
            self.begin.controller().identity_hash(),
        )
    }

    fn begin_arrival(&self, request_fill: u8) -> RemoteControlTargetPairingBeginArrival {
        RemoteControlTargetPairingBeginArrival::new(
            RemoteControlPairingBegin::new(
                *self.begin.controller(),
                self.session.endpoint(),
                invitation_code(),
            ),
            responder(self.link_id, request_fill),
            self.begin.controller().identity_hash(),
        )
    }
}

fn invitation_code() -> RemoteControlPairingInvitationCode {
    RemoteControlPairingInvitationCode::from_value(0x1234_ABCD)
}

fn signer(fill: u8) -> InMemoryNodeIdentity {
    InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
        [fill; crate::identity::IDENTITY_SECRET_KEY_LEN],
    ))
}

fn controller(fill: u8) -> RemoteControlControllerIdentity {
    let signer = signer(fill);
    RemoteControlControllerIdentity::new(IdentityPublicKeys {
        encryption: signer.encryption_public_key(),
        signing: signer.signing_public_key(),
    })
}

fn permissions() -> RemoteControlPairingPermissions {
    RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
        RemoteControlRequestKind::Describe,
    ))
    .unwrap()
}

fn responder(link_id: LinkId, request_fill: u8) -> RemoteControlTargetPairingResponder {
    RemoteControlTargetPairingResponder::new(
        link_id,
        RequestId([request_fill; TRUNCATED_HASH_BYTE_LEN]),
    )
}

fn begin(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
) -> RemoteControlPairingAttemptId {
    begin_with_offer(state, fixture).0
}

fn begin_with_offer(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
) -> (RemoteControlPairingAttemptId, RemoteControlPairingOffer) {
    let (attempt_id, offer) = prepare_offer(state, fixture);
    let DispatchRemoteControlTargetPairingOfferOutcome::AwaitingConfirmation { attempt } =
        state.offer_dispatched(attempt_id)
    else {
        panic!("prepared offer")
    };
    assert_eq!(attempt.attempt_id(), attempt_id);
    (attempt_id, offer)
}

fn prepare_offer(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
) -> (RemoteControlPairingAttemptId, RemoteControlPairingOffer) {
    prepare_offer_at(state, fixture, ATTEMPT_STARTED_AT)
}

fn prepare_offer_at(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
    started_at: InstantMillis,
) -> (RemoteControlPairingAttemptId, RemoteControlPairingOffer) {
    let arrival = fixture.begin_arrival(0x60);
    match state.begin(
        &fixture.target_signer,
        &fixture.session,
        &arrival,
        started_at,
    ) {
        BeginRemoteControlTargetPairingOutcome::OfferPrepared {
            attempt_id,
            responder: offered_to,
            offer,
        } => {
            assert_eq!(offered_to, arrival.responder());
            (attempt_id, offer)
        }
        BeginRemoteControlTargetPairingOutcome::Expired { .. }
        | BeginRemoteControlTargetPairingOutcome::CompletionRetentionExpired { .. }
        | BeginRemoteControlTargetPairingOutcome::Busy { .. }
        | BeginRemoteControlTargetPairingOutcome::PairingUnavailable { .. }
        | BeginRemoteControlTargetPairingOutcome::Rejected { .. } => {
            panic!("fresh state")
        }
    }
}

fn attempt_view(
    state: &RemoteControlTargetPairingState,
) -> RemoteControlTargetPairingAttemptView<'_> {
    match state.view() {
        RemoteControlTargetPairingView::OfferPrepared(attempt)
        | RemoteControlTargetPairingView::AwaitingBoth(attempt)
        | RemoteControlTargetPairingView::AwaitingTargetApproval(attempt)
        | RemoteControlTargetPairingView::AwaitingControllerCommit(attempt)
        | RemoteControlTargetPairingView::Authorizing(attempt) => attempt,
        RemoteControlTargetPairingView::Completing(attempt) => attempt,
        RemoteControlTargetPairingView::Idle => panic!("active attempt"),
    }
}

fn authorize(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
) -> (RemoteControlPairingAttemptId, RemoteControlControllerGrant) {
    let attempt_id = begin(state, fixture);
    assert_eq!(
        state.approve(attempt_id, ATTEMPT_STARTED_AT),
        ApproveRemoteControlTargetPairingOutcome::AwaitingControllerCommit { attempt_id },
    );
    let CommitRemoteControlTargetPairingOutcome::AuthorizationOwed {
        attempt_id: authorized,
        grant,
    } = state.commit(fixture.commit(0x61), ATTEMPT_STARTED_AT)
    else {
        panic!("authorization owed")
    };
    assert_eq!(authorized, attempt_id);
    (attempt_id, grant)
}

fn offer_prepared(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
) -> RemoteControlPairingAttemptId {
    prepare_offer(state, fixture).0
}

fn awaiting_both(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
) -> RemoteControlPairingAttemptId {
    begin(state, fixture)
}

fn awaiting_target_approval(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
) -> RemoteControlPairingAttemptId {
    let attempt_id = begin(state, fixture);
    assert_eq!(
        state.commit(fixture.commit(0x61), ATTEMPT_STARTED_AT),
        CommitRemoteControlTargetPairingOutcome::AwaitingTargetApproval { attempt_id },
    );
    attempt_id
}

fn awaiting_controller_commit(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
) -> RemoteControlPairingAttemptId {
    let attempt_id = begin(state, fixture);
    assert_eq!(
        state.approve(attempt_id, ATTEMPT_STARTED_AT),
        ApproveRemoteControlTargetPairingOutcome::AwaitingControllerCommit { attempt_id },
    );
    attempt_id
}

fn authorizing(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
) -> RemoteControlPairingAttemptId {
    authorize(state, fixture).0
}

fn completing(
    state: &mut RemoteControlTargetPairingState,
    fixture: &TargetPairingFixture,
) -> RemoteControlPairingAttemptId {
    let attempt_id = authorizing(state, fixture);
    assert!(matches!(
        state.authorization_persisted(
            attempt_id,
            &fixture.target_signer,
            AUTHORIZATION_PERSISTED_AT,
        ),
        PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed {
            attempt_id: completed,
            ..
        } if completed == attempt_id
    ));
    attempt_id
}

#[test]
fn competing_begins_preserve_every_occupied_target_phase_exactly() {
    let setups: [fn(
        &mut RemoteControlTargetPairingState,
        &TargetPairingFixture,
    ) -> RemoteControlPairingAttemptId; 6] = [
        offer_prepared,
        awaiting_both,
        awaiting_target_approval,
        awaiting_controller_commit,
        authorizing,
        completing,
    ];

    for setup in setups {
        let expected_fixture = TargetPairingFixture::new();
        let mut expected = RemoteControlTargetPairingState::default();
        let expected_id = setup(&mut expected, &expected_fixture);
        let actual_fixture = TargetPairingFixture::new();
        let mut actual = RemoteControlTargetPairingState::default();
        assert_eq!(setup(&mut actual, &actual_fixture), expected_id);
        let competing_controller = controller(0x71);
        let competing = RemoteControlTargetPairingBeginArrival::new(
            RemoteControlPairingBegin::new(
                competing_controller,
                actual_fixture.session.endpoint(),
                invitation_code(),
            ),
            responder(LinkId::new([0x72; TRUNCATED_HASH_BYTE_LEN]), 0x73),
            competing_controller.identity_hash(),
        );

        assert_eq!(
            actual.begin(
                &actual_fixture.target_signer,
                &actual_fixture.session,
                &competing,
                ATTEMPT_STARTED_AT,
            ),
            BeginRemoteControlTargetPairingOutcome::Busy {
                active: expected_id,
            },
        );
        assert_eq!(actual, expected);
    }
}

#[test]
fn an_invalid_invitation_cannot_probe_or_mutate_a_retained_target_attempt() {
    let expected_fixture = TargetPairingFixture::new();
    let mut expected = RemoteControlTargetPairingState::default();
    let attempt_id = begin(&mut expected, &expected_fixture);
    let actual_fixture = TargetPairingFixture::new();
    let mut actual = RemoteControlTargetPairingState::default();
    assert_eq!(begin(&mut actual, &actual_fixture), attempt_id);
    let competing_controller = controller(0x71);
    let competing = RemoteControlTargetPairingBeginArrival::new(
        RemoteControlPairingBegin::new(
            competing_controller,
            actual_fixture.session.endpoint(),
            RemoteControlPairingInvitationCode::from_value(0xDEAD_BEEF),
        ),
        responder(LinkId::new([0x72; TRUNCATED_HASH_BYTE_LEN]), 0x73),
        competing_controller.identity_hash(),
    );

    assert_eq!(
        actual.begin(
            &actual_fixture.target_signer,
            &actual_fixture.session,
            &competing,
            ATTEMPT_EXPIRES_AT,
        ),
        BeginRemoteControlTargetPairingOutcome::Rejected {
            rejected: competing.responder(),
            reason: RemoteControlTargetPairingBeginRejection::InvalidInvitationProof,
        },
    );
    assert_eq!(actual, expected);
}

#[test]
fn attempt_windows_are_positive_bounded_by_the_pairing_window_and_overflow_safe() {
    let pairing_window =
        RemoteControlPairingWindow::new(PAIRING_OPENED_AT, PAIRING_EXPIRES_AT).unwrap();
    let timeout = RemoteControlPairingAttemptTimeout::try_from(DurationMillis(19_000)).unwrap();
    assert_eq!(
        RemoteControlTargetPairingAttemptWindow::new(PAIRING_OPENED_AT, timeout, &pairing_window,),
        Ok(RemoteControlTargetPairingAttemptWindow {
            started_at: PAIRING_OPENED_AT,
            attempt_timeout: timeout,
            expires_at: PAIRING_EXPIRES_AT,
        }),
    );
    assert_eq!(
        RemoteControlTargetPairingAttemptWindow::new(PAIRING_EXPIRES_AT, timeout, &pairing_window,),
        Err(
            RemoteControlTargetPairingAttemptWindowError::PairingWindowElapsed {
                started_at: PAIRING_EXPIRES_AT,
                pairing_expires_at: PAIRING_EXPIRES_AT,
            }
        ),
    );
    let exceeds = RemoteControlPairingAttemptTimeout::try_from(DurationMillis(19_001)).unwrap();
    assert_eq!(
        RemoteControlTargetPairingAttemptWindow::new(PAIRING_OPENED_AT, exceeds, &pairing_window,),
        Err(
            RemoteControlTargetPairingAttemptWindowError::ExceedsPairingWindow {
                attempt_expires_at: InstantMillis(20_001),
                pairing_expires_at: PAIRING_EXPIRES_AT,
            }
        ),
    );
    let near_limit =
        RemoteControlPairingWindow::new(InstantMillis(u64::MAX - 10), InstantMillis(u64::MAX))
            .unwrap();
    let overflow = RemoteControlPairingAttemptTimeout::try_from(DurationMillis(11)).unwrap();
    assert_eq!(
        RemoteControlTargetPairingAttemptWindow::new(
            InstantMillis(u64::MAX - 10),
            overflow,
            &near_limit,
        ),
        Err(
            RemoteControlTargetPairingAttemptWindowError::DeadlineOverflow {
                started_at: InstantMillis(u64::MAX - 10),
                attempt_timeout: overflow,
            }
        ),
    );
}

#[test]
fn begin_signs_the_timeout_that_fits_the_remaining_pairing_window() {
    let cases = [
        // A 120-second invitation with a configured 60-second attempt.
        (1_000, 121_000, 1_000, 60_000, 61_000),
        (1_000, 121_000, 61_000, 60_000, 121_000),
        (1_000, 121_000, 76_000, 45_000, 121_000),
        (1_000, 121_000, 120_999, 1, 121_000),
        // Bound before adding, so a still-open near-MAX window cannot overflow.
        (
            u64::MAX - 120_000,
            u64::MAX,
            u64::MAX - 45_000,
            45_000,
            u64::MAX,
        ),
        (u64::MAX - 120_000, u64::MAX, u64::MAX - 1, 1, u64::MAX),
    ];
    for (opened_at, expires_at, started_at, expected_timeout, expected_expiry) in cases {
        let fixture = TargetPairingFixture::with_window(opened_at, expires_at, 60_000);
        let mut state = RemoteControlTargetPairingState::default();
        let (attempt_id, offer) = prepare_offer_at(&mut state, &fixture, InstantMillis(started_at));
        let transcript = offer.verify(fixture.context(), &fixture.begin).unwrap();
        let window = attempt_view(&state).window();

        assert_eq!(
            offer.attempt_timeout().duration(),
            DurationMillis(expected_timeout)
        );
        assert_eq!(transcript.attempt_timeout(), offer.attempt_timeout());
        assert_eq!(window.attempt_timeout(), offer.attempt_timeout());
        assert_eq!(window.expires_at(), InstantMillis(expected_expiry));
        assert_eq!(window.started_at(), InstantMillis(started_at));
        assert_eq!(attempt_id, RemoteControlPairingAttemptId::from(&transcript));
        assert_eq!(
            fixture.session.attempt_timeout().duration(),
            DurationMillis(60_000)
        );
        assert!(window.expires_at() > window.started_at());
        assert!(window.expires_at() <= fixture.session.window().expires_at());
    }
}

#[test]
fn begin_still_refuses_elapsed_pairing_windows_without_an_offer() {
    for (opened_at, expires_at, started_at) in [
        (1_000, 121_000, 121_000),
        (1_000, 121_000, 121_001),
        (u64::MAX - 120_000, u64::MAX, u64::MAX),
    ] {
        let fixture = TargetPairingFixture::with_window(opened_at, expires_at, 60_000);
        let mut state = RemoteControlTargetPairingState::default();
        assert_eq!(
            state.begin(
                &fixture.target_signer,
                &fixture.session,
                &fixture.begin_arrival(0x60),
                InstantMillis(started_at),
            ),
            BeginRemoteControlTargetPairingOutcome::PairingUnavailable {
                reason: RemoteControlTargetPairingAttemptWindowError::PairingWindowElapsed {
                    started_at: InstantMillis(started_at),
                    pairing_expires_at: InstantMillis(expires_at),
                },
            },
        );
        assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
    }
}

#[test]
fn late_attempt_confirmation_expires_at_the_original_pairing_deadline() {
    let fixture = TargetPairingFixture::with_window(1_000, 121_000, 60_000);
    let mut state = RemoteControlTargetPairingState::default();
    let (attempt_id, _) = prepare_offer_at(&mut state, &fixture, InstantMillis(76_000));
    assert!(matches!(
        state.offer_dispatched(attempt_id),
        DispatchRemoteControlTargetPairingOfferOutcome::AwaitingConfirmation { .. }
    ));
    assert_eq!(
        state.expire(InstantMillis(120_999)),
        ExpireRemoteControlTargetPairingOutcome::NotDue {
            expires_at: InstantMillis(121_000)
        },
    );
    assert_eq!(
        state.approve(attempt_id, InstantMillis(121_000)),
        ApproveRemoteControlTargetPairingOutcome::Expired {
            expired: RemoteControlTargetPairingAborted::AwaitingBoth {
                attempt_id,
                context: fixture.context(),
            },
        },
    );
    assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
}

#[test]
fn late_attempt_authorization_cannot_extend_the_pairing_deadline() {
    for persisted_at in [120_999, 121_000, 121_001] {
        let fixture = TargetPairingFixture::with_window(1_000, 121_000, 60_000);
        let mut state = RemoteControlTargetPairingState::default();
        let now = InstantMillis(76_000);
        let (attempt_id, offer) = prepare_offer_at(&mut state, &fixture, now);
        let transcript = offer.verify(fixture.context(), &fixture.begin).unwrap();
        state.offer_dispatched(attempt_id);
        assert_eq!(
            state.approve(attempt_id, now),
            ApproveRemoteControlTargetPairingOutcome::AwaitingControllerCommit { attempt_id },
        );
        // Commit the actual shortened transcript, not the configured 60-second offer.
        let arrival = RemoteControlTargetPairingCommitArrival::new(
            RemoteControlPairingCommit::new(&transcript),
            responder(fixture.link_id, 0x61),
            fixture.begin.controller().identity_hash(),
        );
        let CommitRemoteControlTargetPairingOutcome::AuthorizationOwed { grant, .. } =
            state.commit(arrival, now)
        else {
            panic!("the signed shortened offer can be committed")
        };
        let result = state.authorization_persisted(
            attempt_id,
            &fixture.target_signer,
            InstantMillis(persisted_at),
        );
        if persisted_at < 121_000 {
            assert!(matches!(
                result,
                PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed { .. }
            ));
            assert_eq!(
                attempt_view(&state).window().expires_at(),
                InstantMillis(121_000)
            );
        } else {
            assert_eq!(
                result,
                PersistRemoteControlTargetPairingAuthorizationOutcome::AuthorizationPersistedAfterDeadline {
                    attempt_id,
                    context: fixture.context(),
                    grant,
                },
            );
            assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
        }
    }
}

#[test]
fn begin_retains_one_exact_offer_and_refuses_competing_attempts() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let (attempt_id, offer) = begin_with_offer(&mut state, &fixture);
    let view = attempt_view(&state);

    assert_eq!(view.attempt_id(), attempt_id);
    assert_eq!(view.context(), fixture.context());
    assert_eq!(view.controller(), fixture.begin.controller());
    assert_eq!(view.permissions(), fixture.permissions());
    assert_eq!(view.window().expires_at(), ATTEMPT_EXPIRES_AT);
    assert_eq!(&offer, fixture.prepared().offer());

    let competing = TargetPairingFixture::with_route(0x74, 0x85);
    let competing_arrival = competing.begin_arrival(0x61);
    assert_eq!(
        state.begin(
            &competing.target_signer,
            &competing.session,
            &competing_arrival,
            ATTEMPT_STARTED_AT,
        ),
        BeginRemoteControlTargetPairingOutcome::Busy { active: attempt_id },
    );
    assert_eq!(attempt_view(&state).attempt_id(), attempt_id);
}

#[test]
fn begin_requires_the_claimed_controller_to_own_the_identified_link() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let identified = controller(0x32).identity_hash();
    let arrival = RemoteControlTargetPairingBeginArrival::new(
        RemoteControlPairingBegin::new(
            *fixture.begin.controller(),
            fixture.session.endpoint(),
            invitation_code(),
        ),
        responder(fixture.link_id, 0x60),
        identified,
    );

    assert_eq!(
        state.begin(
            &fixture.target_signer,
            &fixture.session,
            &arrival,
            ATTEMPT_STARTED_AT,
        ),
        BeginRemoteControlTargetPairingOutcome::Rejected {
            rejected: arrival.responder(),
            reason: RemoteControlTargetPairingBeginRejection::ControllerIdentityMismatch {
                claimed: fixture.begin.controller().identity_hash(),
                identified,
            },
        },
    );
    assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
}

#[test]
fn only_the_exact_prepared_offer_dispatch_failure_aborts_the_attempt() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let (attempt_id, _) = prepare_offer(&mut state, &fixture);
    let mut other_state = RemoteControlTargetPairingState::default();
    let (other_id, _) = prepare_offer(
        &mut other_state,
        &TargetPairingFixture::with_route(0x74, 0x85),
    );

    assert_eq!(
        state.offer_dispatch_failed(other_id),
        FailRemoteControlTargetPairingOfferDispatchOutcome::AttemptMismatch {
            failed: other_id,
            active: attempt_id,
        },
    );
    assert_eq!(attempt_view(&state).attempt_id(), attempt_id);
    assert_eq!(
        state.offer_dispatch_failed(attempt_id),
        FailRemoteControlTargetPairingOfferDispatchOutcome::Aborted {
            attempt_id,
            context: fixture.context(),
        },
    );
    assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
    assert_eq!(
        state.offer_dispatch_failed(attempt_id),
        FailRemoteControlTargetPairingOfferDispatchOutcome::NoOfferPrepared,
    );
}

#[test]
fn a_prepared_offer_must_be_dispatched_before_confirmation() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let (attempt_id, _) = prepare_offer(&mut state, &fixture);
    let other = begin(
        &mut RemoteControlTargetPairingState::default(),
        &TargetPairingFixture::with_route(0x74, 0x85),
    );

    assert!(matches!(
        state.view(),
        RemoteControlTargetPairingView::OfferPrepared(attempt)
            if attempt.attempt_id() == attempt_id
    ));
    assert_eq!(
        state.approve(other, ATTEMPT_STARTED_AT),
        ApproveRemoteControlTargetPairingOutcome::AttemptMismatch {
            requested: other,
            active: attempt_id,
        },
    );
    assert_eq!(
        state.approve(attempt_id, ATTEMPT_STARTED_AT),
        ApproveRemoteControlTargetPairingOutcome::OfferPendingDispatch { attempt_id },
    );
    assert_eq!(
        state.commit(fixture.commit(0x61), ATTEMPT_STARTED_AT),
        CommitRemoteControlTargetPairingOutcome::Rejected {
            rejected: responder(fixture.link_id, 0x61),
            reason: RemoteControlTargetPairingCommitRejection::OfferPendingDispatch,
        },
    );
    assert_eq!(
        state.reject(other, ATTEMPT_STARTED_AT),
        RejectRemoteControlTargetPairingOutcome::AttemptMismatch {
            requested: other,
            active: attempt_id,
        },
    );
    assert_eq!(
        state.reject(attempt_id, ATTEMPT_STARTED_AT),
        RejectRemoteControlTargetPairingOutcome::OfferPendingDispatch { attempt_id },
    );
    assert_eq!(
        state.offer_dispatched(other),
        DispatchRemoteControlTargetPairingOfferOutcome::AttemptMismatch {
            dispatched: other,
            active: attempt_id,
        },
    );
    let DispatchRemoteControlTargetPairingOfferOutcome::AwaitingConfirmation { attempt } =
        state.offer_dispatched(attempt_id)
    else {
        panic!("matching prepared offer")
    };
    assert_eq!(attempt.attempt_id(), attempt_id);
    assert!(matches!(
        state.view(),
        RemoteControlTargetPairingView::AwaitingBoth(attempt)
            if attempt.attempt_id() == attempt_id
    ));
}

#[test]
fn local_approval_then_controller_commit_owes_the_exact_grant() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let (attempt_id, grant) = authorize(&mut state, &fixture);

    assert_eq!(grant.controller(), fixture.begin.controller());
    assert_eq!(
        grant.permitted_requests(),
        fixture.permissions().permitted_requests()
    );
    assert!(matches!(
        state.view(),
        RemoteControlTargetPairingView::Authorizing(attempt) if attempt.attempt_id() == attempt_id
    ));
}

#[test]
fn controller_commit_then_local_approval_owes_the_same_grant() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let attempt_id = begin(&mut state, &fixture);

    assert_eq!(
        state.commit(fixture.commit(0x61), ATTEMPT_STARTED_AT),
        CommitRemoteControlTargetPairingOutcome::AwaitingTargetApproval { attempt_id },
    );
    assert_eq!(
        state.approve(attempt_id, ATTEMPT_STARTED_AT),
        ApproveRemoteControlTargetPairingOutcome::AuthorizationOwed {
            attempt_id,
            grant: (fixture.begin.controller(), fixture.permissions()).into(),
        },
    );
}

#[test]
fn mismatched_attempts_links_identities_transcripts_and_duplicate_commits_never_advance() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let attempt_id = begin(&mut state, &fixture);
    let other = TargetPairingFixture::with_route(0x75, 0x86);
    let other_controller = controller(0x76).identity_hash();
    let other_id: RemoteControlPairingAttemptId = other.prepared().transcript().into();

    assert_eq!(
        state.approve(other_id, ATTEMPT_STARTED_AT),
        ApproveRemoteControlTargetPairingOutcome::AttemptMismatch {
            requested: other_id,
            active: attempt_id,
        },
    );
    let wrong_link = RemoteControlTargetPairingCommitArrival::new(
        RemoteControlPairingCommit::new(fixture.prepared().transcript()),
        responder(LinkId::new([0x85; TRUNCATED_HASH_BYTE_LEN]), 0x62),
        fixture.begin.controller().identity_hash(),
    );
    assert_eq!(
        state.commit(wrong_link, ATTEMPT_STARTED_AT),
        CommitRemoteControlTargetPairingOutcome::Rejected {
            rejected: wrong_link.responder(),
            reason: RemoteControlTargetPairingCommitRejection::WrongLink {
                expected: fixture.link_id,
                found: wrong_link.responder().link_id(),
            },
        },
    );
    let wrong_identity = RemoteControlTargetPairingCommitArrival::new(
        RemoteControlPairingCommit::new(fixture.prepared().transcript()),
        responder(fixture.link_id, 0x63),
        other_controller,
    );
    assert_eq!(
        state.commit(wrong_identity, ATTEMPT_STARTED_AT),
        CommitRemoteControlTargetPairingOutcome::Rejected {
            rejected: wrong_identity.responder(),
            reason: RemoteControlTargetPairingCommitRejection::ControllerIdentityMismatch {
                expected: fixture.begin.controller().identity_hash(),
                identified: other_controller,
            },
        },
    );
    let wrong_transcript = RemoteControlTargetPairingCommitArrival::new(
        RemoteControlPairingCommit::new(other.prepared().transcript()),
        responder(fixture.link_id, 0x64),
        fixture.begin.controller().identity_hash(),
    );
    assert_eq!(
        state.commit(wrong_transcript, ATTEMPT_STARTED_AT),
        CommitRemoteControlTargetPairingOutcome::Rejected {
            rejected: wrong_transcript.responder(),
            reason: RemoteControlTargetPairingCommitRejection::TranscriptMismatch {
                expected: attempt_id,
                found: wrong_transcript.commit().transcript(),
            },
        },
    );
    assert_eq!(
        state.commit(fixture.commit(0x65), ATTEMPT_STARTED_AT),
        CommitRemoteControlTargetPairingOutcome::AwaitingTargetApproval { attempt_id },
    );
    let duplicate = fixture.commit(0x66);
    assert_eq!(
        state.commit(duplicate, ATTEMPT_STARTED_AT),
        CommitRemoteControlTargetPairingOutcome::Rejected {
            rejected: duplicate.responder(),
            reason: RemoteControlTargetPairingCommitRejection::AlreadyCommitted,
        },
    );
    assert!(matches!(
        state.view(),
        RemoteControlTargetPairingView::AwaitingTargetApproval(attempt)
            if attempt.attempt_id() == attempt_id
    ));
}

#[test]
fn rejection_is_final_only_until_local_approval() {
    let fixture = TargetPairingFixture::new();
    let mut before_commit = RemoteControlTargetPairingState::default();
    let before_commit_id = begin(&mut before_commit, &fixture);
    assert_eq!(
        before_commit.reject(before_commit_id, ATTEMPT_STARTED_AT),
        RejectRemoteControlTargetPairingOutcome::Rejected {
            aborted: RemoteControlTargetPairingAborted::AwaitingBoth {
                attempt_id: before_commit_id,
                context: fixture.context(),
            },
        },
    );
    assert_eq!(before_commit.view(), RemoteControlTargetPairingView::Idle);

    let mut after_commit = RemoteControlTargetPairingState::default();
    let after_commit_id = begin(&mut after_commit, &fixture);
    let commit = fixture.commit(0x66);
    let _awaiting = after_commit.commit(commit, ATTEMPT_STARTED_AT);
    assert_eq!(
        after_commit.reject(after_commit_id, ATTEMPT_STARTED_AT),
        RejectRemoteControlTargetPairingOutcome::Rejected {
            aborted: RemoteControlTargetPairingAborted::AwaitingTargetApproval {
                attempt_id: after_commit_id,
                context: fixture.context(),
                responder: commit.responder(),
            },
        },
    );

    let mut approved = RemoteControlTargetPairingState::default();
    let approved_id = begin(&mut approved, &fixture);
    let _awaiting = approved.approve(approved_id, ATTEMPT_STARTED_AT);
    assert_eq!(
        approved.reject(approved_id, ATTEMPT_STARTED_AT),
        RejectRemoteControlTargetPairingOutcome::AlreadyApproved {
            attempt_id: approved_id,
        },
    );
}

#[test]
fn deadlines_expire_confirmations_and_consume_the_triggering_arrival() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let expired_id = begin(&mut state, &fixture);
    assert_eq!(
        state.expire(InstantMillis(ATTEMPT_EXPIRES_AT.0 - 1)),
        ExpireRemoteControlTargetPairingOutcome::NotDue {
            expires_at: ATTEMPT_EXPIRES_AT,
        },
    );
    assert_eq!(
        state.approve(expired_id, ATTEMPT_EXPIRES_AT),
        ApproveRemoteControlTargetPairingOutcome::Expired {
            expired: RemoteControlTargetPairingAborted::AwaitingBoth {
                attempt_id: expired_id,
                context: fixture.context(),
            },
        },
    );
    assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);

    let first_id = begin(&mut state, &fixture);
    let arrival = fixture.begin_arrival(0x67);
    assert_eq!(
        state.begin(
            &fixture.target_signer,
            &fixture.session,
            &arrival,
            InstantMillis(8_000),
        ),
        BeginRemoteControlTargetPairingOutcome::Expired {
            expired: RemoteControlTargetPairingAborted::AwaitingBoth {
                attempt_id: first_id,
                context: fixture.context(),
            },
        },
    );
    assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
    let arrival = fixture.begin_arrival(0x68);
    assert_eq!(
        state.begin(
            &fixture.target_signer,
            &fixture.session,
            &arrival,
            PAIRING_EXPIRES_AT,
        ),
        BeginRemoteControlTargetPairingOutcome::PairingUnavailable {
            reason: RemoteControlTargetPairingAttemptWindowError::PairingWindowElapsed {
                started_at: PAIRING_EXPIRES_AT,
                pairing_expires_at: PAIRING_EXPIRES_AT,
            },
        },
    );
}

#[test]
fn only_the_attempt_link_can_abort_confirmation() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let attempt_id = begin(&mut state, &fixture);
    assert_eq!(
        state.close_link(LinkId::new([0x99; TRUNCATED_HASH_BYTE_LEN])),
        CloseRemoteControlTargetPairingLinkOutcome::UnrelatedLink,
    );
    assert_eq!(attempt_view(&state).attempt_id(), attempt_id);
    assert_eq!(
        state.close_link(fixture.link_id),
        CloseRemoteControlTargetPairingLinkOutcome::Aborted {
            aborted: RemoteControlTargetPairingAborted::AwaitingBoth {
                attempt_id,
                context: fixture.context(),
            },
        },
    );
    assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
}

#[test]
fn persisted_authorization_retains_one_exact_replayable_completion_until_its_deadline() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let (attempt_id, _grant) = authorize(&mut state, &fixture);
    let other = TargetPairingFixture::with_route(0x75, 0x86);
    let other_id: RemoteControlPairingAttemptId = other.prepared().transcript().into();

    assert_eq!(
        state
            .authorization_persisted(other_id, &fixture.target_signer, AUTHORIZATION_PERSISTED_AT,),
        PersistRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
            settled: other_id,
            active: attempt_id,
        },
    );
    assert_eq!(
        state.authorization_persisted(
            attempt_id,
            &signer(0x53),
            AUTHORIZATION_PERSISTED_AT,
        ),
        PersistRemoteControlTargetPairingAuthorizationOutcome::SigningFailed {
            attempt_id,
            error: crate::remote_control::RemoteControlPairingCompletionSigningError::TargetIdentityMismatch {
                expected: fixture.prepared().transcript().target().identity_hash(),
                found: signer(0x53).identity_hash(),
            },
        },
    );
    let PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed {
        attempt_id: completed_attempt,
        responder: initial_responder,
        completed,
    } = state.authorization_persisted(
        attempt_id,
        &fixture.target_signer,
        AUTHORIZATION_PERSISTED_AT,
    )
    else {
        panic!("completion owed")
    };
    assert_eq!(completed_attempt, attempt_id);
    assert_eq!(initial_responder, fixture.commit(0x61).responder());
    assert_eq!(completed.verify(fixture.prepared().transcript()), Ok(()),);
    let wrong_controller = controller(0x32).identity_hash();
    let rejected = responder(fixture.link_id, 0x62);
    assert_eq!(
        state.commit(
            RemoteControlTargetPairingCommitArrival::new(
                fixture.commit(0x61).commit(),
                rejected,
                wrong_controller,
            ),
            InstantMillis(ATTEMPT_EXPIRES_AT.0 - 1),
        ),
        CommitRemoteControlTargetPairingOutcome::Rejected {
            rejected,
            reason: RemoteControlTargetPairingCommitRejection::ControllerIdentityMismatch {
                expected: fixture.begin.controller().identity_hash(),
                identified: wrong_controller,
            },
        },
    );
    assert_eq!(
        state.authorization_persisted(
            other_id,
            &fixture.target_signer,
            InstantMillis(AUTHORIZATION_PERSISTED_AT.0 + 1),
        ),
        PersistRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
            settled: other_id,
            active: attempt_id,
        },
    );
    assert_eq!(
        state.authorization_persisted(
            attempt_id,
            &fixture.target_signer,
            InstantMillis(AUTHORIZATION_PERSISTED_AT.0 + 1),
        ),
        PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed {
            attempt_id,
            responder: initial_responder,
            completed,
        },
    );
    let retried_responder = fixture.commit(0x62).responder();
    assert_eq!(
        state.commit(
            fixture.commit(0x62),
            InstantMillis(ATTEMPT_EXPIRES_AT.0 - 1),
        ),
        CommitRemoteControlTargetPairingOutcome::CompletionOwed {
            attempt_id,
            responder: retried_responder,
            completed,
        },
    );
    assert!(matches!(
        state.view(),
        RemoteControlTargetPairingView::Completing(attempt)
            if attempt.attempt_id() == attempt_id
    ));
    assert_eq!(
        state.expire(InstantMillis(ATTEMPT_EXPIRES_AT.0 - 1)),
        ExpireRemoteControlTargetPairingOutcome::NotDue {
            expires_at: ATTEMPT_EXPIRES_AT,
        },
    );
    assert_eq!(
        state.expire(ATTEMPT_EXPIRES_AT),
        ExpireRemoteControlTargetPairingOutcome::CompletionRetentionExpired {
            expired: RemoteControlTargetPairingCompletionRetentionExpired::new(
                attempt_id,
                fixture.context(),
            ),
        },
    );
    assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
}

#[test]
fn retained_completion_ends_only_with_its_bound_link() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let (attempt_id, _grant) = authorize(&mut state, &fixture);
    assert!(matches!(
        state.authorization_persisted(
            attempt_id,
            &fixture.target_signer,
            AUTHORIZATION_PERSISTED_AT,
        ),
        PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed { .. },
    ));
    assert_eq!(
        state.close_link(LinkId::new([0xFF; TRUNCATED_HASH_BYTE_LEN])),
        CloseRemoteControlTargetPairingLinkOutcome::UnrelatedLink,
    );
    assert!(matches!(
        state.view(),
        RemoteControlTargetPairingView::Completing(attempt)
            if attempt.attempt_id() == attempt_id
    ));
    assert_eq!(
        state.close_link(fixture.link_id),
        CloseRemoteControlTargetPairingLinkOutcome::CompletionRetentionEnded { attempt_id },
    );
    assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
}

#[test]
fn authorization_persisted_after_the_shared_deadline_returns_the_exact_grant() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let (attempt_id, grant) = authorize(&mut state, &fixture);
    assert_eq!(
        state.authorization_persisted(
            attempt_id,
            &fixture.target_signer,
            ATTEMPT_EXPIRES_AT,
        ),
        PersistRemoteControlTargetPairingAuthorizationOutcome::AuthorizationPersistedAfterDeadline {
            attempt_id,
            context: fixture.context(),
            grant,
        },
    );
    assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
}

#[test]
fn failed_authorization_aborts_only_the_correlated_attempt() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let (attempt_id, _grant) = authorize(&mut state, &fixture);
    let other = TargetPairingFixture::with_route(0x75, 0x86);
    let other_id: RemoteControlPairingAttemptId = other.prepared().transcript().into();

    assert_eq!(
        state.authorization_failed(other_id),
        FailRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
            settled: other_id,
            active: attempt_id,
        },
    );
    assert_eq!(
        state.authorization_failed(attempt_id),
        FailRemoteControlTargetPairingAuthorizationOutcome::Aborted {
            attempt_id,
            context: fixture.context(),
            responder: fixture.commit(0x61).responder(),
        },
    );
    assert_eq!(state.view(), RemoteControlTargetPairingView::Idle);
}

#[test]
fn finalization_is_irrevocable_under_expiry_rejection_and_link_loss() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let (attempt_id, _grant) = authorize(&mut state, &fixture);

    assert_eq!(
        state.expire(InstantMillis(u64::MAX)),
        ExpireRemoteControlTargetPairingOutcome::FinalizationInProgress { attempt_id },
    );
    assert_eq!(
        state.reject(attempt_id, InstantMillis(u64::MAX)),
        RejectRemoteControlTargetPairingOutcome::FinalizationInProgress { attempt_id },
    );
    assert_eq!(
        state.close_link(fixture.link_id),
        CloseRemoteControlTargetPairingLinkOutcome::FinalizationInProgress { attempt_id },
    );
    assert!(matches!(
        state.view(),
        RemoteControlTargetPairingView::Authorizing(attempt)
            if attempt.attempt_id() == attempt_id
    ));
}
