use crate::engine::{
    EngineReaction, EngineState, RemoteControlTargetPairingAuthorizationPersistence,
    RemoteControlTargetPairingFinalization, SettleRemoteControlTargetPairingAuthorization,
    SettleRemoteControlTargetPairingAuthorizationFailure,
};
use crate::interfaces::AttachedInterfaces;
use crate::remote_control::{
    FailRemoteControlTargetPairingAuthorizationOutcome,
    PersistRemoteControlTargetPairingAuthorizationOutcome, RemoteControlPairingResponse,
    RemoteControlTargetPairingView,
};
use crate::storage::StorageLayout;
use crate::units::InstantMillis;

impl<S: StorageLayout> EngineState<S> {
    pub(crate) fn prepare_remote_control_target_pairing_authorization_into<F, Work>(
        &mut self,
        attempt_id: crate::remote_control::RemoteControlPairingAttemptId,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_, Work>),
    ) -> Result<(), crate::engine::RemoteControlTargetPairingPreparationFailure>
    where
        F: FnMut(&mut [u8]),
    {
        use crate::engine::RemoteControlTargetPairingPreparationFailure as Failure;
        use crate::remote_control::PrepareRemoteControlTargetPairingAuthorizationOutcome as Outcome;
        let target_identity = match self.remote_control_target_pairing.view() {
            RemoteControlTargetPairingView::Authorizing(attempt)
                if attempt.attempt_id() == attempt_id =>
            {
                attempt.target().identity_hash()
            }
            RemoteControlTargetPairingView::Authorizing(attempt) => {
                return Err(Failure::AttemptMismatch {
                    active: attempt.attempt_id(),
                })
            }
            RemoteControlTargetPairingView::Idle
            | RemoteControlTargetPairingView::OfferPrepared(_)
            | RemoteControlTargetPairingView::AwaitingBoth(_)
            | RemoteControlTargetPairingView::AwaitingTargetApproval(_)
            | RemoteControlTargetPairingView::AwaitingControllerCommit(_)
            | RemoteControlTargetPairingView::Completing(_) => {
                return Err(Failure::NoAuthorizationOwed)
            }
        };
        let result = match self.held_identities.get(&target_identity) {
            Some(signer) => match self
                .remote_control_target_pairing
                .prepare_authorization(attempt_id, &signer, now)
            {
                Outcome::Prepared { .. } => Ok(()),
                Outcome::DeadlineElapsed { .. } => Err(Failure::DeadlineElapsed),
                Outcome::SigningFailed { error, .. } => Err(Failure::SigningFailed { error }),
                Outcome::NoAuthorizationOwed => Err(Failure::NoAuthorizationOwed),
                Outcome::AttemptMismatch { active, .. } => Err(Failure::AttemptMismatch { active }),
            },
            None => Err(Failure::TargetSignerUnavailable { target_identity }),
        };
        if result.is_err() {
            if let FailRemoteControlTargetPairingAuthorizationOutcome::Aborted { context, .. } =
                self.remote_control_target_pairing
                    .authorization_failed(attempt_id)
            {
                self.retire_remote_control_pairing_exchange_link(
                    context,
                    interfaces,
                    fill_entropy,
                    sink,
                );
            }
        }
        result
    }

    pub(crate) fn settle_remote_control_target_pairing_authorization_into<F>(
        &mut self,
        settlement: SettleRemoteControlTargetPairingAuthorization,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> Result<
        RemoteControlTargetPairingFinalization,
        SettleRemoteControlTargetPairingAuthorizationFailure,
    >
    where
        F: FnMut(&mut [u8]),
    {
        let attempt_id = settlement.attempt_id;
        match settlement.persistence {
            RemoteControlTargetPairingAuthorizationPersistence::Failed => {
                match self
                    .remote_control_target_pairing
                    .authorization_failed(attempt_id)
                {
                    FailRemoteControlTargetPairingAuthorizationOutcome::Aborted {
                        attempt_id,
                        context,
                        responder,
                    } => Ok(
                        RemoteControlTargetPairingFinalization::AuthorizationFailureRecorded {
                            attempt_id,
                            retired_link: self.retire_remote_control_pairing_exchange_link(
                                context,
                                interfaces,
                                fill_entropy,
                                sink,
                            ),
                            responder,
                        },
                    ),
                    FailRemoteControlTargetPairingAuthorizationOutcome::NoAuthorizationOwed => Err(
                        SettleRemoteControlTargetPairingAuthorizationFailure::NoAuthorizationOwed {
                            settled: attempt_id,
                        },
                    ),
                    FailRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
                        settled,
                        active,
                    } => Err(
                        SettleRemoteControlTargetPairingAuthorizationFailure::AttemptMismatch {
                            settled,
                            active,
                        },
                    ),
                }
            }
            RemoteControlTargetPairingAuthorizationPersistence::Persisted => {
                let target_identity = match self.remote_control_target_pairing.view() {
                    RemoteControlTargetPairingView::Authorizing(attempt)
                    | RemoteControlTargetPairingView::Completing(attempt)
                        if attempt.attempt_id() == attempt_id =>
                    {
                        attempt.target().identity_hash()
                    }
                    RemoteControlTargetPairingView::Authorizing(attempt)
                    | RemoteControlTargetPairingView::Completing(attempt) => {
                        return Err(
                            SettleRemoteControlTargetPairingAuthorizationFailure::AttemptMismatch {
                                settled: attempt_id,
                                active: attempt.attempt_id(),
                            },
                        )
                    }
                    RemoteControlTargetPairingView::Idle
                    | RemoteControlTargetPairingView::OfferPrepared(_)
                    | RemoteControlTargetPairingView::AwaitingBoth(_)
                    | RemoteControlTargetPairingView::AwaitingTargetApproval(_)
                    | RemoteControlTargetPairingView::AwaitingControllerCommit(_) => return Err(
                        SettleRemoteControlTargetPairingAuthorizationFailure::NoAuthorizationOwed {
                            settled: attempt_id,
                        },
                    ),
                };
                let completion = match self
                    .remote_control_target_pairing
                    .settle_prepared_authorization(attempt_id, now)
                {
                    Some(completion) => completion,
                    None => {
                        let Some(target_signer) = self.held_identities.get(&target_identity) else {
                            return Err(
                        SettleRemoteControlTargetPairingAuthorizationFailure::TargetSignerUnavailable {
                            attempt_id,
                            target_identity,
                        },
                    );
                        };
                        self.remote_control_target_pairing.authorization_persisted(
                            attempt_id,
                            &target_signer,
                            now,
                        )
                    }
                };
                let (attempt_id, responder, completed) = match completion {
                    PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionOwed {
                        attempt_id,
                        responder,
                        completed,
                    } => (attempt_id, responder, completed),
                    PersistRemoteControlTargetPairingAuthorizationOutcome::SigningFailed {
                        attempt_id,
                        error,
                    } => {
                        return Err(
                            SettleRemoteControlTargetPairingAuthorizationFailure::CompletionSigningFailed {
                                attempt_id,
                                error,
                            },
                        )
                    }
                    PersistRemoteControlTargetPairingAuthorizationOutcome::AuthorizationPersistedAfterDeadline {
                        attempt_id,
                        context,
                        grant,
                    } => {
                        return Ok(
                            RemoteControlTargetPairingFinalization::AuthorizationRollbackRequired {
                                attempt_id,
                                retired_link: self.retire_remote_control_pairing_exchange_link(
                                    context,
                                    interfaces,
                                    fill_entropy,
                                    sink,
                                ),
                                grant,
                            },
                        )
                    }
                    PersistRemoteControlTargetPairingAuthorizationOutcome::CompletionRetentionExpired {
                        expired,
                    } => {
                        sink(EngineReaction::Journaled(
                            crate::engine::Journaled::RemoteControlTargetPairingCompletionRetentionExpired {
                                attempt_id: expired.attempt_id(),
                            },
                        ));
                        return Err(
                            SettleRemoteControlTargetPairingAuthorizationFailure::CompletionRetentionExpired {
                                attempt_id: expired.attempt_id(),
                                retired_link: self.retire_remote_control_pairing_exchange_link(
                                    expired.context(),
                                    interfaces,
                                    fill_entropy,
                                    sink,
                                ),
                            },
                        )
                    }
                    PersistRemoteControlTargetPairingAuthorizationOutcome::NoAuthorizationOwed => {
                        return Err(
                            SettleRemoteControlTargetPairingAuthorizationFailure::NoAuthorizationOwed {
                                settled: attempt_id,
                            },
                        )
                    }
                    PersistRemoteControlTargetPairingAuthorizationOutcome::AttemptMismatch {
                        settled,
                        active,
                    } => {
                        return Err(
                            SettleRemoteControlTargetPairingAuthorizationFailure::AttemptMismatch {
                                settled,
                                active,
                            },
                        )
                    }
                };
                self.dispatch_remote_control_pairing_response(
                    responder,
                    RemoteControlPairingResponse::Completed(completed),
                    interfaces,
                    now,
                    fill_entropy,
                    sink,
                )
                .map_err(|failure| {
                    SettleRemoteControlTargetPairingAuthorizationFailure::CompletionDispatchFailed {
                        attempt_id,
                        failure,
                    }
                })?;
                Ok(RemoteControlTargetPairingFinalization::CompletionDispatched { attempt_id })
            }
        }
    }
}
