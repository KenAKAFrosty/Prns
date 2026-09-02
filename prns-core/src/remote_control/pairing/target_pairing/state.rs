use crate::identity::IdentitySigner;
use crate::remote_control::RemoteControlControllerGrant;
use crate::routing::links::LinkId;
use crate::units::InstantMillis;

use super::super::{
    RemoteControlPairingAttemptId, RemoteControlPairingCompleted, RemoteControlPairingContext,
    RemoteControlPairingInvitationProofInvalid, RemoteControlPairingPreparedOffer,
    RemoteControlPairingSession, RemoteControlPairingTranscript,
};
use super::model::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteControlTargetPairingConfirmationProgress {
    Both,
    TargetApproval {
        responder: RemoteControlTargetPairingResponder,
    },
    ControllerCommit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteControlTargetPairingActiveStage {
    OfferPrepared,
    Confirming(RemoteControlTargetPairingConfirmationProgress),
}

#[derive(Debug, PartialEq, Eq)]
struct RemoteControlTargetPairingAttempt {
    transcript: RemoteControlPairingTranscript,
    window: RemoteControlTargetPairingAttemptWindow,
}

impl RemoteControlTargetPairingAttempt {
    fn view(&self) -> RemoteControlTargetPairingAttemptView<'_> {
        RemoteControlTargetPairingAttemptView {
            transcript: &self.transcript,
            window: self.window,
        }
    }

    fn attempt_id(&self) -> RemoteControlPairingAttemptId {
        (&self.transcript).into()
    }

    fn grant(&self) -> RemoteControlControllerGrant {
        (self.transcript.controller(), self.transcript.permissions()).into()
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
enum RemoteControlTargetPairingPhase {
    #[default]
    Idle,
    Active {
        attempt: RemoteControlTargetPairingAttempt,
        stage: RemoteControlTargetPairingActiveStage,
    },
    Authorizing {
        attempt: RemoteControlTargetPairingAttempt,
        responder: RemoteControlTargetPairingResponder,
    },
    Completing {
        attempt: RemoteControlTargetPairingAttempt,
        responder: RemoteControlTargetPairingResponder,
        completed: RemoteControlPairingCompleted,
    },
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RemoteControlTargetPairingState {
    phase: RemoteControlTargetPairingPhase,
}

impl RemoteControlTargetPairingState {
    #[must_use]
    pub fn view(&self) -> RemoteControlTargetPairingView<'_> {
        match &self.phase {
            RemoteControlTargetPairingPhase::Idle => RemoteControlTargetPairingView::Idle,
            RemoteControlTargetPairingPhase::Active { attempt, stage } => match stage {
                RemoteControlTargetPairingActiveStage::OfferPrepared => {
                    RemoteControlTargetPairingView::OfferPrepared(attempt.view())
                }
                RemoteControlTargetPairingActiveStage::Confirming(progress) => match progress {
                    RemoteControlTargetPairingConfirmationProgress::Both => {
                        RemoteControlTargetPairingView::AwaitingBoth(attempt.view())
                    }
                    RemoteControlTargetPairingConfirmationProgress::TargetApproval { .. } => {
                        RemoteControlTargetPairingView::AwaitingTargetApproval(attempt.view())
                    }
                    RemoteControlTargetPairingConfirmationProgress::ControllerCommit => {
                        RemoteControlTargetPairingView::AwaitingControllerCommit(attempt.view())
                    }
                },
            },
            RemoteControlTargetPairingPhase::Authorizing { attempt, .. } => {
                RemoteControlTargetPairingView::Authorizing(attempt.view())
            }
            RemoteControlTargetPairingPhase::Completing { attempt, .. } => {
                RemoteControlTargetPairingView::Completing(attempt.view())
            }
        }
    }

    pub fn begin(
        &mut self,
        target_signer: &impl IdentitySigner,
        session: &RemoteControlPairingSession,
        arrival: &RemoteControlTargetPairingBeginArrival,
        started_at: InstantMillis,
    ) -> BeginRemoteControlTargetPairingOutcome {
        if let Some(expired) = self.take_expired_completion(started_at) {
            return BeginRemoteControlTargetPairingOutcome::CompletionRetentionExpired { expired };
        }
        let claimed = arrival.begin.controller().identity_hash();
        if claimed != arrival.identified_controller {
            return BeginRemoteControlTargetPairingOutcome::Rejected {
                rejected: arrival.responder,
                reason: RemoteControlTargetPairingBeginRejection::ControllerIdentityMismatch {
                    claimed,
                    identified: arrival.identified_controller,
                },
            };
        }
        match session.invitation_verifier().verify(
            session.endpoint(),
            arrival.begin.controller(),
            arrival.begin.invitation_proof(),
        ) {
            Ok(()) => {}
            Err(RemoteControlPairingInvitationProofInvalid) => {
                return BeginRemoteControlTargetPairingOutcome::Rejected {
                    rejected: arrival.responder,
                    reason: RemoteControlTargetPairingBeginRejection::InvalidInvitationProof,
                };
            }
        }
        if let Some(expired) = self.take_expired_attempt(started_at) {
            return BeginRemoteControlTargetPairingOutcome::Expired { expired };
        }
        if let Some(active) = self.active_attempt_id() {
            return BeginRemoteControlTargetPairingOutcome::Busy { active };
        }
        let attempt_timeout = session.attempt_timeout();
        let window = match RemoteControlTargetPairingAttemptWindow::new(
            started_at,
            attempt_timeout,
            session.window(),
        ) {
            Ok(window) => window,
            Err(reason) => {
                return BeginRemoteControlTargetPairingOutcome::PairingUnavailable { reason }
            }
        };
        let prepared = RemoteControlPairingPreparedOffer::new(
            target_signer,
            RemoteControlPairingContext::new(session.endpoint(), arrival.responder.link_id()),
            &arrival.begin,
            session.permissions().clone(),
            attempt_timeout,
        );
        let (offer, transcript) = prepared.into_parts();
        let attempt = RemoteControlTargetPairingAttempt { transcript, window };
        let attempt_id = attempt.attempt_id();
        self.phase = RemoteControlTargetPairingPhase::Active {
            attempt,
            stage: RemoteControlTargetPairingActiveStage::OfferPrepared,
        };
        BeginRemoteControlTargetPairingOutcome::OfferPrepared {
            attempt_id,
            responder: arrival.responder,
            offer,
        }
    }

    pub fn offer_dispatched(
        &mut self,
        dispatched: RemoteControlPairingAttemptId,
    ) -> DispatchRemoteControlTargetPairingOfferOutcome<'_> {
        match &mut self.phase {
            RemoteControlTargetPairingPhase::Active {
                attempt,
                stage: stage @ RemoteControlTargetPairingActiveStage::OfferPrepared,
            } => {
                let active = attempt.attempt_id();
                if dispatched != active {
                    return DispatchRemoteControlTargetPairingOfferOutcome::AttemptMismatch {
                        dispatched,
                        active,
                    };
                }
                *stage = RemoteControlTargetPairingActiveStage::Confirming(
                    RemoteControlTargetPairingConfirmationProgress::Both,
                );
                DispatchRemoteControlTargetPairingOfferOutcome::AwaitingConfirmation {
                    attempt: attempt.view(),
                }
            }
            RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Active { .. }
            | RemoteControlTargetPairingPhase::Authorizing { .. }
            | RemoteControlTargetPairingPhase::Completing { .. } => {
                DispatchRemoteControlTargetPairingOfferOutcome::NoOfferPrepared
            }
        }
    }

    pub fn offer_dispatch_failed(
        &mut self,
        failed: RemoteControlPairingAttemptId,
    ) -> FailRemoteControlTargetPairingOfferDispatchOutcome {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Active {
                attempt,
                stage: RemoteControlTargetPairingActiveStage::OfferPrepared,
            } => {
                let active = attempt.attempt_id();
                if failed == active {
                    let context = attempt.view().context();
                    return FailRemoteControlTargetPairingOfferDispatchOutcome::Aborted {
                        attempt_id: active,
                        context,
                    };
                }
                self.phase = RemoteControlTargetPairingPhase::Active {
                    attempt,
                    stage: RemoteControlTargetPairingActiveStage::OfferPrepared,
                };
                FailRemoteControlTargetPairingOfferDispatchOutcome::AttemptMismatch {
                    failed,
                    active,
                }
            }
            phase @ (RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Active { .. }
            | RemoteControlTargetPairingPhase::Authorizing { .. }
            | RemoteControlTargetPairingPhase::Completing { .. }) => {
                self.phase = phase;
                FailRemoteControlTargetPairingOfferDispatchOutcome::NoOfferPrepared
            }
        }
    }

    pub fn approve(
        &mut self,
        requested: RemoteControlPairingAttemptId,
        now: InstantMillis,
    ) -> ApproveRemoteControlTargetPairingOutcome {
        if let Some(expired) = self.take_expired_completion(now) {
            return ApproveRemoteControlTargetPairingOutcome::CompletionRetentionExpired {
                expired,
            };
        }
        if let Some(expired) = self.take_expired_attempt(now) {
            return ApproveRemoteControlTargetPairingOutcome::Expired { expired };
        }
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Idle => {
                ApproveRemoteControlTargetPairingOutcome::NoActiveAttempt
            }
            RemoteControlTargetPairingPhase::Active {
                attempt,
                stage: RemoteControlTargetPairingActiveStage::OfferPrepared,
            } => {
                let active = attempt.attempt_id();
                self.phase = RemoteControlTargetPairingPhase::Active {
                    attempt,
                    stage: RemoteControlTargetPairingActiveStage::OfferPrepared,
                };
                if requested != active {
                    ApproveRemoteControlTargetPairingOutcome::AttemptMismatch { requested, active }
                } else {
                    ApproveRemoteControlTargetPairingOutcome::OfferPendingDispatch {
                        attempt_id: active,
                    }
                }
            }
            RemoteControlTargetPairingPhase::Active {
                attempt,
                stage: RemoteControlTargetPairingActiveStage::Confirming(progress),
            } => {
                let active = attempt.attempt_id();
                if requested != active {
                    self.phase = RemoteControlTargetPairingPhase::Active {
                        attempt,
                        stage: RemoteControlTargetPairingActiveStage::Confirming(progress),
                    };
                    return ApproveRemoteControlTargetPairingOutcome::AttemptMismatch {
                        requested,
                        active,
                    };
                }
                match progress {
                    RemoteControlTargetPairingConfirmationProgress::Both => {
                        self.phase = RemoteControlTargetPairingPhase::Active {
                            attempt,
                            stage: RemoteControlTargetPairingActiveStage::Confirming(
                                RemoteControlTargetPairingConfirmationProgress::ControllerCommit,
                            ),
                        };
                        ApproveRemoteControlTargetPairingOutcome::AwaitingControllerCommit {
                            attempt_id: active,
                        }
                    }
                    RemoteControlTargetPairingConfirmationProgress::TargetApproval {
                        responder,
                    } => {
                        let grant = attempt.grant();
                        self.phase =
                            RemoteControlTargetPairingPhase::Authorizing { attempt, responder };
                        ApproveRemoteControlTargetPairingOutcome::AuthorizationOwed {
                            attempt_id: active,
                            grant,
                        }
                    }
                    RemoteControlTargetPairingConfirmationProgress::ControllerCommit => {
                        self.phase = RemoteControlTargetPairingPhase::Active {
                            attempt,
                            stage: RemoteControlTargetPairingActiveStage::Confirming(progress),
                        };
                        ApproveRemoteControlTargetPairingOutcome::AlreadyApproved {
                            attempt_id: active,
                        }
                    }
                }
            }
            RemoteControlTargetPairingPhase::Authorizing { attempt, responder } => {
                let attempt_id = attempt.attempt_id();
                self.phase = RemoteControlTargetPairingPhase::Authorizing { attempt, responder };
                ApproveRemoteControlTargetPairingOutcome::FinalizationInProgress { attempt_id }
            }
            RemoteControlTargetPairingPhase::Completing {
                attempt,
                responder,
                completed,
            } => {
                let attempt_id = attempt.attempt_id();
                self.phase = RemoteControlTargetPairingPhase::Completing {
                    attempt,
                    responder,
                    completed,
                };
                ApproveRemoteControlTargetPairingOutcome::FinalizationInProgress { attempt_id }
            }
        }
    }

    pub fn commit(
        &mut self,
        arrival: RemoteControlTargetPairingCommitArrival,
        now: InstantMillis,
    ) -> CommitRemoteControlTargetPairingOutcome {
        if let Some(expired) = self.take_expired_completion(now) {
            return CommitRemoteControlTargetPairingOutcome::CompletionRetentionExpired {
                expired,
                rejected: arrival.responder,
            };
        }
        if let Some(expired) = self.take_expired_attempt(now) {
            return CommitRemoteControlTargetPairingOutcome::Expired {
                expired,
                rejected: arrival.responder,
            };
        }
        let Some(attempt) = self.attempt() else {
            return CommitRemoteControlTargetPairingOutcome::Rejected {
                rejected: arrival.responder,
                reason: RemoteControlTargetPairingCommitRejection::NoActiveAttempt,
            };
        };
        if let Err(reason) = validate_commit(attempt, arrival) {
            return CommitRemoteControlTargetPairingOutcome::Rejected {
                rejected: arrival.responder,
                reason,
            };
        }
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Active {
                attempt,
                stage: RemoteControlTargetPairingActiveStage::OfferPrepared,
            } => {
                self.phase = RemoteControlTargetPairingPhase::Active {
                    attempt,
                    stage: RemoteControlTargetPairingActiveStage::OfferPrepared,
                };
                CommitRemoteControlTargetPairingOutcome::Rejected {
                    rejected: arrival.responder,
                    reason: RemoteControlTargetPairingCommitRejection::OfferPendingDispatch,
                }
            }
            RemoteControlTargetPairingPhase::Active {
                attempt,
                stage: RemoteControlTargetPairingActiveStage::Confirming(progress),
            } => {
                let attempt_id = attempt.attempt_id();
                match progress {
                    RemoteControlTargetPairingConfirmationProgress::Both => {
                        self.phase = RemoteControlTargetPairingPhase::Active {
                            attempt,
                            stage: RemoteControlTargetPairingActiveStage::Confirming(
                                RemoteControlTargetPairingConfirmationProgress::TargetApproval {
                                    responder: arrival.responder,
                                },
                            ),
                        };
                        CommitRemoteControlTargetPairingOutcome::AwaitingTargetApproval {
                            attempt_id,
                        }
                    }
                    RemoteControlTargetPairingConfirmationProgress::ControllerCommit => {
                        let grant = attempt.grant();
                        self.phase = RemoteControlTargetPairingPhase::Authorizing {
                            attempt,
                            responder: arrival.responder,
                        };
                        CommitRemoteControlTargetPairingOutcome::AuthorizationOwed {
                            attempt_id,
                            grant,
                        }
                    }
                    RemoteControlTargetPairingConfirmationProgress::TargetApproval { .. } => {
                        self.phase = RemoteControlTargetPairingPhase::Active {
                            attempt,
                            stage: RemoteControlTargetPairingActiveStage::Confirming(progress),
                        };
                        CommitRemoteControlTargetPairingOutcome::Rejected {
                            rejected: arrival.responder,
                            reason: RemoteControlTargetPairingCommitRejection::AlreadyCommitted,
                        }
                    }
                }
            }
            RemoteControlTargetPairingPhase::Authorizing { attempt, responder } => {
                self.phase = RemoteControlTargetPairingPhase::Authorizing { attempt, responder };
                CommitRemoteControlTargetPairingOutcome::Rejected {
                    rejected: arrival.responder,
                    reason: RemoteControlTargetPairingCommitRejection::FinalizationInProgress,
                }
            }
            RemoteControlTargetPairingPhase::Completing {
                attempt,
                responder,
                completed,
            } => {
                let attempt_id = attempt.attempt_id();
                self.phase = RemoteControlTargetPairingPhase::Completing {
                    attempt,
                    responder,
                    completed,
                };
                CommitRemoteControlTargetPairingOutcome::CompletionOwed {
                    attempt_id,
                    responder: arrival.responder,
                    completed,
                }
            }
            RemoteControlTargetPairingPhase::Idle => {
                CommitRemoteControlTargetPairingOutcome::Rejected {
                    rejected: arrival.responder,
                    reason: RemoteControlTargetPairingCommitRejection::NoActiveAttempt,
                }
            }
        }
    }

    pub fn reject(
        &mut self,
        requested: RemoteControlPairingAttemptId,
        now: InstantMillis,
    ) -> RejectRemoteControlTargetPairingOutcome {
        if let Some(expired) = self.take_expired_completion(now) {
            return RejectRemoteControlTargetPairingOutcome::CompletionRetentionExpired { expired };
        }
        if let Some(expired) = self.take_expired_attempt(now) {
            return RejectRemoteControlTargetPairingOutcome::Expired { expired };
        }
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Idle => {
                RejectRemoteControlTargetPairingOutcome::NoActiveAttempt
            }
            RemoteControlTargetPairingPhase::Active {
                attempt,
                stage: RemoteControlTargetPairingActiveStage::OfferPrepared,
            } => {
                let active = attempt.attempt_id();
                self.phase = RemoteControlTargetPairingPhase::Active {
                    attempt,
                    stage: RemoteControlTargetPairingActiveStage::OfferPrepared,
                };
                if requested != active {
                    RejectRemoteControlTargetPairingOutcome::AttemptMismatch { requested, active }
                } else {
                    RejectRemoteControlTargetPairingOutcome::OfferPendingDispatch {
                        attempt_id: active,
                    }
                }
            }
            RemoteControlTargetPairingPhase::Active {
                attempt,
                stage: RemoteControlTargetPairingActiveStage::Confirming(progress),
            } => {
                let active = attempt.attempt_id();
                if requested != active {
                    self.phase = RemoteControlTargetPairingPhase::Active {
                        attempt,
                        stage: RemoteControlTargetPairingActiveStage::Confirming(progress),
                    };
                    return RejectRemoteControlTargetPairingOutcome::AttemptMismatch {
                        requested,
                        active,
                    };
                }
                match progress {
                    RemoteControlTargetPairingConfirmationProgress::Both
                    | RemoteControlTargetPairingConfirmationProgress::TargetApproval { .. } => {
                        RejectRemoteControlTargetPairingOutcome::Rejected {
                            aborted: abort_active(
                                attempt,
                                RemoteControlTargetPairingActiveStage::Confirming(progress),
                            ),
                        }
                    }
                    RemoteControlTargetPairingConfirmationProgress::ControllerCommit => {
                        self.phase = RemoteControlTargetPairingPhase::Active {
                            attempt,
                            stage: RemoteControlTargetPairingActiveStage::Confirming(progress),
                        };
                        RejectRemoteControlTargetPairingOutcome::AlreadyApproved {
                            attempt_id: active,
                        }
                    }
                }
            }
            RemoteControlTargetPairingPhase::Authorizing { attempt, responder } => {
                let attempt_id = attempt.attempt_id();
                self.phase = RemoteControlTargetPairingPhase::Authorizing { attempt, responder };
                RejectRemoteControlTargetPairingOutcome::FinalizationInProgress { attempt_id }
            }
            RemoteControlTargetPairingPhase::Completing {
                attempt,
                responder,
                completed,
            } => {
                let attempt_id = attempt.attempt_id();
                self.phase = RemoteControlTargetPairingPhase::Completing {
                    attempt,
                    responder,
                    completed,
                };
                RejectRemoteControlTargetPairingOutcome::FinalizationInProgress { attempt_id }
            }
        }
    }

    pub fn authorization_persisted(
        &mut self,
        settled: RemoteControlPairingAttemptId,
        target_signer: &impl IdentitySigner,
        now: InstantMillis,
    ) -> PersistRemoteControlTargetPairingAuthorizationOutcome {
        if let Some(expired) = self.take_expired_completion(now) {
            return PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionRetentionExpired {
                expired,
            };
        }
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Authorizing { attempt, responder } => {
                let active = attempt.attempt_id();
                if settled != active {
                    self.phase =
                        RemoteControlTargetPairingPhase::Authorizing { attempt, responder };
                    return PersistRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
                        settled,
                        active,
                    };
                }
                if now >= attempt.window.expires_at() {
                    return PersistRemoteControlTargetPairingAuthorizationOutcome::AuthorizationPersistedAfterDeadline {
                        attempt_id: active,
                        context: attempt.view().context(),
                        grant: attempt.grant(),
                    };
                }
                match RemoteControlPairingCompleted::signed_by(target_signer, &attempt.transcript) {
                    Ok(completed) => {
                        self.phase = RemoteControlTargetPairingPhase::Completing {
                            attempt,
                            responder,
                            completed,
                        };
                        PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed {
                            attempt_id: active,
                            responder,
                            completed,
                        }
                    }
                    Err(error) => {
                        self.phase =
                            RemoteControlTargetPairingPhase::Authorizing { attempt, responder };
                        PersistRemoteControlTargetPairingAuthorizationOutcome::SigningFailed {
                            attempt_id: active,
                            error,
                        }
                    }
                }
            }
            RemoteControlTargetPairingPhase::Completing {
                attempt,
                responder,
                completed,
            } => {
                let active = attempt.attempt_id();
                self.phase = RemoteControlTargetPairingPhase::Completing {
                    attempt,
                    responder,
                    completed,
                };
                if settled != active {
                    return PersistRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
                        settled,
                        active,
                    };
                }
                PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed {
                    attempt_id: active,
                    responder,
                    completed,
                }
            }
            phase @ (RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Active { .. }) => {
                self.phase = phase;
                PersistRemoteControlTargetPairingAuthorizationOutcome::NoAuthorizationOwed
            }
        }
    }

    pub fn authorization_failed(
        &mut self,
        settled: RemoteControlPairingAttemptId,
    ) -> FailRemoteControlTargetPairingAuthorizationOutcome {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Authorizing { attempt, responder } => {
                let active = attempt.attempt_id();
                if settled != active {
                    self.phase =
                        RemoteControlTargetPairingPhase::Authorizing { attempt, responder };
                    return FailRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
                        settled,
                        active,
                    };
                }
                FailRemoteControlTargetPairingAuthorizationOutcome::Aborted {
                    attempt_id: active,
                    context: attempt.view().context(),
                    responder,
                }
            }
            phase @ (RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Active { .. }
            | RemoteControlTargetPairingPhase::Completing { .. }) => {
                self.phase = phase;
                FailRemoteControlTargetPairingAuthorizationOutcome::NoAuthorizationOwed
            }
        }
    }

    pub fn expire(&mut self, now: InstantMillis) -> ExpireRemoteControlTargetPairingOutcome {
        if let Some(expired) = self.take_expired_completion(now) {
            return ExpireRemoteControlTargetPairingOutcome::CompletionRetentionExpired { expired };
        }
        if let Some(aborted) = self.take_expired_attempt(now) {
            return ExpireRemoteControlTargetPairingOutcome::Expired { aborted };
        }
        match self.view() {
            RemoteControlTargetPairingView::Idle => {
                ExpireRemoteControlTargetPairingOutcome::NoActiveAttempt
            }
            RemoteControlTargetPairingView::OfferPrepared(attempt)
            | RemoteControlTargetPairingView::AwaitingBoth(attempt)
            | RemoteControlTargetPairingView::AwaitingTargetApproval(attempt)
            | RemoteControlTargetPairingView::AwaitingControllerCommit(attempt) => {
                ExpireRemoteControlTargetPairingOutcome::NotDue {
                    expires_at: attempt.window().expires_at(),
                }
            }
            RemoteControlTargetPairingView::Authorizing(attempt) => {
                ExpireRemoteControlTargetPairingOutcome::FinalizationInProgress {
                    attempt_id: attempt.attempt_id(),
                }
            }
            RemoteControlTargetPairingView::Completing(attempt) => {
                ExpireRemoteControlTargetPairingOutcome::NotDue {
                    expires_at: attempt.window().expires_at(),
                }
            }
        }
    }

    pub fn close_link(&mut self, link_id: LinkId) -> CloseRemoteControlTargetPairingLinkOutcome {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Idle => {
                CloseRemoteControlTargetPairingLinkOutcome::NoActiveAttempt
            }
            RemoteControlTargetPairingPhase::Active { attempt, stage }
                if attempt.view().context().link_id() == link_id =>
            {
                CloseRemoteControlTargetPairingLinkOutcome::Aborted {
                    aborted: abort_active(attempt, stage),
                }
            }
            RemoteControlTargetPairingPhase::Active { attempt, stage } => {
                self.phase = RemoteControlTargetPairingPhase::Active { attempt, stage };
                CloseRemoteControlTargetPairingLinkOutcome::UnrelatedLink
            }
            RemoteControlTargetPairingPhase::Authorizing { attempt, responder } => {
                let attempt_id = attempt.attempt_id();
                let related = attempt.view().context().link_id() == link_id;
                self.phase = RemoteControlTargetPairingPhase::Authorizing { attempt, responder };
                if related {
                    CloseRemoteControlTargetPairingLinkOutcome::FinalizationInProgress {
                        attempt_id,
                    }
                } else {
                    CloseRemoteControlTargetPairingLinkOutcome::UnrelatedLink
                }
            }
            RemoteControlTargetPairingPhase::Completing {
                attempt,
                responder,
                completed,
            } => {
                let attempt_id = attempt.attempt_id();
                let related = attempt.view().context().link_id() == link_id;
                if related {
                    CloseRemoteControlTargetPairingLinkOutcome::CompletionRetentionEnded {
                        attempt_id,
                    }
                } else {
                    self.phase = RemoteControlTargetPairingPhase::Completing {
                        attempt,
                        responder,
                        completed,
                    };
                    CloseRemoteControlTargetPairingLinkOutcome::UnrelatedLink
                }
            }
        }
    }

    fn attempt(&self) -> Option<&RemoteControlTargetPairingAttempt> {
        match &self.phase {
            RemoteControlTargetPairingPhase::Idle => None,
            RemoteControlTargetPairingPhase::Active { attempt, .. }
            | RemoteControlTargetPairingPhase::Authorizing { attempt, .. }
            | RemoteControlTargetPairingPhase::Completing { attempt, .. } => Some(attempt),
        }
    }

    fn active_attempt_id(&self) -> Option<RemoteControlPairingAttemptId> {
        self.attempt()
            .map(RemoteControlTargetPairingAttempt::attempt_id)
    }

    fn take_expired_attempt(
        &mut self,
        now: InstantMillis,
    ) -> Option<RemoteControlTargetPairingAborted> {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Active { attempt, stage }
                if now >= attempt.window.expires_at() =>
            {
                Some(abort_active(attempt, stage))
            }
            phase @ (RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Active { .. }
            | RemoteControlTargetPairingPhase::Authorizing { .. }
            | RemoteControlTargetPairingPhase::Completing { .. }) => {
                self.phase = phase;
                None
            }
        }
    }

    fn take_expired_completion(
        &mut self,
        now: InstantMillis,
    ) -> Option<RemoteControlTargetPairingCompletionRetentionExpired> {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Completing { attempt, .. }
                if now >= attempt.window.expires_at() =>
            {
                Some(RemoteControlTargetPairingCompletionRetentionExpired::new(
                    attempt.attempt_id(),
                    attempt.view().context(),
                ))
            }
            phase @ (RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Active { .. }
            | RemoteControlTargetPairingPhase::Authorizing { .. }
            | RemoteControlTargetPairingPhase::Completing { .. }) => {
                self.phase = phase;
                None
            }
        }
    }
}

