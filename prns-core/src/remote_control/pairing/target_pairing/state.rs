use crate::identity::IdentitySigner;
use crate::remote_control::RemoteControlControllerGrant;
use crate::routing::links::LinkId;
use crate::units::{DurationMillis, InstantMillis};

use super::super::{
    RemoteControlPairingAttemptId, RemoteControlPairingAttemptTimeout,
    RemoteControlPairingCompleted, RemoteControlPairingContext,
    RemoteControlPairingInvitationProofInvalid, RemoteControlPairingPreparedOffer,
    RemoteControlPairingSession, RemoteControlPairingTranscript, RemoteControlPairingWindow,
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

#[derive(Debug, PartialEq, Eq)]
enum AuthorizationCompletion {
    Unprepared,
    Prepared(RemoteControlPairingCompleted),
}

const _: () = assert!(
    core::mem::size_of::<AuthorizationCompletion>()
        == core::mem::size_of::<RemoteControlPairingCompleted>()
);

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
        completion: AuthorizationCompletion,
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
        let attempt_timeout = attempt_timeout_for_remaining_window(
            session.attempt_timeout(),
            started_at,
            session.window(),
        );
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
        let prepared = match RemoteControlPairingPreparedOffer::new(
            target_signer,
            RemoteControlPairingContext::new(session.endpoint(), arrival.responder.link_id()),
            &arrival.begin,
            session.permissions().clone(),
            attempt_timeout,
        ) {
            Ok(prepared) => prepared,
            Err(super::super::RemoteControlPairingPreparedOfferError::AuthorityUnsupported {
                version,
                authority,
            }) => {
                return BeginRemoteControlTargetPairingOutcome::Rejected {
                    rejected: arrival.responder,
                    reason: RemoteControlTargetPairingBeginRejection::AuthorityUnsupported {
                        version,
                        authority,
                    },
                }
            }
            Err(
                super::super::RemoteControlPairingPreparedOfferError::RequestUnsupportedForVersion {
                    version,
                    request,
                },
            ) => {
                return BeginRemoteControlTargetPairingOutcome::Rejected {
                    rejected: arrival.responder,
                    reason:
                        RemoteControlTargetPairingBeginRejection::RequestUnsupportedForVersion {
                            version,
                            request,
                        },
                }
            }
        };
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
                        self.phase = RemoteControlTargetPairingPhase::Authorizing {
                            attempt,
                            responder,
                            completion: AuthorizationCompletion::Unprepared,
                        };
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
            RemoteControlTargetPairingPhase::Authorizing {
                attempt,
                responder,
                completion,
            } => {
                let attempt_id = attempt.attempt_id();
                self.phase = RemoteControlTargetPairingPhase::Authorizing {
                    attempt,
                    responder,
                    completion,
                };
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
                            completion: AuthorizationCompletion::Unprepared,
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
            RemoteControlTargetPairingPhase::Authorizing {
                attempt,
                responder,
                completion,
            } => {
                self.phase = RemoteControlTargetPairingPhase::Authorizing {
                    attempt,
                    responder,
                    completion,
                };
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
            RemoteControlTargetPairingPhase::Authorizing {
                attempt,
                responder,
                completion,
            } => {
                let attempt_id = attempt.attempt_id();
                self.phase = RemoteControlTargetPairingPhase::Authorizing {
                    attempt,
                    responder,
                    completion,
                };
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

    /// Reserves a signed completion before durable authorization storage begins.
    /// Readiness survives timeout and link loss, but does not expose the completion
    /// for delivery. The caller must resolve indeterminate storage before settling
    /// success or failure; dropping a storage future is not proof of failure.
    pub fn prepare_authorization(
        &mut self,
        prepared: RemoteControlPairingAttemptId,
        target_signer: &impl IdentitySigner,
        now: InstantMillis,
    ) -> PrepareRemoteControlTargetPairingAuthorizationOutcome {
        let RemoteControlTargetPairingPhase::Authorizing {
            attempt,
            completion,
            ..
        } = &mut self.phase
        else {
            return PrepareRemoteControlTargetPairingAuthorizationOutcome::NoAuthorizationOwed;
        };
        let active = attempt.attempt_id();
        if prepared != active {
            return PrepareRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
                prepared,
                active,
            };
        }
        match completion {
            AuthorizationCompletion::Prepared(_) => {
                return PrepareRemoteControlTargetPairingAuthorizationOutcome::Prepared {
                    attempt_id: active,
                };
            }
            AuthorizationCompletion::Unprepared => {}
        }
        if now >= attempt.window.expires_at() {
            return PrepareRemoteControlTargetPairingAuthorizationOutcome::DeadlineElapsed {
                attempt_id: active,
            };
        }
        match RemoteControlPairingCompleted::signed_by(target_signer, &attempt.transcript) {
            Ok(completed) => {
                *completion = AuthorizationCompletion::Prepared(completed);
                PrepareRemoteControlTargetPairingAuthorizationOutcome::Prepared {
                    attempt_id: active,
                }
            }
            Err(error) => PrepareRemoteControlTargetPairingAuthorizationOutcome::SigningFailed {
                attempt_id: active,
                error,
            },
        }
    }

    pub fn authorization_persisted(
        &mut self,
        settled: RemoteControlPairingAttemptId,
        target_signer: &impl IdentitySigner,
        now: InstantMillis,
    ) -> PersistRemoteControlTargetPairingAuthorizationOutcome {
        if let Some(completion) = self.settle_prepared_authorization(settled, now) {
            return completion;
        }
        if let Some(expired) = self.take_expired_completion(now) {
            return PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionRetentionExpired {
                expired,
            };
        }
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Authorizing {
                attempt,
                responder,
                completion,
            } => {
                let active = attempt.attempt_id();
                if settled != active {
                    self.phase = RemoteControlTargetPairingPhase::Authorizing {
                        attempt,
                        responder,
                        completion,
                    };
                    return PersistRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
                        settled,
                        active,
                    };
                }
                if matches!(completion, AuthorizationCompletion::Unprepared)
                    && now >= attempt.window.expires_at()
                {
                    return PersistRemoteControlTargetPairingAuthorizationOutcome::AuthorizationPersistedAfterDeadline {
                        attempt_id: active,
                        context: attempt.view().context(),
                        grant: attempt.grant(),
                    };
                }
                let signed = match completion {
                    AuthorizationCompletion::Prepared(completed) => Ok(completed),
                    AuthorizationCompletion::Unprepared => {
                        RemoteControlPairingCompleted::signed_by(target_signer, &attempt.transcript)
                    }
                };
                match signed {
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
                        self.phase = RemoteControlTargetPairingPhase::Authorizing {
                            attempt,
                            responder,
                            completion: AuthorizationCompletion::Unprepared,
                        };
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

    /// Settles a prepared or retained completion without requiring its signer.
    /// Returns `None` when no prepared or retained completion exists.
    pub fn settle_prepared_authorization(
        &mut self,
        settled: RemoteControlPairingAttemptId,
        now: InstantMillis,
    ) -> Option<PersistRemoteControlTargetPairingAuthorizationOutcome> {
        if let Some(expired) = self.take_expired_completion(now) {
            return Some(
                PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionRetentionExpired {
                    expired,
                },
            );
        }
        let active = match &self.phase {
            RemoteControlTargetPairingPhase::Authorizing {
                attempt,
                completion: AuthorizationCompletion::Prepared(_),
                ..
            }
            | RemoteControlTargetPairingPhase::Completing { attempt, .. } => attempt.attempt_id(),
            RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Active { .. }
            | RemoteControlTargetPairingPhase::Authorizing {
                completion: AuthorizationCompletion::Unprepared,
                ..
            } => return None,
        };
        if settled != active {
            return Some(
                PersistRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
                    settled,
                    active,
                },
            );
        }
        match core::mem::take(&mut self.phase) {
            RemoteControlTargetPairingPhase::Authorizing {
                attempt,
                responder,
                completion: AuthorizationCompletion::Prepared(completed),
            }
            | RemoteControlTargetPairingPhase::Completing {
                attempt,
                responder,
                completed,
            } => {
                self.phase = RemoteControlTargetPairingPhase::Completing {
                    attempt,
                    responder,
                    completed,
                };
                Some(
                    PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed {
                        attempt_id: active,
                        responder,
                        completed,
                    },
                )
            }
            phase @ (RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Active { .. }
            | RemoteControlTargetPairingPhase::Authorizing {
                completion: AuthorizationCompletion::Unprepared,
                ..
            }) => {
                self.phase = phase;
                None
            }
        }
    }

    pub fn authorization_failed(
        &mut self,
        settled: RemoteControlPairingAttemptId,
    ) -> FailRemoteControlTargetPairingAuthorizationOutcome {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Authorizing {
                attempt,
                responder,
                completion,
            } => {
                let active = attempt.attempt_id();
                if settled != active {
                    self.phase = RemoteControlTargetPairingPhase::Authorizing {
                        attempt,
                        responder,
                        completion,
                    };
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
            RemoteControlTargetPairingPhase::Authorizing {
                attempt,
                responder,
                completion,
            } => {
                let attempt_id = attempt.attempt_id();
                let related = attempt.view().context().link_id() == link_id;
                self.phase = RemoteControlTargetPairingPhase::Authorizing {
                    attempt,
                    responder,
                    completion,
                };
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

pub(super) fn attempt_timeout_for_remaining_window(
    configured: RemoteControlPairingAttemptTimeout,
    started_at: InstantMillis,
    pairing_window: &RemoteControlPairingWindow,
) -> RemoteControlPairingAttemptTimeout {
    let remaining = pairing_window.expires_at().0.saturating_sub(started_at.0);
    let bounded = DurationMillis(configured.duration().0.min(remaining));
    // A still-open invitation may have less than the configured attempt duration left.
    // For an elapsed window, retain the configured timeout so the strict constructor
    // returns its existing PairingWindowElapsed error rather than admitting zero time.
    RemoteControlPairingAttemptTimeout::try_from(bounded).unwrap_or(configured)
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
