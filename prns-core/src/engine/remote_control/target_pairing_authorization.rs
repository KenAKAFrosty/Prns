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
                let Some(target_signer) = self.held_identities.get(&target_identity) else {
                    return Err(
                        SettleRemoteControlTargetPairingAuthorizationFailure::TargetSignerUnavailable {
                            attempt_id,
                            target_identity,
                        },
                    );
                };
                let (attempt_id, responder, completed) = match self
                    .remote_control_target_pairing
                    .authorization_persisted(attempt_id, &target_signer, now)
                {
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