fn abort_active(
    attempt: RemoteControlTargetPairingAttempt,
    stage: RemoteControlTargetPairingActiveStage,
) -> RemoteControlTargetPairingAborted {
    let attempt_id = attempt.attempt_id();
    let context = attempt.view().context();
    match stage {
        RemoteControlTargetPairingActiveStage::OfferPrepared => {
            RemoteControlTargetPairingAborted::OfferPrepared {
                attempt_id,
                context,
            }
        }
        RemoteControlTargetPairingActiveStage::Confirming(
            RemoteControlTargetPairingConfirmationProgress::Both,
        ) => RemoteControlTargetPairingAborted::AwaitingBoth {
            attempt_id,
            context,
        },
        RemoteControlTargetPairingActiveStage::Confirming(
            RemoteControlTargetPairingConfirmationProgress::TargetApproval { responder },
        ) => RemoteControlTargetPairingAborted::AwaitingTargetApproval {
            attempt_id,
            context,
            responder,
        },
        RemoteControlTargetPairingActiveStage::Confirming(
            RemoteControlTargetPairingConfirmationProgress::ControllerCommit,
        ) => RemoteControlTargetPairingAborted::AwaitingControllerCommit {
            attempt_id,
            context,
        },
    }
}

fn validate_commit(
    attempt: &RemoteControlTargetPairingAttempt,
    arrival: RemoteControlTargetPairingCommitArrival,
) -> Result<(), RemoteControlTargetPairingCommitRejection> {
    let expected_link = attempt.view().context().link_id();
    let found_link = arrival.responder.link_id();
    if found_link != expected_link {
        return Err(RemoteControlTargetPairingCommitRejection::WrongLink {
            expected: expected_link,
            found: found_link,
        });
    }
    let expected_controller = attempt.transcript.controller().identity_hash();
    if arrival.identified_controller != expected_controller {
        return Err(
            RemoteControlTargetPairingCommitRejection::ControllerIdentityMismatch {
                expected: expected_controller,
                identified: arrival.identified_controller,
            },
        );
    }
    if !arrival.commit.matches(&attempt.transcript) {
        return Err(
            RemoteControlTargetPairingCommitRejection::TranscriptMismatch {
                expected: attempt.attempt_id(),
                found: arrival.commit.transcript(),
            },
        );
    }
    Ok(())
}
