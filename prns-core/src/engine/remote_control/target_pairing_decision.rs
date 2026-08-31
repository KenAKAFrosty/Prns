use crate::engine::{
    ApproveRemoteControlTargetPairing, ApproveRemoteControlTargetPairingFailure, EngineReaction,
    EngineState, RejectRemoteControlTargetPairing, RejectRemoteControlTargetPairingFailure,
    RemoteControlTargetPairingApproval, RemoteControlTargetPairingRejection,
};
use crate::interfaces::AttachedInterfaces;
use crate::remote_control::{
    ApproveRemoteControlTargetPairingOutcome, RejectRemoteControlTargetPairingOutcome,
};
use crate::storage::StorageLayout;
use crate::units::InstantMillis;

impl<S: StorageLayout> EngineState<S> {
    pub(crate) fn approve_remote_control_target_pairing_into<F>(
        &mut self,
        command: ApproveRemoteControlTargetPairing,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> Result<RemoteControlTargetPairingApproval, ApproveRemoteControlTargetPairingFailure>
    where
        F: FnMut(&mut [u8]),
    {
        match self
            .remote_control_target_pairing
            .approve(command.attempt_id, now)
        {
            ApproveRemoteControlTargetPairingOutcome::AwaitingControllerCommit { attempt_id } => {
                Ok(RemoteControlTargetPairingApproval::AwaitingControllerCommit { attempt_id })
            }
            ApproveRemoteControlTargetPairingOutcome::AuthorizationOwed { attempt_id, grant } => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlTargetPairingAuthorizationRequired {
                        attempt_id,
                        grant,
                    },
                ));
                Ok(RemoteControlTargetPairingApproval::AuthorizationOwed { attempt_id, grant })
            }
            ApproveRemoteControlTargetPairingOutcome::Expired { expired } => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlTargetPairingExpired {
                        aborted: expired,
                    },
                ));
                Err(ApproveRemoteControlTargetPairingFailure::Expired {
                    retired_link: self.retire_remote_control_pairing_exchange_link(
                        expired.context(),
                        interfaces,
                        fill_entropy,
                        sink,
                    ),
                    expired,
                })
            }
            ApproveRemoteControlTargetPairingOutcome::CompletionRetentionExpired { expired } => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlTargetPairingCompletionRetentionExpired {
                        attempt_id: expired.attempt_id(),
                    },
                ));
                Err(
                    ApproveRemoteControlTargetPairingFailure::CompletionRetentionExpired {
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
            ApproveRemoteControlTargetPairingOutcome::NoActiveAttempt => {
                Err(ApproveRemoteControlTargetPairingFailure::NoActiveAttempt)
            }
            ApproveRemoteControlTargetPairingOutcome::AttemptMismatch { requested, active } => {
                Err(ApproveRemoteControlTargetPairingFailure::AttemptMismatch { requested, active })
            }
            ApproveRemoteControlTargetPairingOutcome::OfferPendingDispatch { attempt_id } => {
                Err(ApproveRemoteControlTargetPairingFailure::OfferPendingDispatch { attempt_id })
            }
            ApproveRemoteControlTargetPairingOutcome::AlreadyApproved { attempt_id } => {
                Err(ApproveRemoteControlTargetPairingFailure::AlreadyApproved { attempt_id })
            }
            ApproveRemoteControlTargetPairingOutcome::FinalizationInProgress { attempt_id } => {
                Err(ApproveRemoteControlTargetPairingFailure::FinalizationInProgress { attempt_id })
            }
        }
    }

    pub(crate) fn reject_remote_control_target_pairing_into<F>(
        &mut self,
        command: RejectRemoteControlTargetPairing,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> Result<RemoteControlTargetPairingRejection, RejectRemoteControlTargetPairingFailure>
    where
        F: FnMut(&mut [u8]),
    {
        match self
            .remote_control_target_pairing
            .reject(command.attempt_id, now)
        {
            RejectRemoteControlTargetPairingOutcome::Rejected { aborted } => {
                Ok(RemoteControlTargetPairingRejection {
                    retired_link: self.retire_remote_control_pairing_exchange_link(
                        aborted.context(),
                        interfaces,
                        fill_entropy,
                        sink,
                    ),
                    aborted,
                })
            }
            RejectRemoteControlTargetPairingOutcome::Expired { expired } => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlTargetPairingExpired {
                        aborted: expired,
                    },
                ));
                Err(RejectRemoteControlTargetPairingFailure::Expired {
                    retired_link: self.retire_remote_control_pairing_exchange_link(
                        expired.context(),
                        interfaces,
                        fill_entropy,
                        sink,
                    ),
                    expired,
                })
            }
            RejectRemoteControlTargetPairingOutcome::CompletionRetentionExpired { expired } => {
                sink(EngineReaction::Journaled(
                    crate::engine::Journaled::RemoteControlTargetPairingCompletionRetentionExpired {
                        attempt_id: expired.attempt_id(),
                    },
                ));
                Err(
                    RejectRemoteControlTargetPairingFailure::CompletionRetentionExpired {
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
            RejectRemoteControlTargetPairingOutcome::NoActiveAttempt => {
                Err(RejectRemoteControlTargetPairingFailure::NoActiveAttempt)
            }
            RejectRemoteControlTargetPairingOutcome::AttemptMismatch { requested, active } => {
                Err(RejectRemoteControlTargetPairingFailure::AttemptMismatch { requested, active })
            }
            RejectRemoteControlTargetPairingOutcome::OfferPendingDispatch { attempt_id } => {
                Err(RejectRemoteControlTargetPairingFailure::OfferPendingDispatch { attempt_id })
            }
            RejectRemoteControlTargetPairingOutcome::AlreadyApproved { attempt_id } => {
                Err(RejectRemoteControlTargetPairingFailure::AlreadyApproved { attempt_id })
            }
            RejectRemoteControlTargetPairingOutcome::FinalizationInProgress { attempt_id } => {
                Err(RejectRemoteControlTargetPairingFailure::FinalizationInProgress { attempt_id })
            }
        }
    }
}
