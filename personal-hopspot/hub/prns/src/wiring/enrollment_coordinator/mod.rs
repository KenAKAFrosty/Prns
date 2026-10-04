use crate::{DeviceStore, PersistEnrollmentError, PersistEnrollmentOutcome};
use hopspot_hub_core::*;
use personal_rns::remote_control::RemoteControlControllerPairingAborted;
use personal_rns::runtime::Message;
use pipecircuit::StateMachine;

#[cfg(test)]
mod behavior;
#[cfg(test)]
mod tests;

pub struct EnrollmentCoordinator {
    settlement: EnrollmentSettlement,
}

#[derive(Debug)]
pub enum EnrollmentCoordinatorError {
    Persistence(PersistEnrollmentError),
    SettlementInvariant {
        outcome: RecordEnrollmentPersistenceOutcome,
    },
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum EnrollmentCoordinatorOutcome {
    Unhandled,
    UnrelatedAttempt {
        rejected: ObserveEnrollmentResult,
    },
    AwaitingAuthorization,
    AwaitingPersistence,
    Settled,
    Recorded {
        enrollment: Enrollment,
    },
    Failed {
        enrollment: Enrollment,
        reason: EnrollmentFailure,
    },
    MissingDevice {
        enrollment: Enrollment,
    },
    StaleEnrollment {
        enrollment: Enrollment,
    },
    TargetMismatch {
        rejected: CompleteEnrollment,
    },
    TargetAlreadyPaired {
        rejected: CompleteEnrollment,
        device: DeviceId,
    },
    InsufficientCapacity {
        rejected: CompleteEnrollment,
        required: usize,
    },
}

impl EnrollmentCoordinator {
    pub fn new(completion: CompleteEnrollment) -> Self {
        Self {
            settlement: EnrollmentSettlement::new(completion),
        }
    }

    pub fn inspect(&mut self) -> EnrollmentSettlementSnapshot {
        self.settlement.step(ReadEnrollmentSettlement)
    }

    pub fn receive<const CAPACITY: usize>(
        &mut self,
        message: &Message<'_>,
        registry: &mut DeviceRegistry,
        store: &mut DeviceStore,
    ) -> Result<EnrollmentCoordinatorOutcome, EnrollmentCoordinatorError> {
        let Some(input) = protocol_result(message) else {
            return Ok(EnrollmentCoordinatorOutcome::Unhandled);
        };
        self.observe::<CAPACITY>(input, registry, store)
    }

    pub fn observe<const CAPACITY: usize>(
        &mut self,
        input: ObserveEnrollmentResult,
        registry: &mut DeviceRegistry,
        store: &mut DeviceStore,
    ) -> Result<EnrollmentCoordinatorOutcome, EnrollmentCoordinatorError> {
        match self.settlement.step(input) {
            ObserveEnrollmentResultOutcome::Persist { completion } => {
                self.persist::<CAPACITY>(completion, registry, store)
            }
            ObserveEnrollmentResultOutcome::Fail { failure } => {
                Ok(settle_failure(registry.step(failure)))
            }
            ObserveEnrollmentResultOutcome::UnrelatedAttempt { rejected } => {
                Ok(EnrollmentCoordinatorOutcome::UnrelatedAttempt { rejected })
            }
            ObserveEnrollmentResultOutcome::AwaitingPersistence => {
                Ok(EnrollmentCoordinatorOutcome::AwaitingPersistence)
            }
            ObserveEnrollmentResultOutcome::Settled => Ok(EnrollmentCoordinatorOutcome::Settled),
        }
    }

    pub fn retry<const CAPACITY: usize>(
        &mut self,
        registry: &mut DeviceRegistry,
        store: &mut DeviceStore,
    ) -> Result<EnrollmentCoordinatorOutcome, EnrollmentCoordinatorError> {
        match self.settlement.step(RetryEnrollmentPersistence) {
            RetryEnrollmentPersistenceOutcome::Persist { completion } => {
                self.persist::<CAPACITY>(completion, registry, store)
            }
            RetryEnrollmentPersistenceOutcome::AwaitingAuthorization => {
                Ok(EnrollmentCoordinatorOutcome::AwaitingAuthorization)
            }
            RetryEnrollmentPersistenceOutcome::Settled => Ok(EnrollmentCoordinatorOutcome::Settled),
        }
    }

    fn persist<const CAPACITY: usize>(
        &mut self,
        completion: CompleteEnrollment,
        registry: &mut DeviceRegistry,
        store: &mut DeviceStore,
    ) -> Result<EnrollmentCoordinatorOutcome, EnrollmentCoordinatorError> {
        store
            .persist_enrollment::<CAPACITY>(registry, completion)
            .map_err(EnrollmentCoordinatorError::Persistence)
            .and_then(|outcome| self.persisted(outcome))
    }

