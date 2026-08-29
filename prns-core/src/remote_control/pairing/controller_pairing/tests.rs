#![allow(clippy::panic, clippy::unwrap_used)]

use super::*;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::vault::IdentitySecretKey;
use crate::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
use crate::remote_control::{
    ApproveRemoteControlTargetPairingOutcome, BeginRemoteControlTargetPairingOutcome,
    CommitRemoteControlTargetPairingOutcome, CompleteRemoteControlTargetPairingOutcome,
    DispatchRemoteControlTargetPairingOfferOutcome,
    PersistRemoteControlTargetPairingAuthorizationOutcome, RemoteControlControllerIdentity,
    RemoteControlPairingAttemptId, RemoteControlPairingAttemptTimeout, RemoteControlPairingBegin,
    RemoteControlPairingCompleted, RemoteControlPairingContext, RemoteControlPairingIdentity,
    RemoteControlPairingPermissions, RemoteControlPairingPreparedOffer,
    RemoteControlPairingRequest, RemoteControlPairingResponse, RemoteControlPairingSession,
    RemoteControlPairingTranscript, RemoteControlPairingWindow, RemoteControlRequestKind,
    RemoteControlRequestSet, RemoteControlTargetPairingBeginArrival,
    RemoteControlTargetPairingCommitArrival, RemoteControlTargetPairingResponder,
    RemoteControlTargetPairingState, RemoteControlTargetPairingView,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::units::{DurationMillis, InstantMillis};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

const PAIRING_OPENED_AT: InstantMillis = InstantMillis(1_000);
const PAIRING_EXPIRES_AT: InstantMillis = InstantMillis(20_000);
const CONTROLLER_STARTED_AT: InstantMillis = InstantMillis(1_500);
const TARGET_STARTED_AT: InstantMillis = InstantMillis(2_000);
const OFFER_RECEIVED_AT: InstantMillis = InstantMillis(2_100);
const ATTEMPT_TIMEOUT: DurationMillis = DurationMillis(5_000);
const CONTROLLER_ATTEMPT_EXPIRES_AT: InstantMillis = InstantMillis(7_100);

struct PairingFixture {
    controller: RemoteControlControllerIdentity,
    target_signer: InMemoryNodeIdentity,
    session: RemoteControlPairingSession,
    link_id: LinkId,
}

impl PairingFixture {
    fn new() -> Self {
        Self::with_route(0x73, 0x84)
    }

    fn with_route(endpoint_fill: u8, link_fill: u8) -> Self {
        Self {
            controller: controller(0x31),
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
        }
    }

    fn context(&self) -> RemoteControlPairingContext {
        RemoteControlPairingContext::new(self.session.endpoint(), self.link_id)
    }

    fn attempt_timeout(&self) -> RemoteControlPairingAttemptTimeout {
        RemoteControlPairingAttemptTimeout::try_from(ATTEMPT_TIMEOUT).unwrap()
    }

    fn prepared(&self, begin: &RemoteControlPairingBegin) -> RemoteControlPairingPreparedOffer {
        RemoteControlPairingPreparedOffer::new(
            &self.target_signer,
            self.context(),
            begin,
            self.session.permissions().clone(),
            self.attempt_timeout(),
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
    let mut requests = RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
    assert!(requests.insert(RemoteControlRequestKind::AnnounceSelf));
    RemoteControlPairingPermissions::try_from(requests).unwrap()
}

fn responder(link_id: LinkId, request_fill: u8) -> RemoteControlTargetPairingResponder {
    RemoteControlTargetPairingResponder::new(
        link_id,
        RequestId([request_fill; TRUNCATED_HASH_BYTE_LEN]),
    )
}

fn begin_controller(
    state: &mut RemoteControlControllerPairingState,
    fixture: &PairingFixture,
) -> RemoteControlPairingBegin {
    match state.begin(
        fixture.controller,
        fixture.context(),
        CONTROLLER_STARTED_AT,
        PAIRING_EXPIRES_AT,
    ) {
        BeginRemoteControlControllerPairingOutcome::BeginOwed { begin } => begin,
        BeginRemoteControlControllerPairingOutcome::Busy { .. }
        | BeginRemoteControlControllerPairingOutcome::PairingUnavailable { .. } => {
            panic!("fresh controller")
        }
    }
}

fn receive_valid_offer(
    state: &mut RemoteControlControllerPairingState,
    fixture: &PairingFixture,
    begin: &RemoteControlPairingBegin,
) -> (
    RemoteControlPairingAttemptId,
    RemoteControlPairingTranscript,
) {
    let (offer, transcript) = fixture.prepared(begin).into_parts();
    let outcome = state.receive_offer(offer, OFFER_RECEIVED_AT);
    let ReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired { attempt_id } =
        outcome
    else {
        panic!("confirmation required")
    };
    (attempt_id, transcript)
}

fn prepare_completion(
    state: &mut RemoteControlControllerPairingState,
    fixture: &PairingFixture,
) -> (
    RemoteControlPairingAttemptId,
    RemoteControlPairingTranscript,
) {
    let begin = begin_controller(state, fixture);
    let (attempt_id, transcript) = receive_valid_offer(state, fixture, &begin);
    assert!(matches!(
        state.approve(attempt_id, OFFER_RECEIVED_AT),
        ApproveRemoteControlControllerPairingOutcome::CommitOwed {
            attempt_id: committed,
            ..
        } if committed == attempt_id
    ));
    (attempt_id, transcript)
}

fn round_trip_request(request: RemoteControlPairingRequest) -> RemoteControlPairingRequest {
    let mut wire = [0u8; RemoteControlPairingRequest::MAX_ENCODED_LEN];
    let len = request.write_into(&mut wire).unwrap();
    RemoteControlPairingRequest::parse(wire.get(..len).unwrap()).unwrap()
}

fn round_trip_response(response: RemoteControlPairingResponse) -> RemoteControlPairingResponse {
    let mut wire = [0u8; RemoteControlPairingResponse::MAX_ENCODED_LEN];
    let len = response.write_into(&mut wire).unwrap();
    RemoteControlPairingResponse::parse(wire.get(..len).unwrap()).unwrap()
}

#[test]
fn controller_windows_are_positive_and_never_outlive_pairing_availability() {
    assert_eq!(
        RemoteControlControllerPairingWindow::new(PAIRING_EXPIRES_AT, PAIRING_EXPIRES_AT),
        Err(
            RemoteControlControllerPairingWindowError::DeadlineNotFuture {
                started_at: PAIRING_EXPIRES_AT,
                expires_at: PAIRING_EXPIRES_AT,
            }
        ),
    );
    let pairing =
        RemoteControlControllerPairingWindow::new(PAIRING_OPENED_AT, PAIRING_EXPIRES_AT).unwrap();
    let attempt = RemoteControlControllerPairingAttemptWindow::new(
        OFFER_RECEIVED_AT,
        RemoteControlPairingAttemptTimeout::try_from(DurationMillis(30_000)).unwrap(),
        pairing,
    )
    .unwrap();
    assert_eq!(attempt.offered_at(), OFFER_RECEIVED_AT);
    assert_eq!(attempt.expires_at(), PAIRING_EXPIRES_AT);
    assert_eq!(
        RemoteControlControllerPairingAttemptWindow::new(
            PAIRING_EXPIRES_AT,
            RemoteControlPairingAttemptTimeout::try_from(DurationMillis(1)).unwrap(),
            pairing,
        ),
        Err(
            RemoteControlControllerPairingAttemptWindowError::PairingWindowElapsed {
                offered_at: PAIRING_EXPIRES_AT,
                pairing_expires_at: PAIRING_EXPIRES_AT,
            }
        ),
    );
}

#[test]
fn begin_retains_the_exact_context_and_refuses_elapsed_or_competing_pairings() {
    let fixture = PairingFixture::new();
    let mut state = RemoteControlControllerPairingState::default();
    let begin = begin_controller(&mut state, &fixture);
    let RemoteControlControllerPairingView::AwaitingOffer(view) = state.view() else {
        panic!("awaiting offer")
    };

    assert_eq!(begin.controller(), &fixture.controller);
    assert_eq!(view.controller(), &fixture.controller);
    assert_eq!(view.context(), fixture.context());
    assert_eq!(view.window().started_at(), CONTROLLER_STARTED_AT);
    assert_eq!(view.window().expires_at(), PAIRING_EXPIRES_AT);
    assert_eq!(
        state.begin(
            fixture.controller,
            fixture.context(),
            CONTROLLER_STARTED_AT,
            PAIRING_EXPIRES_AT,
        ),
        BeginRemoteControlControllerPairingOutcome::Busy {
            active: RemoteControlControllerPairingActivity::AwaitingOffer,
        },
    );

    let mut elapsed = RemoteControlControllerPairingState::default();
    assert_eq!(
        elapsed.begin(
            fixture.controller,
            fixture.context(),
            PAIRING_EXPIRES_AT,
            PAIRING_EXPIRES_AT,
        ),
        BeginRemoteControlControllerPairingOutcome::PairingUnavailable {
            reason: RemoteControlControllerPairingWindowError::DeadlineNotFuture {
                started_at: PAIRING_EXPIRES_AT,
                expires_at: PAIRING_EXPIRES_AT,
            },
        },
    );
}

#[test]
fn offers_must_authenticate_the_exact_controller_context() {
    let fixture = PairingFixture::new();
    let other = PairingFixture::with_route(0x74, 0x85);
    let mut state = RemoteControlControllerPairingState::default();
    let begin = begin_controller(&mut state, &fixture);
    let (wrong_offer, _) = other.prepared(&begin).into_parts();

    assert_eq!(
        state.receive_offer(wrong_offer, OFFER_RECEIVED_AT),
        ReceiveRemoteControlControllerPairingOfferOutcome::Rejected {
            reason: crate::remote_control::RemoteControlPairingOfferVerificationError::InvalidTargetSignature,
        },
    );
    assert!(matches!(
        state.view(),
        RemoteControlControllerPairingView::AwaitingOffer(_)
    ));

    let (attempt_id, transcript) = receive_valid_offer(&mut state, &fixture, &begin);
    let RemoteControlControllerPairingView::AwaitingApproval(view) = state.view() else {
        panic!("awaiting approval")
    };
    assert_eq!(view.attempt_id(), attempt_id);
    assert_eq!(view.confirmation_code(), transcript.confirmation_code());
    assert_eq!(view.target(), transcript.target());
    assert_eq!(view.permissions(), transcript.permissions());
    assert_eq!(view.window().expires_at(), CONTROLLER_ATTEMPT_EXPIRES_AT);
}

#[test]
fn approval_is_correlated_irrevocable_and_bounded() {
    let fixture = PairingFixture::new();
    let other = PairingFixture::with_route(0x74, 0x85);
    let mut state = RemoteControlControllerPairingState::default();
    let begin = begin_controller(&mut state, &fixture);
    let (attempt_id, _) = receive_valid_offer(&mut state, &fixture, &begin);
    let other_begin = RemoteControlPairingBegin::new(other.controller);
    let other_id: RemoteControlPairingAttemptId = other.prepared(&other_begin).transcript().into();

    assert_eq!(
        state.approve(other_id, OFFER_RECEIVED_AT),
        ApproveRemoteControlControllerPairingOutcome::AttemptMismatch {
            requested: other_id,
            active: attempt_id,
        },
    );
    let ApproveRemoteControlControllerPairingOutcome::CommitOwed {
        attempt_id: committed,
        commit,
    } = state.approve(attempt_id, OFFER_RECEIVED_AT)
    else {
        panic!("commit owed")
    };
    assert_eq!(committed, attempt_id);
    assert_eq!(commit.transcript(), attempt_id.transcript());
    assert_eq!(
        state.reject(attempt_id, OFFER_RECEIVED_AT),
        RejectRemoteControlControllerPairingOutcome::AlreadyApproved { attempt_id },
    );
    assert_eq!(
        state.expire(InstantMillis(CONTROLLER_ATTEMPT_EXPIRES_AT.0 - 1)),
        ExpireRemoteControlControllerPairingOutcome::NotDue {
            expires_at: CONTROLLER_ATTEMPT_EXPIRES_AT,
        },
    );
    assert_eq!(
        state.expire(CONTROLLER_ATTEMPT_EXPIRES_AT),
        ExpireRemoteControlControllerPairingOutcome::Expired {
            aborted: RemoteControlControllerPairingAborted::AwaitingCompletion {
                attempt_id,
                context: fixture.context(),
            },
        },
    );
}

#[test]
fn completed_requires_the_exact_transcript_before_persistence() {
    let fixture = PairingFixture::new();
    let other = PairingFixture::with_route(0x74, 0x85);
    let mut state = RemoteControlControllerPairingState::default();
    let (attempt_id, transcript) = prepare_completion(&mut state, &fixture);
    let other_begin = RemoteControlPairingBegin::new(other.controller);
    let other_transcript = other.prepared(&other_begin).into_parts().1;
    let wrong =
        RemoteControlPairingCompleted::signed_by(&other.target_signer, &other_transcript).unwrap();

    assert_eq!(
        state.receive_completed(wrong, InstantMillis(2_500)),
        ReceiveRemoteControlControllerPairingCompletedOutcome::Rejected {
            attempt_id,
            reason: crate::remote_control::RemoteControlPairingCompletedVerificationError::TranscriptMismatch {
                expected: attempt_id.transcript(),
                found: other_transcript.digest(),
            },
        },
    );
    let completed =
        RemoteControlPairingCompleted::signed_by(&fixture.target_signer, &transcript).unwrap();
    assert_eq!(
        state.receive_completed(completed, InstantMillis(2_500)),
        ReceiveRemoteControlControllerPairingCompletedOutcome::PersistenceOwed { attempt_id },
    );
    let access = state.target_access(attempt_id).unwrap();
    assert_eq!(access.target(), transcript.target());
    assert_eq!(
        access.permitted_requests(),
        transcript.permissions().permitted_requests()
    );
    assert_eq!(access.endpoint(), transcript.target().endpoint());
    assert_eq!(
        state.close_link(fixture.link_id),
        CloseRemoteControlControllerPairingLinkOutcome::PersistenceInProgress { attempt_id },
    );
    assert_eq!(
        state.expire(InstantMillis(u64::MAX)),
        ExpireRemoteControlControllerPairingOutcome::PersistenceInProgress { attempt_id },
    );
    let PersistRemoteControlControllerPairingOutcome::Completed {
        attempt_id: persisted,
        access,
    } = state.persistence_succeeded(attempt_id)
    else {
        panic!("persisted")
    };
    assert_eq!(persisted, attempt_id);
    assert_eq!(access.target(), transcript.target());
    assert_eq!(state.view(), RemoteControlControllerPairingView::Idle);
}

#[test]
fn request_failure_aborts_only_the_exchange_link() {
    let fixture = PairingFixture::new();
    let mut state = RemoteControlControllerPairingState::default();
    let _begin = begin_controller(&mut state, &fixture);

    assert_eq!(
        state.request_failed(LinkId::new([0x99; TRUNCATED_HASH_BYTE_LEN])),
        FailRemoteControlControllerPairingRequestOutcome::UnrelatedLink,
    );
    assert_eq!(
        state.request_failed(fixture.link_id),
        FailRemoteControlControllerPairingRequestOutcome::Aborted {
            aborted: RemoteControlControllerPairingAborted::AwaitingOffer {
                context: fixture.context(),
            },
        },
    );
    assert_eq!(state.view(), RemoteControlControllerPairingView::Idle);

    let mut awaiting_completion = RemoteControlControllerPairingState::default();
    let (attempt_id, _transcript) = prepare_completion(&mut awaiting_completion, &fixture);
    assert_eq!(
        awaiting_completion.request_failed(fixture.link_id),
        FailRemoteControlControllerPairingRequestOutcome::Aborted {
            aborted: RemoteControlControllerPairingAborted::AwaitingCompletion {
                attempt_id,
                context: fixture.context(),
            },
        },
    );
    assert_eq!(
        awaiting_completion.view(),
        RemoteControlControllerPairingView::Idle
    );
}

#[test]
fn rejection_and_persistence_failure_settle_only_the_exact_attempt() {
    let fixture = PairingFixture::new();
    let other = PairingFixture::with_route(0x74, 0x85);
    let other_begin = RemoteControlPairingBegin::new(other.controller);
    let other_id: RemoteControlPairingAttemptId = other.prepared(&other_begin).transcript().into();

    let mut confirming = RemoteControlControllerPairingState::default();
    let begin = begin_controller(&mut confirming, &fixture);
    let (attempt_id, _transcript) = receive_valid_offer(&mut confirming, &fixture, &begin);
    assert_eq!(
        confirming.reject(other_id, OFFER_RECEIVED_AT),
        RejectRemoteControlControllerPairingOutcome::AttemptMismatch {
            requested: other_id,
            active: attempt_id,
        },
    );
    assert_eq!(
        confirming.reject(attempt_id, OFFER_RECEIVED_AT),
        RejectRemoteControlControllerPairingOutcome::Rejected {
            aborted: RemoteControlControllerPairingAborted::AwaitingApproval {
                attempt_id,
                context: fixture.context(),
            },
        },
    );
    assert_eq!(confirming.view(), RemoteControlControllerPairingView::Idle);

    let mut persisting = RemoteControlControllerPairingState::default();
    let (attempt_id, transcript) = prepare_completion(&mut persisting, &fixture);
    let completed =
        RemoteControlPairingCompleted::signed_by(&fixture.target_signer, &transcript).unwrap();
    assert_eq!(
        persisting.receive_completed(completed, InstantMillis(2_500)),
        ReceiveRemoteControlControllerPairingCompletedOutcome::PersistenceOwed { attempt_id },
    );
    assert_eq!(persisting.target_access(other_id), None);
    assert_eq!(
        persisting.persistence_succeeded(other_id),
        PersistRemoteControlControllerPairingOutcome::AttemptMismatch {
            persisted: other_id,
            active: attempt_id,
        },
    );
    let FailRemoteControlControllerPairingPersistenceOutcome::Failed {
        attempt_id: failed,
        access,
    } = persisting.persistence_failed(attempt_id)
    else {
        panic!("persistence failure")
    };
    assert_eq!(failed, attempt_id);
    assert_eq!(access.target(), transcript.target());
    assert_eq!(persisting.view(), RemoteControlControllerPairingView::Idle);
}

#[test]
fn real_wire_messages_drive_both_reducers_to_the_same_durable_pairing() {
    let fixture = PairingFixture::new();
    let mut controller_state = RemoteControlControllerPairingState::default();
    let mut target_state = RemoteControlTargetPairingState::default();

    let begin = begin_controller(&mut controller_state, &fixture);
    let begin = match round_trip_request(RemoteControlPairingRequest::Begin(begin)) {
        RemoteControlPairingRequest::Begin(begin) => begin,
        RemoteControlPairingRequest::Commit(_) => panic!("begin request"),
    };
    let begin_responder = responder(fixture.link_id, 0x60);
    let begin_arrival = RemoteControlTargetPairingBeginArrival::new(
        begin,
        begin_responder,
        fixture.controller.identity_hash(),
    );
    let BeginRemoteControlTargetPairingOutcome::OfferPrepared {
        attempt_id: target_attempt_id,
        responder: offered_to,
        offer,
    } = target_state.begin(
        &fixture.target_signer,
        &fixture.session,
        &begin_arrival,
        TARGET_STARTED_AT,
    )
    else {
        panic!("offer owed")
    };
    assert_eq!(offered_to, begin_responder);
    let DispatchRemoteControlTargetPairingOfferOutcome::AwaitingConfirmation { attempt } =
        target_state.offer_dispatched(target_attempt_id)
    else {
        panic!("prepared target offer")
    };
    assert_eq!(attempt.attempt_id(), target_attempt_id);

    let offer = match round_trip_response(RemoteControlPairingResponse::Offer(offer)) {
        RemoteControlPairingResponse::Offer(offer) => offer,
        RemoteControlPairingResponse::Completed(_) => panic!("offer response"),
    };
    assert_eq!(offer.attempt_timeout().duration(), ATTEMPT_TIMEOUT);
    let ReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired {
        attempt_id: controller_attempt_id,
    } = controller_state.receive_offer(offer, OFFER_RECEIVED_AT)
    else {
        panic!("confirmation required")
    };
    assert_eq!(controller_attempt_id, target_attempt_id);

    let (target_identity, target_endpoint, permitted_requests) = {
        let RemoteControlControllerPairingView::AwaitingApproval(controller_view) =
            controller_state.view()
        else {
            panic!("controller confirmation")
        };
        let RemoteControlTargetPairingView::AwaitingBoth(target_view) = target_state.view() else {
            panic!("target confirmation")
        };
        assert_eq!(controller_view.attempt_id(), target_view.attempt_id());
        assert_eq!(
            controller_view.confirmation_code(),
            target_view.confirmation_code()
        );
        assert_eq!(controller_view.controller(), target_view.controller());
        assert_eq!(controller_view.target(), target_view.target());
        assert_eq!(controller_view.permissions(), target_view.permissions());
        (
            controller_view.target().identity_hash(),
            controller_view.target().endpoint(),
            *controller_view.permissions().permitted_requests(),
        )
    };

    assert_eq!(
        target_state.approve(target_attempt_id, InstantMillis(2_200)),
        ApproveRemoteControlTargetPairingOutcome::AwaitingControllerCommit {
            attempt_id: target_attempt_id,
        },
    );
    let ApproveRemoteControlControllerPairingOutcome::CommitOwed {
        attempt_id: committed,
        commit,
    } = controller_state.approve(controller_attempt_id, InstantMillis(2_200))
    else {
        panic!("commit owed")
    };
    assert_eq!(committed, target_attempt_id);
    let commit = match round_trip_request(RemoteControlPairingRequest::Commit(commit)) {
        RemoteControlPairingRequest::Commit(commit) => commit,
        RemoteControlPairingRequest::Begin(_) => panic!("commit request"),
    };
    let commit_responder = responder(fixture.link_id, 0x61);
    let arrival = RemoteControlTargetPairingCommitArrival::new(
        commit,
        commit_responder,
        fixture.controller.identity_hash(),
    );
    let CommitRemoteControlTargetPairingOutcome::AuthorizationOwed {
        attempt_id: authorized,
        grant,
    } = target_state.commit(arrival, InstantMillis(2_300))
    else {
        panic!("authorization owed")
    };
    assert_eq!(authorized, controller_attempt_id);
    assert_eq!(grant.controller(), &fixture.controller);
    assert_eq!(
        grant.permitted_requests(),
        fixture.session.permissions().permitted_requests()
    );

    let PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed {
        attempt_id: completed_attempt,
        responder: completed_to,
        completed,
    } = target_state.authorization_persisted(authorized, &fixture.target_signer)
    else {
        panic!("completion owed")
    };
    assert_eq!(completed_attempt, controller_attempt_id);
    assert_eq!(completed_to, commit_responder);
    let completed = match round_trip_response(RemoteControlPairingResponse::Completed(completed)) {
        RemoteControlPairingResponse::Completed(completed) => completed,
        RemoteControlPairingResponse::Offer(_) => panic!("completed response"),
    };
    assert_eq!(completed.transcript(), controller_attempt_id.transcript());
    assert_eq!(
        controller_state.receive_completed(completed, InstantMillis(2_400)),
        ReceiveRemoteControlControllerPairingCompletedOutcome::PersistenceOwed {
            attempt_id: controller_attempt_id,
        },
    );
    assert_eq!(
        target_state.complete(target_attempt_id),
        CompleteRemoteControlTargetPairingOutcome::Completed {
            attempt_id: target_attempt_id,
        },
    );
    let access = controller_state
        .target_access(controller_attempt_id)
        .unwrap();
    assert_eq!(access.target().identity_hash(), target_identity);
    assert_eq!(access.endpoint(), target_endpoint);
    assert_eq!(access.permitted_requests(), &permitted_requests);
    assert_eq!(access.permitted_requests(), grant.permitted_requests());
    let PersistRemoteControlControllerPairingOutcome::Completed {
        attempt_id: persisted,
        access,
    } = controller_state.persistence_succeeded(controller_attempt_id)
    else {
        panic!("controller persistence")
    };
    assert_eq!(persisted, target_attempt_id);
    assert_eq!(access.target().identity_hash(), target_identity);
    assert_eq!(
        controller_state.view(),
        RemoteControlControllerPairingView::Idle
    );
    assert_eq!(target_state.view(), RemoteControlTargetPairingView::Idle);
}
