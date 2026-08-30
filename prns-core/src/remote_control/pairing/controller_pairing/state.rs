use crate::remote_control::{RemoteControlControllerIdentity, RemoteControlTargetAccess};
use crate::routing::links::LinkId;
use crate::units::InstantMillis;

use super::super::{
    RemoteControlPairingAttemptId, RemoteControlPairingBegin, RemoteControlPairingCompleted,
    RemoteControlPairingContext, RemoteControlPairingInvitationCode, RemoteControlPairingOffer,
    RemoteControlPairingTranscript,
};
use super::model::*;

#[derive(Debug, Default, PartialEq, Eq)]
enum RemoteControlControllerPairingPhase {
    #[default]
    Idle,
    AwaitingOffer {
        context: RemoteControlPairingContext,
        controller: RemoteControlControllerIdentity,
        window: RemoteControlControllerPairingWindow,
    },
    AwaitingApproval {
        transcript: RemoteControlPairingTranscript,
        window: RemoteControlControllerPairingAttemptWindow,
    },
    AwaitingCompletion {
        transcript: RemoteControlPairingTranscript,
        window: RemoteControlControllerPairingAttemptWindow,
    },
    Persisting {
        attempt_id: RemoteControlPairingAttemptId,
        access: RemoteControlTargetAccess,
    },
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RemoteControlControllerPairingState {
    phase: RemoteControlControllerPairingPhase,
}

impl RemoteControlControllerPairingState {
    #[must_use]
    pub fn view(&self) -> RemoteControlControllerPairingView<'_> {
        match &self.phase {
            RemoteControlControllerPairingPhase::Idle => RemoteControlControllerPairingView::Idle,
            RemoteControlControllerPairingPhase::AwaitingOffer {
                context,
                controller,
                window,
            } => RemoteControlControllerPairingView::AwaitingOffer(
                RemoteControlControllerPairingBeginView {
                    context: *context,
                    controller,
                    window: *window,
                },
            ),
            RemoteControlControllerPairingPhase::AwaitingApproval { transcript, window } => {
                RemoteControlControllerPairingView::AwaitingApproval(
                    RemoteControlControllerPairingAttemptView {
                        transcript,
                        window: *window,
                    },
                )
            }
            RemoteControlControllerPairingPhase::AwaitingCompletion { transcript, window } => {
                RemoteControlControllerPairingView::AwaitingCompletion(
                    RemoteControlControllerPairingAttemptView {
                        transcript,
                        window: *window,
                    },
                )
            }
            RemoteControlControllerPairingPhase::Persisting { attempt_id, access } => {
                RemoteControlControllerPairingView::Persisting(
                    RemoteControlControllerPairingPersistenceView {
                        attempt_id: *attempt_id,
                        access,
                    },
                )
            }
        }
    }

    pub fn begin(
        &mut self,
        controller: RemoteControlControllerIdentity,
        context: RemoteControlPairingContext,
        invitation_code: RemoteControlPairingInvitationCode,
        started_at: InstantMillis,
        pairing_expires_at: InstantMillis,
    ) -> BeginRemoteControlControllerPairingOutcome {
        if let Some(active) = self.activity() {
            return BeginRemoteControlControllerPairingOutcome::Busy { active };
        }
        let window = match RemoteControlControllerPairingWindow::new(started_at, pairing_expires_at)
        {
            Ok(window) => window,
            Err(reason) => {
                return BeginRemoteControlControllerPairingOutcome::PairingUnavailable { reason }
            }
        };
        let begin = RemoteControlPairingBegin::new(controller, context.endpoint(), invitation_code);
        self.phase = RemoteControlControllerPairingPhase::AwaitingOffer {
            context,
            controller,
            window,
        };
        BeginRemoteControlControllerPairingOutcome::BeginOwed {
            begin,
            context,
            expires_at: window.expires_at(),
        }
    }

