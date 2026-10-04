use crate::domain_primitives::{Enrollment, EnrollmentFailure};
use crate::state_machines::{CompleteEnrollment, FailEnrollment};
use pipecircuit::{StateMachine, StepInputOf};
use prns_core::remote_control::{RemoteControlPairingAttemptId, RemoteControlTargetIdentity};

#[cfg(test)]
mod architecture;
#[cfg(test)]
mod behavior;
#[cfg(test)]
mod tests;

pub struct EnrollmentSettlement {
    completion: CompleteEnrollment,
    status: EnrollmentSettlementStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentSettlementStatus {
    AwaitingAuthorization,
    AwaitingPersistence,
    Recorded,
    Failed { reason: EnrollmentFailure },
}

impl EnrollmentSettlement {
    pub fn new(completion: CompleteEnrollment) -> Self {
        Self {
            completion,
            status: EnrollmentSettlementStatus::AwaitingAuthorization,
        }
    }

    fn completion(&self) -> CompleteEnrollment {
        CompleteEnrollment {
            enrollment: self.completion.enrollment,
            target: RemoteControlTargetIdentity::new(*self.completion.target.public_keys()),
        }
    }
}

impl StateMachine for EnrollmentSettlement {}

#[derive(Debug, PartialEq, Eq)]
pub enum EnrollmentProtocolResult {
    AuthorizationPersisted,
    Failed { reason: EnrollmentFailure },
}

#[derive(Debug, PartialEq, Eq)]
pub struct ObserveEnrollmentResult {
    pub attempt: RemoteControlPairingAttemptId,
    pub result: EnrollmentProtocolResult,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum ObserveEnrollmentResultOutcome {
    Persist { completion: CompleteEnrollment },
    Fail { failure: FailEnrollment },
    UnrelatedAttempt { rejected: ObserveEnrollmentResult },
    AwaitingPersistence,
    Settled,
}

impl StepInputOf<EnrollmentSettlement> for ObserveEnrollmentResult {
    type Outcome = ObserveEnrollmentResultOutcome;

    fn step(self, settlement: &mut EnrollmentSettlement) -> Self::Outcome {
        if self.attempt != settlement.completion.enrollment.attempt() {
            return Self::Outcome::UnrelatedAttempt { rejected: self };
        }
        match settlement.status {
            EnrollmentSettlementStatus::AwaitingAuthorization => {}
            EnrollmentSettlementStatus::AwaitingPersistence => {
                return Self::Outcome::AwaitingPersistence;
            }
            EnrollmentSettlementStatus::Recorded | EnrollmentSettlementStatus::Failed { .. } => {
                return Self::Outcome::Settled;
            }
        }
        match self.result {
            EnrollmentProtocolResult::AuthorizationPersisted => {
                settlement.status = EnrollmentSettlementStatus::AwaitingPersistence;
                Self::Outcome::Persist {
                    completion: settlement.completion(),
                }
            }
            EnrollmentProtocolResult::Failed { reason } => {
                settlement.status = EnrollmentSettlementStatus::Failed { reason };
                Self::Outcome::Fail {
                    failure: FailEnrollment {
                        enrollment: settlement.completion.enrollment,
                        reason,
                    },
                }
            }
        }
    }
}

pub struct RetryEnrollmentPersistence;

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum RetryEnrollmentPersistenceOutcome {
    Persist { completion: CompleteEnrollment },
    AwaitingAuthorization,
    Settled,
}

impl StepInputOf<EnrollmentSettlement> for RetryEnrollmentPersistence {
    type Outcome = RetryEnrollmentPersistenceOutcome;

    fn step(self, settlement: &mut EnrollmentSettlement) -> Self::Outcome {
        match settlement.status {
            EnrollmentSettlementStatus::AwaitingAuthorization => {
                Self::Outcome::AwaitingAuthorization
            }
            EnrollmentSettlementStatus::AwaitingPersistence => Self::Outcome::Persist {
                completion: settlement.completion(),
            },
            EnrollmentSettlementStatus::Recorded | EnrollmentSettlementStatus::Failed { .. } => {
                Self::Outcome::Settled
            }
        }
    }
}

pub struct RecordEnrollmentPersistence;

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum RecordEnrollmentPersistenceOutcome {
    Recorded,
    AlreadyRecorded,
    AwaitingAuthorization,
    Failed { reason: EnrollmentFailure },
}

impl StepInputOf<EnrollmentSettlement> for RecordEnrollmentPersistence {
    type Outcome = RecordEnrollmentPersistenceOutcome;

    fn step(self, settlement: &mut EnrollmentSettlement) -> Self::Outcome {
        match settlement.status {
            EnrollmentSettlementStatus::AwaitingAuthorization => {
                Self::Outcome::AwaitingAuthorization
            }
            EnrollmentSettlementStatus::AwaitingPersistence => {
                settlement.status = EnrollmentSettlementStatus::Recorded;
                Self::Outcome::Recorded
            }
            EnrollmentSettlementStatus::Recorded => Self::Outcome::AlreadyRecorded,
            EnrollmentSettlementStatus::Failed { reason } => Self::Outcome::Failed { reason },
        }
    }
}

pub struct ReadEnrollmentSettlement;

#[derive(Debug, PartialEq, Eq)]
pub struct EnrollmentSettlementSnapshot {
    pub enrollment: Enrollment,
    pub status: EnrollmentSettlementStatus,
}

impl StepInputOf<EnrollmentSettlement> for ReadEnrollmentSettlement {
    type Outcome = EnrollmentSettlementSnapshot;

    fn step(self, settlement: &mut EnrollmentSettlement) -> Self::Outcome {
        EnrollmentSettlementSnapshot {
            enrollment: settlement.completion.enrollment,
            status: settlement.status,
        }
    }
}