    fn persisted(
        &mut self,
        outcome: PersistEnrollmentOutcome,
    ) -> Result<EnrollmentCoordinatorOutcome, EnrollmentCoordinatorError> {
        match outcome {
            PersistEnrollmentOutcome::Recorded { enrollment } => settle_recording(
                self.settlement.step(RecordEnrollmentPersistence),
                enrollment,
            ),
            PersistEnrollmentOutcome::MissingDevice { rejected } => {
                Ok(EnrollmentCoordinatorOutcome::MissingDevice {
                    enrollment: rejected.enrollment,
                })
            }
            PersistEnrollmentOutcome::StaleEnrollment { rejected } => {
                Ok(EnrollmentCoordinatorOutcome::StaleEnrollment {
                    enrollment: rejected.enrollment,
                })
            }
            PersistEnrollmentOutcome::TargetMismatch { rejected } => {
                Ok(EnrollmentCoordinatorOutcome::TargetMismatch { rejected })
            }
            PersistEnrollmentOutcome::TargetAlreadyPaired { rejected, device } => {
                Ok(EnrollmentCoordinatorOutcome::TargetAlreadyPaired { rejected, device })
            }
            PersistEnrollmentOutcome::InsufficientCapacity { rejected, required } => {
                Ok(EnrollmentCoordinatorOutcome::InsufficientCapacity { rejected, required })
            }
        }
    }
}

fn settle_recording(
    outcome: RecordEnrollmentPersistenceOutcome,
    enrollment: Enrollment,
) -> Result<EnrollmentCoordinatorOutcome, EnrollmentCoordinatorError> {
    match outcome {
        RecordEnrollmentPersistenceOutcome::Recorded => {
            Ok(EnrollmentCoordinatorOutcome::Recorded { enrollment })
        }
        outcome @ (RecordEnrollmentPersistenceOutcome::AlreadyRecorded
        | RecordEnrollmentPersistenceOutcome::AwaitingAuthorization
        | RecordEnrollmentPersistenceOutcome::Failed { .. }) => {
            Err(EnrollmentCoordinatorError::SettlementInvariant { outcome })
        }
    }
}

fn settle_failure(outcome: FailEnrollmentOutcome) -> EnrollmentCoordinatorOutcome {
    match outcome {
        FailEnrollmentOutcome::Failed { enrollment, reason } => {
            EnrollmentCoordinatorOutcome::Failed { enrollment, reason }
        }
        FailEnrollmentOutcome::MissingDevice { rejected } => {
            EnrollmentCoordinatorOutcome::MissingDevice {
                enrollment: rejected.enrollment,
            }
        }
        FailEnrollmentOutcome::StaleEnrollment { rejected } => {
            EnrollmentCoordinatorOutcome::StaleEnrollment {
                enrollment: rejected.enrollment,
            }
        }
    }
}

#[expect(clippy::wildcard_enum_match_arm)]
pub(super) fn protocol_result(message: &Message<'_>) -> Option<ObserveEnrollmentResult> {
    match message {
        Message::RemoteControlControllerPairingAuthorizationPersisted { attempt_id } => {
            Some(ObserveEnrollmentResult {
                attempt: *attempt_id,
                result: EnrollmentProtocolResult::AuthorizationPersisted,
            })
        }
        Message::RemoteControlControllerPairingAuthorizationPersistenceFailed { attempt_id } => {
            Some(ObserveEnrollmentResult {
                attempt: *attempt_id,
                result: EnrollmentProtocolResult::Failed {
                    reason: EnrollmentFailure::PersistenceFailed,
                },
            })
        }
        Message::RemoteControlControllerPairingExpired { aborted } => {
            aborted_result(*aborted, EnrollmentFailure::Expired)
        }
        Message::RemoteControlControllerPairingLinkClosed { aborted } => {
            aborted_result(*aborted, EnrollmentFailure::ConnectionLost)
        }
        _ => None,
    }
}

fn aborted_result(
    aborted: RemoteControlControllerPairingAborted,
    reason: EnrollmentFailure,
) -> Option<ObserveEnrollmentResult> {
    match aborted {
        RemoteControlControllerPairingAborted::AwaitingOffer { .. } => None,
        RemoteControlControllerPairingAborted::AwaitingApproval { attempt_id, .. }
        | RemoteControlControllerPairingAborted::AwaitingCompletion { attempt_id, .. } => {
            Some(ObserveEnrollmentResult {
                attempt: attempt_id,
                result: EnrollmentProtocolResult::Failed { reason },
            })
        }
    }
}