    pub fn receive_offer(
        &mut self,
        offer: RemoteControlPairingOffer,
        received_at: InstantMillis,
    ) -> ReceiveRemoteControlControllerPairingOfferOutcome {
        if let Some(expired) = self.take_expired_exchange(received_at) {
            return ReceiveRemoteControlControllerPairingOfferOutcome::Expired { expired };
        }
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlControllerPairingPhase::AwaitingOffer {
                context,
                controller,
                window,
            } => {
                let transcript = match offer.verify_controller(context, &controller) {
                    Ok(transcript) => transcript,
                    Err(reason) => {
                        self.phase = RemoteControlControllerPairingPhase::AwaitingOffer {
                            context,
                            controller,
                            window,
                        };
                        return ReceiveRemoteControlControllerPairingOfferOutcome::Rejected {
                            reason,
                        };
                    }
                };
                let attempt_window = match RemoteControlControllerPairingAttemptWindow::new(
                    received_at,
                    transcript.attempt_timeout(),
                    window,
                ) {
                    Ok(window) => window,
                    Err(reason) => {
                        return ReceiveRemoteControlControllerPairingOfferOutcome::PairingUnavailable {
                            reason,
                        }
                    }
                };
                let attempt_id = (&transcript).into();
                self.phase = RemoteControlControllerPairingPhase::AwaitingApproval {
                    transcript,
                    window: attempt_window,
                };
                ReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired {
                    attempt_id,
                }
            }
            RemoteControlControllerPairingPhase::Idle => {
                ReceiveRemoteControlControllerPairingOfferOutcome::NoActiveAttempt
            }
            phase @ (RemoteControlControllerPairingPhase::AwaitingApproval { .. }
            | RemoteControlControllerPairingPhase::AwaitingCompletion { .. }
            | RemoteControlControllerPairingPhase::Persisting { .. }) => {
                let Some(active) = activity_for(&phase) else {
                    self.phase = phase;
                    return ReceiveRemoteControlControllerPairingOfferOutcome::NoActiveAttempt;
                };
                self.phase = phase;
                ReceiveRemoteControlControllerPairingOfferOutcome::Unexpected { active }
            }
        }
    }

    pub fn approve(
        &mut self,
        requested: RemoteControlPairingAttemptId,
        now: InstantMillis,
    ) -> ApproveRemoteControlControllerPairingOutcome {
        if let Some(expired) = self.take_expired_exchange(now) {
            return ApproveRemoteControlControllerPairingOutcome::Expired { expired };
        }
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlControllerPairingPhase::Idle => {
                ApproveRemoteControlControllerPairingOutcome::NoActiveAttempt
            }
            RemoteControlControllerPairingPhase::AwaitingOffer {
                context,
                controller,
                window,
            } => {
                self.phase = RemoteControlControllerPairingPhase::AwaitingOffer {
                    context,
                    controller,
                    window,
                };
                ApproveRemoteControlControllerPairingOutcome::OfferNotReceived
            }
            RemoteControlControllerPairingPhase::AwaitingApproval { transcript, window } => {
                let active = (&transcript).into();
                if requested != active {
                    self.phase = RemoteControlControllerPairingPhase::AwaitingApproval {
                        transcript,
                        window,
                    };
                    return ApproveRemoteControlControllerPairingOutcome::AttemptMismatch {
                        requested,
                        active,
                    };
                }
                let commit = crate::remote_control::RemoteControlPairingCommit::new(&transcript);
                let context = transcript.context();
                self.phase =
                    RemoteControlControllerPairingPhase::AwaitingCompletion { transcript, window };
                ApproveRemoteControlControllerPairingOutcome::CommitOwed {
                    attempt_id: active,
                    commit,
                    context,
                    expires_at: window.expires_at(),
                }
            }
            RemoteControlControllerPairingPhase::AwaitingCompletion { transcript, window } => {
                let attempt_id = (&transcript).into();
                self.phase =
                    RemoteControlControllerPairingPhase::AwaitingCompletion { transcript, window };
                ApproveRemoteControlControllerPairingOutcome::AlreadyApproved { attempt_id }
            }
            RemoteControlControllerPairingPhase::Persisting { attempt_id, access } => {
                self.phase = RemoteControlControllerPairingPhase::Persisting { attempt_id, access };
                ApproveRemoteControlControllerPairingOutcome::PersistenceInProgress { attempt_id }
            }
        }
    }

    pub fn reject(
        &mut self,
        requested: RemoteControlPairingAttemptId,
        now: InstantMillis,
    ) -> RejectRemoteControlControllerPairingOutcome {
        if let Some(expired) = self.take_expired_exchange(now) {
            return RejectRemoteControlControllerPairingOutcome::Expired { expired };
        }
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlControllerPairingPhase::Idle => {
                RejectRemoteControlControllerPairingOutcome::NoActiveAttempt
            }
            RemoteControlControllerPairingPhase::AwaitingOffer {
                context,
                controller,
                window,
            } => {
                self.phase = RemoteControlControllerPairingPhase::AwaitingOffer {
                    context,
                    controller,
                    window,
                };
                RejectRemoteControlControllerPairingOutcome::OfferNotReceived
            }
            RemoteControlControllerPairingPhase::AwaitingApproval { transcript, window } => {
                let active = (&transcript).into();
                if requested != active {
                    self.phase = RemoteControlControllerPairingPhase::AwaitingApproval {
                        transcript,
                        window,
                    };
                    return RejectRemoteControlControllerPairingOutcome::AttemptMismatch {
                        requested,
                        active,
                    };
                }
                RejectRemoteControlControllerPairingOutcome::Rejected {
                    aborted: RemoteControlControllerPairingAborted::AwaitingApproval {
                        attempt_id: active,
                        context: transcript.context(),
                    },
                }
            }
            RemoteControlControllerPairingPhase::AwaitingCompletion { transcript, window } => {
                let attempt_id = (&transcript).into();
                self.phase =
                    RemoteControlControllerPairingPhase::AwaitingCompletion { transcript, window };
                RejectRemoteControlControllerPairingOutcome::AlreadyApproved { attempt_id }
            }
            RemoteControlControllerPairingPhase::Persisting { attempt_id, access } => {
                self.phase = RemoteControlControllerPairingPhase::Persisting { attempt_id, access };
                RejectRemoteControlControllerPairingOutcome::PersistenceInProgress { attempt_id }
            }
        }
    }

    pub fn receive_completed(
        &mut self,
        completed: RemoteControlPairingCompleted,
        received_at: InstantMillis,
    ) -> ReceiveRemoteControlControllerPairingCompletedOutcome {
        if let Some(expired) = self.take_expired_exchange(received_at) {
            return ReceiveRemoteControlControllerPairingCompletedOutcome::Expired { expired };
        }
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlControllerPairingPhase::Idle => {
                ReceiveRemoteControlControllerPairingCompletedOutcome::NoActiveAttempt
            }
            RemoteControlControllerPairingPhase::AwaitingOffer {
                context,
                controller,
                window,
            } => {
                self.phase = RemoteControlControllerPairingPhase::AwaitingOffer {
                    context,
                    controller,
                    window,
                };
                ReceiveRemoteControlControllerPairingCompletedOutcome::OfferNotReceived
            }
            RemoteControlControllerPairingPhase::AwaitingApproval { transcript, window } => {
                let attempt_id = (&transcript).into();
                self.phase =
                    RemoteControlControllerPairingPhase::AwaitingApproval { transcript, window };
                ReceiveRemoteControlControllerPairingCompletedOutcome::ApprovalRequired {
                    attempt_id,
                }
            }
            RemoteControlControllerPairingPhase::AwaitingCompletion { transcript, window } => {
                let attempt_id = (&transcript).into();
                if let Err(reason) = completed.verify(&transcript) {
                    self.phase = RemoteControlControllerPairingPhase::AwaitingCompletion {
                        transcript,
                        window,
                    };
                    return ReceiveRemoteControlControllerPairingCompletedOutcome::Rejected {
                        attempt_id,
                        reason,
                    };
                }
                let (_context, _controller, target, permissions, _attempt_timeout) =
                    transcript.into_parts();
                self.phase = RemoteControlControllerPairingPhase::Persisting {
                    attempt_id,
                    access: (target, permissions).into(),
                };
                ReceiveRemoteControlControllerPairingCompletedOutcome::PersistenceOwed {
                    attempt_id,
                }
            }
            RemoteControlControllerPairingPhase::Persisting { attempt_id, access } => {
                self.phase = RemoteControlControllerPairingPhase::Persisting { attempt_id, access };
                ReceiveRemoteControlControllerPairingCompletedOutcome::AlreadyReceived {
                    attempt_id,
                }
            }
        }
    }

    #[must_use]
    pub fn target_access(
        &self,
        attempt_id: RemoteControlPairingAttemptId,
    ) -> Option<&RemoteControlTargetAccess> {
        let RemoteControlControllerPairingPhase::Persisting {
            attempt_id: active,
            access,
        } = &self.phase
        else {
            return None;
        };
        if *active != attempt_id {
            return None;
        }
        Some(access)
    }

    pub fn persistence_succeeded(
        &mut self,
        persisted: RemoteControlPairingAttemptId,
    ) -> PersistRemoteControlControllerPairingOutcome {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlControllerPairingPhase::Persisting { attempt_id, access } => {
                if persisted != attempt_id {
                    self.phase =
                        RemoteControlControllerPairingPhase::Persisting { attempt_id, access };
                    return PersistRemoteControlControllerPairingOutcome::AttemptMismatch {
                        persisted,
                        active: attempt_id,
                    };
                }
                PersistRemoteControlControllerPairingOutcome::Completed { attempt_id, access }
            }
            phase @ (RemoteControlControllerPairingPhase::Idle
            | RemoteControlControllerPairingPhase::AwaitingOffer { .. }
            | RemoteControlControllerPairingPhase::AwaitingApproval { .. }
            | RemoteControlControllerPairingPhase::AwaitingCompletion { .. }) => {
                self.phase = phase;
                PersistRemoteControlControllerPairingOutcome::NoPersistenceOwed
            }
        }
    }

    pub fn persistence_failed(
        &mut self,
        failed: RemoteControlPairingAttemptId,
    ) -> FailRemoteControlControllerPairingPersistenceOutcome {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlControllerPairingPhase::Persisting { attempt_id, access } => {
                if failed != attempt_id {
                    self.phase =
                        RemoteControlControllerPairingPhase::Persisting { attempt_id, access };
                    return FailRemoteControlControllerPairingPersistenceOutcome::AttemptMismatch {
                        failed,
                        active: attempt_id,
                    };
                }
                FailRemoteControlControllerPairingPersistenceOutcome::Failed { attempt_id, access }
            }
            phase @ (RemoteControlControllerPairingPhase::Idle
            | RemoteControlControllerPairingPhase::AwaitingOffer { .. }
            | RemoteControlControllerPairingPhase::AwaitingApproval { .. }
            | RemoteControlControllerPairingPhase::AwaitingCompletion { .. }) => {
                self.phase = phase;
                FailRemoteControlControllerPairingPersistenceOutcome::NoPersistenceOwed
            }
        }
    }

    pub fn expire(&mut self, now: InstantMillis) -> ExpireRemoteControlControllerPairingOutcome {
        if let Some(aborted) = self.take_expired_exchange(now) {
            return ExpireRemoteControlControllerPairingOutcome::Expired { aborted };
        }
        match self.view() {
            RemoteControlControllerPairingView::Idle => {
                ExpireRemoteControlControllerPairingOutcome::NoActiveAttempt
            }
            RemoteControlControllerPairingView::AwaitingOffer(begin) => {
                ExpireRemoteControlControllerPairingOutcome::NotDue {
                    expires_at: begin.window().expires_at(),
                }
            }
            RemoteControlControllerPairingView::AwaitingApproval(attempt)
            | RemoteControlControllerPairingView::AwaitingCompletion(attempt) => {
                ExpireRemoteControlControllerPairingOutcome::NotDue {
                    expires_at: attempt.window().expires_at(),
                }
            }
            RemoteControlControllerPairingView::Persisting(persistence) => {
                ExpireRemoteControlControllerPairingOutcome::PersistenceInProgress {
                    attempt_id: persistence.attempt_id(),
                }
            }
        }
    }

    pub fn close_link(
        &mut self,
        link_id: LinkId,
    ) -> CloseRemoteControlControllerPairingLinkOutcome {
        match self.abort_for_link(link_id) {
            RemoteControlControllerPairingLinkAbort::Aborted(aborted) => {
                CloseRemoteControlControllerPairingLinkOutcome::Aborted { aborted }
            }
            RemoteControlControllerPairingLinkAbort::UnrelatedLink => {
                CloseRemoteControlControllerPairingLinkOutcome::UnrelatedLink
            }
            RemoteControlControllerPairingLinkAbort::NoActiveAttempt => {
                CloseRemoteControlControllerPairingLinkOutcome::NoActiveAttempt
            }
            RemoteControlControllerPairingLinkAbort::PersistenceInProgress { attempt_id } => {
                CloseRemoteControlControllerPairingLinkOutcome::PersistenceInProgress { attempt_id }
            }
        }
    }

    pub fn request_failed(
        &mut self,
        link_id: LinkId,
    ) -> FailRemoteControlControllerPairingRequestOutcome {
        match self.abort_for_link(link_id) {
            RemoteControlControllerPairingLinkAbort::Aborted(aborted) => {
                FailRemoteControlControllerPairingRequestOutcome::Aborted { aborted }
            }
            RemoteControlControllerPairingLinkAbort::UnrelatedLink => {
                FailRemoteControlControllerPairingRequestOutcome::UnrelatedLink
            }
            RemoteControlControllerPairingLinkAbort::NoActiveAttempt => {
                FailRemoteControlControllerPairingRequestOutcome::NoActiveAttempt
            }
            RemoteControlControllerPairingLinkAbort::PersistenceInProgress { attempt_id } => {
                FailRemoteControlControllerPairingRequestOutcome::PersistenceInProgress {
                    attempt_id,
                }
            }
        }
    }

    fn activity(&self) -> Option<RemoteControlControllerPairingActivity> {
        activity_for(&self.phase)
    }

    fn take_expired_exchange(
        &mut self,
        now: InstantMillis,
    ) -> Option<RemoteControlControllerPairingAborted> {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlControllerPairingPhase::AwaitingOffer {
                context, window, ..
            } if now >= window.expires_at() => {
                Some(RemoteControlControllerPairingAborted::AwaitingOffer { context })
            }
            RemoteControlControllerPairingPhase::AwaitingApproval { transcript, window }
                if now >= window.expires_at() =>
            {
                let attempt_id = (&transcript).into();
                Some(RemoteControlControllerPairingAborted::AwaitingApproval {
                    attempt_id,
                    context: transcript.context(),
                })
            }
            RemoteControlControllerPairingPhase::AwaitingCompletion { transcript, window }
                if now >= window.expires_at() =>
            {
                let attempt_id = (&transcript).into();
                Some(RemoteControlControllerPairingAborted::AwaitingCompletion {
                    attempt_id,
                    context: transcript.context(),
                })
            }
            phase @ (RemoteControlControllerPairingPhase::Idle
            | RemoteControlControllerPairingPhase::AwaitingOffer { .. }
            | RemoteControlControllerPairingPhase::AwaitingApproval { .. }
            | RemoteControlControllerPairingPhase::AwaitingCompletion { .. }
            | RemoteControlControllerPairingPhase::Persisting { .. }) => {
                self.phase = phase;
                None
            }
        }
    }

    fn abort_for_link(&mut self, link_id: LinkId) -> RemoteControlControllerPairingLinkAbort {
        let phase = core::mem::take(&mut self.phase);
        match phase {
            RemoteControlControllerPairingPhase::Idle => {
                RemoteControlControllerPairingLinkAbort::NoActiveAttempt
            }
            RemoteControlControllerPairingPhase::AwaitingOffer { context, .. }
                if context.link_id() == link_id =>
            {
                RemoteControlControllerPairingLinkAbort::Aborted(
                    RemoteControlControllerPairingAborted::AwaitingOffer { context },
                )
            }
            RemoteControlControllerPairingPhase::AwaitingApproval { transcript, .. }
                if transcript.context().link_id() == link_id =>
            {
                RemoteControlControllerPairingLinkAbort::Aborted(
                    RemoteControlControllerPairingAborted::AwaitingApproval {
                        attempt_id: (&transcript).into(),
                        context: transcript.context(),
                    },
                )
            }
            RemoteControlControllerPairingPhase::AwaitingCompletion { transcript, .. }
                if transcript.context().link_id() == link_id =>
            {
                RemoteControlControllerPairingLinkAbort::Aborted(
                    RemoteControlControllerPairingAborted::AwaitingCompletion {
                        attempt_id: (&transcript).into(),
                        context: transcript.context(),
                    },
                )
            }
            RemoteControlControllerPairingPhase::Persisting { attempt_id, access } => {
                self.phase = RemoteControlControllerPairingPhase::Persisting { attempt_id, access };
                RemoteControlControllerPairingLinkAbort::PersistenceInProgress { attempt_id }
            }
            phase @ (RemoteControlControllerPairingPhase::AwaitingOffer { .. }
            | RemoteControlControllerPairingPhase::AwaitingApproval { .. }
            | RemoteControlControllerPairingPhase::AwaitingCompletion { .. }) => {
                self.phase = phase;
                RemoteControlControllerPairingLinkAbort::UnrelatedLink
            }
        }
    }
}

enum RemoteControlControllerPairingLinkAbort {
    Aborted(RemoteControlControllerPairingAborted),
    UnrelatedLink,
    NoActiveAttempt,
    PersistenceInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

fn activity_for(
    phase: &RemoteControlControllerPairingPhase,
) -> Option<RemoteControlControllerPairingActivity> {
    match phase {
        RemoteControlControllerPairingPhase::AwaitingOffer { .. } => {
            Some(RemoteControlControllerPairingActivity::AwaitingOffer)
        }
        RemoteControlControllerPairingPhase::AwaitingApproval { transcript, .. } => {
            Some(RemoteControlControllerPairingActivity::AwaitingApproval {
                attempt_id: transcript.into(),
            })
        }
        RemoteControlControllerPairingPhase::AwaitingCompletion { transcript, .. } => {
            Some(RemoteControlControllerPairingActivity::AwaitingCompletion {
                attempt_id: transcript.into(),
            })
        }
        RemoteControlControllerPairingPhase::Persisting { attempt_id, .. } => {
            Some(RemoteControlControllerPairingActivity::Persisting {
                attempt_id: *attempt_id,
            })
        }
        RemoteControlControllerPairingPhase::Idle => None,
    }
}
