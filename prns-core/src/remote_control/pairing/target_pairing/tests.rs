#![allow(clippy::panic, clippy::unwrap_used)]

use super::*;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::vault::IdentitySecretKey;
use crate::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity, RemoteControlPairingAttemptId,
    RemoteControlPairingAttemptTimeout, RemoteControlPairingBegin, RemoteControlPairingCommit,
    RemoteControlPairingContext, RemoteControlPairingIdentity, RemoteControlPairingOffer,
    RemoteControlPairingPermissions, RemoteControlPairingPreparedOffer,
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
        Self {
            target_signer: signer(0x52),
            session: RemoteControlPairingSession::new(
                RemoteControlPairingIdentity::new(IdentityHash::new(
                    [endpoint_fill; TRUNCATED_HASH_BYTE_LEN],
                )),
                RemoteControlPairingWindow::new(PAIRING_OPENED_AT, PAIRING_EXPIRES_AT).unwrap(),
                permissions(),
                RemoteControlPairingAttemptTimeout::try_from(ATTEMPT_TIMEOUT).unwrap(),
            ),
            link_id: LinkId::new([link_fill; TRUNCATED_HASH_BYTE_LEN]),
            begin: RemoteControlPairingBegin::new(controller(0x31)),
        }
    }

    fn context(&self) -> RemoteControlPairingContext {
        RemoteControlPairingContext::new(self.session.endpoint(), self.link_id)
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
        )
    }

    fn begin_arrival(&self, request_fill: u8) -> RemoteControlTargetPairingBeginArrival {
        RemoteControlTargetPairingBeginArrival::new(
            RemoteControlPairingBegin::new(*self.begin.controller()),
            responder(self.link_id, request_fill),
            self.begin.controller().identity_hash(),
        )
    }
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
    let arrival = fixture.begin_arrival(0x60);
    match state.begin(
        &fixture.target_signer,
        &fixture.session,
        &arrival,
        ATTEMPT_STARTED_AT,
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
        | RemoteControlTargetPairingView::Authorizing(attempt)
        | RemoteControlTargetPairingView::Completing(attempt) => attempt,
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
        RemoteControlPairingBegin::new(*fixture.begin.controller()),
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
        FailRemoteControlTargetPairingOfferDispatchOutcome::Aborted { attempt_id },
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
fn mismatched_attempts_links_transcripts_and_duplicate_commits_never_advance() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let attempt_id = begin(&mut state, &fixture);
    let other = TargetPairingFixture::with_route(0x75, 0x86);
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
    let wrong_transcript = RemoteControlTargetPairingCommitArrival::new(
        RemoteControlPairingCommit::new(other.prepared().transcript()),
        responder(fixture.link_id, 0x63),
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
        state.commit(fixture.commit(0x64), ATTEMPT_STARTED_AT),
        CommitRemoteControlTargetPairingOutcome::AwaitingTargetApproval { attempt_id },
    );
    let duplicate = fixture.commit(0x65);
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
fn completion_does_not_exist_until_the_exact_authorization_is_persisted() {
    let fixture = TargetPairingFixture::new();
    let mut state = RemoteControlTargetPairingState::default();
    let (attempt_id, _grant) = authorize(&mut state, &fixture);
    let other = TargetPairingFixture::with_route(0x75, 0x86);
    let other_id: RemoteControlPairingAttemptId = other.prepared().transcript().into();

    assert_eq!(
        state.authorization_persisted(other_id, &fixture.target_signer),
        PersistRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
            settled: other_id,
            active: attempt_id,
        },
    );
    assert_eq!(
        state.authorization_persisted(attempt_id, &signer(0x53)),
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
        responder,
        completed,
    } = state.authorization_persisted(attempt_id, &fixture.target_signer)
    else {
        panic!("completion owed")
    };
    assert_eq!(completed_attempt, attempt_id);
    assert_eq!(responder, fixture.commit(0x61).responder());
    assert_eq!(completed.verify(fixture.prepared().transcript()), Ok(()),);
    assert_eq!(
        state.complete(other_id),
        CompleteRemoteControlTargetPairingOutcome::AttemptMismatch {
            settled: other_id,
            active: attempt_id,
        },
    );
    assert_eq!(
        state.complete(attempt_id),
        CompleteRemoteControlTargetPairingOutcome::Completed { attempt_id },
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
