use crate::identity::IdentitySigner;
use crate::remote_control::RemoteControlControllerGrant;
use crate::routing::links::LinkId;
use crate::units::InstantMillis;

use super::super::{
    RemoteControlPairingAttemptTimeout, RemoteControlPairingBegin, RemoteControlPairingCompleted,
    RemoteControlPairingContext, RemoteControlPairingOffer, RemoteControlPairingPreparedOffer,
    RemoteControlPairingSession,
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

#[derive(Debug, PartialEq, Eq)]
struct RemoteControlTargetPairingAttempt {
    prepared: RemoteControlPairingPreparedOffer,
    window: RemoteControlTargetPairingAttemptWindow,
}

impl RemoteControlTargetPairingAttempt {
    fn view(&self) -> RemoteControlTargetPairingAttemptView<'_> {
        RemoteControlTargetPairingAttemptView {
            transcript: self.prepared.transcript(),
            window: self.window,
        }
    }

    fn attempt_id(&self) -> RemoteControlTargetPairingAttemptId {
        self.prepared.transcript().into()
    }

    fn grant(&self) -> RemoteControlControllerGrant {
        (
            self.prepared.transcript().controller(),
            self.prepared.transcript().permissions(),
        )
            .into()
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
enum RemoteControlTargetPairingPhase {
    #[default]
    Idle,
    Confirming {
        attempt: RemoteControlTargetPairingAttempt,
        progress: RemoteControlTargetPairingConfirmationProgress,
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
            RemoteControlTargetPairingPhase::Confirming { attempt, progress } => match progress {
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
        link_id: LinkId,
        begin: &RemoteControlPairingBegin,
        started_at: InstantMillis,
        attempt_timeout: RemoteControlPairingAttemptTimeout,
    ) -> BeginRemoteControlTargetPairingOutcome {
        if let Some(expired) = self.take_expired_confirmation(started_at) {
            return BeginRemoteControlTargetPairingOutcome::Expired { expired };
        }
        if let Some(active) = self.active_attempt_id() {
            return BeginRemoteControlTargetPairingOutcome::Busy { active };
        }
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
            RemoteControlPairingContext::new(session.endpoint(), link_id),
            begin,
            session.permissions().clone(),
            attempt_timeout,
        );
        let attempt = RemoteControlTargetPairingAttempt { prepared, window };
        let attempt_id = attempt.attempt_id();
        self.phase = RemoteControlTargetPairingPhase::Confirming {
            attempt,
            progress: RemoteControlTargetPairingConfirmationProgress::Both,
        };
        BeginRemoteControlTargetPairingOutcome::Offered { attempt_id }
    }

    #[must_use]
    pub fn offer(
        &self,
        attempt_id: RemoteControlTargetPairingAttemptId,
    ) -> Option<&RemoteControlPairingOffer> {
        let attempt = self.attempt()?;
        if attempt.attempt_id() != attempt_id {
            return None;
        }
        Some(attempt.prepared.offer())
    }

    pub fn approve(
        &mut self,
        requested: RemoteControlTargetPairingAttemptId,
        now: InstantMillis,
    ) -> ApproveRemoteControlTargetPairingOutcome {
        if let Some(expired) = self.take_expired_confirmation(now) {
            return ApproveRemoteControlTargetPairingOutcome::Expired { expired };
        }
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Idle => {
                ApproveRemoteControlTargetPairingOutcome::NoActiveAttempt
            }
            RemoteControlTargetPairingPhase::Confirming { attempt, progress } => {
                let active = attempt.attempt_id();
                if requested != active {
                    self.phase = RemoteControlTargetPairingPhase::Confirming { attempt, progress };
                    return ApproveRemoteControlTargetPairingOutcome::AttemptMismatch {
                        requested,
                        active,
                    };
                }
                match progress {
                    RemoteControlTargetPairingConfirmationProgress::Both => {
                        self.phase = RemoteControlTargetPairingPhase::Confirming {
                            attempt,
                            progress:
                                RemoteControlTargetPairingConfirmationProgress::ControllerCommit,
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
                        self.phase =
                            RemoteControlTargetPairingPhase::Confirming { attempt, progress };
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
        if let Some(expired) = self.take_expired_confirmation(now) {
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
            RemoteControlTargetPairingPhase::Confirming { attempt, progress } => {
                let attempt_id = attempt.attempt_id();
                match progress {
                    RemoteControlTargetPairingConfirmationProgress::Both => {
                        self.phase = RemoteControlTargetPairingPhase::Confirming {
                            attempt,
                            progress:
                                RemoteControlTargetPairingConfirmationProgress::TargetApproval {
                                    responder: arrival.responder,
                                },
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
                        self.phase =
                            RemoteControlTargetPairingPhase::Confirming { attempt, progress };
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
                self.phase = RemoteControlTargetPairingPhase::Completing {
                    attempt,
                    responder,
                    completed,
                };
                CommitRemoteControlTargetPairingOutcome::Rejected {
                    rejected: arrival.responder,
                    reason: RemoteControlTargetPairingCommitRejection::FinalizationInProgress,
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
        requested: RemoteControlTargetPairingAttemptId,
        now: InstantMillis,
    ) -> RejectRemoteControlTargetPairingOutcome {
        if let Some(expired) = self.take_expired_confirmation(now) {
            return RejectRemoteControlTargetPairingOutcome::Expired { expired };
        }
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Idle => {
                RejectRemoteControlTargetPairingOutcome::NoActiveAttempt
            }
            RemoteControlTargetPairingPhase::Confirming { attempt, progress } => {
                let active = attempt.attempt_id();
                if requested != active {
                    self.phase = RemoteControlTargetPairingPhase::Confirming { attempt, progress };
                    return RejectRemoteControlTargetPairingOutcome::AttemptMismatch {
                        requested,
                        active,
                    };
                }
                match progress {
                    RemoteControlTargetPairingConfirmationProgress::Both
                    | RemoteControlTargetPairingConfirmationProgress::TargetApproval { .. } => {
                        RejectRemoteControlTargetPairingOutcome::Rejected {
                            aborted: abort_confirming(attempt, progress),
                        }
                    }
                    RemoteControlTargetPairingConfirmationProgress::ControllerCommit => {
                        self.phase =
                            RemoteControlTargetPairingPhase::Confirming { attempt, progress };
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
        settled: RemoteControlTargetPairingAttemptId,
        target_signer: &impl IdentitySigner,
    ) -> PersistRemoteControlTargetPairingAuthorizationOutcome {
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
                match RemoteControlPairingCompleted::signed_by(
                    target_signer,
                    attempt.prepared.transcript(),
                ) {
                    Ok(completed) => {
                        self.phase = RemoteControlTargetPairingPhase::Completing {
                            attempt,
                            responder,
                            completed,
                        };
                        PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed {
                            attempt_id: active,
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
            phase @ (RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Confirming { .. }
            | RemoteControlTargetPairingPhase::Completing { .. }) => {
                self.phase = phase;
                PersistRemoteControlTargetPairingAuthorizationOutcome::NoAuthorizationOwed
            }
        }
    }

    pub fn authorization_failed(
        &mut self,
        settled: RemoteControlTargetPairingAttemptId,
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
                    responder,
                }
            }
            phase @ (RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Confirming { .. }
            | RemoteControlTargetPairingPhase::Completing { .. }) => {
                self.phase = phase;
                FailRemoteControlTargetPairingAuthorizationOutcome::NoAuthorizationOwed
            }
        }
    }

    #[must_use]
    pub fn completion(
        &self,
        attempt_id: RemoteControlTargetPairingAttemptId,
    ) -> Option<RemoteControlTargetPairingCompletionView<'_>> {
        let RemoteControlTargetPairingPhase::Completing {
            attempt,
            responder,
            completed,
        } = &self.phase
        else {
            return None;
        };
        if attempt.attempt_id() != attempt_id {
            return None;
        }
        Some(RemoteControlTargetPairingCompletionView {
            attempt: attempt.view(),
            responder: *responder,
            completed,
        })
    }

    pub fn complete(
        &mut self,
        settled: RemoteControlTargetPairingAttemptId,
    ) -> CompleteRemoteControlTargetPairingOutcome {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Completing {
                attempt,
                responder,
                completed,
            } => {
                let active = attempt.attempt_id();
                if settled != active {
                    self.phase = RemoteControlTargetPairingPhase::Completing {
                        attempt,
                        responder,
                        completed,
                    };
                    return CompleteRemoteControlTargetPairingOutcome::AttemptMismatch {
                        settled,
                        active,
                    };
                }
                CompleteRemoteControlTargetPairingOutcome::Completed { attempt_id: active }
            }
            phase @ (RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Confirming { .. }
            | RemoteControlTargetPairingPhase::Authorizing { .. }) => {
                self.phase = phase;
                CompleteRemoteControlTargetPairingOutcome::NoCompletionOwed
            }
        }
    }

    pub fn expire(&mut self, now: InstantMillis) -> ExpireRemoteControlTargetPairingOutcome {
        if let Some(aborted) = self.take_expired_confirmation(now) {
            return ExpireRemoteControlTargetPairingOutcome::Expired { aborted };
        }
        match self.view() {
            RemoteControlTargetPairingView::Idle => {
                ExpireRemoteControlTargetPairingOutcome::NoActiveAttempt
            }
            RemoteControlTargetPairingView::AwaitingBoth(attempt)
            | RemoteControlTargetPairingView::AwaitingTargetApproval(attempt)
            | RemoteControlTargetPairingView::AwaitingControllerCommit(attempt) => {
                ExpireRemoteControlTargetPairingOutcome::NotDue {
                    expires_at: attempt.window().expires_at(),
                }
            }
            RemoteControlTargetPairingView::Authorizing(attempt)
            | RemoteControlTargetPairingView::Completing(attempt) => {
                ExpireRemoteControlTargetPairingOutcome::FinalizationInProgress {
                    attempt_id: attempt.attempt_id(),
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
            RemoteControlTargetPairingPhase::Confirming { attempt, progress }
                if attempt.view().context().link_id() == link_id =>
            {
                CloseRemoteControlTargetPairingLinkOutcome::Aborted {
                    aborted: abort_confirming(attempt, progress),
                }
            }
            RemoteControlTargetPairingPhase::Confirming { attempt, progress } => {
                self.phase = RemoteControlTargetPairingPhase::Confirming { attempt, progress };
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
                self.phase = RemoteControlTargetPairingPhase::Completing {
                    attempt,
                    responder,
                    completed,
                };
                if related {
                    CloseRemoteControlTargetPairingLinkOutcome::FinalizationInProgress {
                        attempt_id,
                    }
                } else {
                    CloseRemoteControlTargetPairingLinkOutcome::UnrelatedLink
                }
            }
        }
    }

    fn attempt(&self) -> Option<&RemoteControlTargetPairingAttempt> {
        match &self.phase {
            RemoteControlTargetPairingPhase::Idle => None,
            RemoteControlTargetPairingPhase::Confirming { attempt, .. }
            | RemoteControlTargetPairingPhase::Authorizing { attempt, .. }
            | RemoteControlTargetPairingPhase::Completing { attempt, .. } => Some(attempt),
        }
    }

    fn active_attempt_id(&self) -> Option<RemoteControlTargetPairingAttemptId> {
        self.attempt()
            .map(RemoteControlTargetPairingAttempt::attempt_id)
    }

    fn take_expired_confirmation(
        &mut self,
        now: InstantMillis,
    ) -> Option<RemoteControlTargetPairingAborted> {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlTargetPairingPhase::Confirming { attempt, progress }
                if now >= attempt.window.expires_at() =>
            {
                Some(abort_confirming(attempt, progress))
            }
            phase @ (RemoteControlTargetPairingPhase::Idle
            | RemoteControlTargetPairingPhase::Confirming { .. }
            | RemoteControlTargetPairingPhase::Authorizing { .. }
            | RemoteControlTargetPairingPhase::Completing { .. }) => {
                self.phase = phase;
                None
            }
        }
    }
}

fn abort_confirming(
    attempt: RemoteControlTargetPairingAttempt,
    progress: RemoteControlTargetPairingConfirmationProgress,
) -> RemoteControlTargetPairingAborted {
    let attempt_id = attempt.attempt_id();
    let context = attempt.view().context();
    match progress {
        RemoteControlTargetPairingConfirmationProgress::Both => {
            RemoteControlTargetPairingAborted::AwaitingBoth {
                attempt_id,
                context,
            }
        }
        RemoteControlTargetPairingConfirmationProgress::TargetApproval { responder } => {
            RemoteControlTargetPairingAborted::AwaitingTargetApproval {
                attempt_id,
                context,
                responder,
            }
        }
        RemoteControlTargetPairingConfirmationProgress::ControllerCommit => {
            RemoteControlTargetPairingAborted::AwaitingControllerCommit {
                attempt_id,
                context,
            }
        }
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
    if !arrival.commit.matches(attempt.prepared.transcript()) {
        return Err(
            RemoteControlTargetPairingCommitRejection::TranscriptMismatch {
                expected: attempt.attempt_id(),
                found: arrival.commit.transcript(),
            },
        );
    }
    Ok(())
}
