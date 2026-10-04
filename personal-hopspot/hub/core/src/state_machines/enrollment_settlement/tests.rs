#![allow(clippy::unwrap_used, clippy::panic)]

extern crate alloc;

use super::*;
use crate::domain_primitives::DeviceLabel;
use crate::state_machines::{
    BeginEnrollment, BeginEnrollmentOutcome, CreateDevice, CreateDeviceOutcome, DeviceRegistry,
};
use core::num::NonZeroU32;
use prns_core::crypto::{Ed25519PublicKey, X25519PublicKey};
use prns_core::identity::{
    IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
};
use proptest::prelude::*;

fn attempt(seed: u8) -> RemoteControlPairingAttemptId {
    RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([seed; 32])
}

fn completion(enrollment: Enrollment) -> CompleteEnrollment {
    CompleteEnrollment {
        enrollment,
        target: RemoteControlTargetIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([7; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([8; 32])),
        }),
    }
}

fn setup() -> (Enrollment, EnrollmentSettlement) {
    let mut registry = DeviceRegistry::try_new(NonZeroU32::MIN).unwrap();
    let CreateDeviceOutcome::Created { device } = registry
        .step(CreateDevice {
            label: DeviceLabel::new("MCU").unwrap(),
        })
        .unwrap()
    else {
        panic!("create refused")
    };
    let BeginEnrollmentOutcome::Started { enrollment } = registry
        .step(BeginEnrollment {
            device,
            attempt: attempt(1),
            target: RemoteControlTargetIdentity::new(IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([7; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([8; 32])),
            }),
        })
        .unwrap()
    else {
        panic!("begin refused")
    };
    (
        enrollment,
        EnrollmentSettlement::new(completion(enrollment)),
    )
}

fn observed(seed: u8, result: EnrollmentProtocolResult) -> ObserveEnrollmentResult {
    ObserveEnrollmentResult {
        attempt: attempt(seed),
        result,
    }
}

#[test]
fn authorization_latches_before_retry_and_terminal_recording_is_idempotent() {
    let (enrollment, mut settlement) = setup();
    assert_eq!(
        settlement.step(ReadEnrollmentSettlement),
        EnrollmentSettlementSnapshot {
            enrollment,
            status: EnrollmentSettlementStatus::AwaitingAuthorization
        }
    );
    assert_eq!(
        settlement.step(RetryEnrollmentPersistence),
        RetryEnrollmentPersistenceOutcome::AwaitingAuthorization
    );
    assert_eq!(
        settlement.step(RecordEnrollmentPersistence),
        RecordEnrollmentPersistenceOutcome::AwaitingAuthorization
    );
    assert_eq!(
        settlement.step(observed(
            2,
            EnrollmentProtocolResult::AuthorizationPersisted
        )),
        ObserveEnrollmentResultOutcome::UnrelatedAttempt {
            rejected: observed(2, EnrollmentProtocolResult::AuthorizationPersisted)
        }
    );
    assert_eq!(
        settlement.step(observed(
            1,
            EnrollmentProtocolResult::AuthorizationPersisted
        )),
        ObserveEnrollmentResultOutcome::Persist {
            completion: completion(enrollment)
        }
    );
    for result in [
        EnrollmentProtocolResult::AuthorizationPersisted,
        EnrollmentProtocolResult::Failed {
            reason: EnrollmentFailure::ConnectionLost,
        },
    ] {
        assert_eq!(
            settlement.step(observed(1, result)),
            ObserveEnrollmentResultOutcome::AwaitingPersistence
        );
    }
    assert_eq!(
        settlement.step(ReadEnrollmentSettlement),
        EnrollmentSettlementSnapshot {
            enrollment,
            status: EnrollmentSettlementStatus::AwaitingPersistence
        }
    );
    assert_eq!(
        settlement.step(RetryEnrollmentPersistence),
        RetryEnrollmentPersistenceOutcome::Persist {
            completion: completion(enrollment)
        }
    );
    assert_eq!(
        settlement.step(RetryEnrollmentPersistence),
        RetryEnrollmentPersistenceOutcome::Persist {
            completion: completion(enrollment)
        }
    );
    assert_eq!(
        settlement.step(RecordEnrollmentPersistence),
        RecordEnrollmentPersistenceOutcome::Recorded
    );
    assert_eq!(
        settlement.step(RecordEnrollmentPersistence),
        RecordEnrollmentPersistenceOutcome::AlreadyRecorded
    );
    assert_eq!(
        settlement.step(RetryEnrollmentPersistence),
        RetryEnrollmentPersistenceOutcome::Settled
    );
    assert_eq!(
        settlement.step(observed(
            1,
            EnrollmentProtocolResult::Failed {
                reason: EnrollmentFailure::Expired
            }
        )),
        ObserveEnrollmentResultOutcome::Settled
    );
    assert_eq!(
        settlement.step(ReadEnrollmentSettlement),
        EnrollmentSettlementSnapshot {
            enrollment,
            status: EnrollmentSettlementStatus::Recorded
        }
    );
}

#[test]
fn protocol_failure_preserves_every_reason_and_never_enables_persistence() {
    for reason in [
        EnrollmentFailure::Expired,
        EnrollmentFailure::Rejected,
        EnrollmentFailure::ConnectionLost,
        EnrollmentFailure::PersistenceFailed,
    ] {
        let (enrollment, mut settlement) = setup();
        assert_eq!(
            settlement.step(observed(1, EnrollmentProtocolResult::Failed { reason })),
            ObserveEnrollmentResultOutcome::Fail {
                failure: FailEnrollment { enrollment, reason }
            }
        );
        assert_eq!(
            settlement.step(ReadEnrollmentSettlement),
            EnrollmentSettlementSnapshot {
                enrollment,
                status: EnrollmentSettlementStatus::Failed { reason }
            }
        );
        assert_eq!(
            settlement.step(RetryEnrollmentPersistence),
            RetryEnrollmentPersistenceOutcome::Settled
        );
        assert_eq!(
            settlement.step(RecordEnrollmentPersistence),
            RecordEnrollmentPersistenceOutcome::Failed { reason }
        );
        assert_eq!(
            settlement.step(observed(
                1,
                EnrollmentProtocolResult::AuthorizationPersisted
            )),
            ObserveEnrollmentResultOutcome::Settled
        );
        assert_eq!(
            settlement.step(observed(2, EnrollmentProtocolResult::Failed { reason })),
            ObserveEnrollmentResultOutcome::UnrelatedAttempt {
                rejected: observed(2, EnrollmentProtocolResult::Failed { reason })
            }
        );
    }
}

proptest! {
    #[test]
    fn arbitrary_event_retry_and_acknowledgment_histories_match_the_phase_model(history in prop::collection::vec((0_u8..5, any::<bool>()), 0..100)) {
        let (enrollment, mut settlement) = setup();
        let mut expected = EnrollmentSettlementStatus::AwaitingAuthorization;
        for (operation, matching) in history {
            match operation {
                0 | 1 => {
                    let seed = if matching { 1 } else { 2 };
                    let result = if operation == 0 { EnrollmentProtocolResult::AuthorizationPersisted } else { EnrollmentProtocolResult::Failed { reason: EnrollmentFailure::Rejected } };
                    let expected_outcome = if !matching {
                        ObserveEnrollmentResultOutcome::UnrelatedAttempt { rejected: observed(seed, if operation == 0 { EnrollmentProtocolResult::AuthorizationPersisted } else { EnrollmentProtocolResult::Failed { reason: EnrollmentFailure::Rejected } }) }
                    } else {
                        match expected {
                            EnrollmentSettlementStatus::AwaitingAuthorization if operation == 0 => {
                                expected = EnrollmentSettlementStatus::AwaitingPersistence;
                                ObserveEnrollmentResultOutcome::Persist { completion: completion(enrollment) }
                            }
                            EnrollmentSettlementStatus::AwaitingAuthorization => {
                                expected = EnrollmentSettlementStatus::Failed { reason: EnrollmentFailure::Rejected };
                                ObserveEnrollmentResultOutcome::Fail { failure: FailEnrollment { enrollment, reason: EnrollmentFailure::Rejected } }
                            }
                            EnrollmentSettlementStatus::AwaitingPersistence => ObserveEnrollmentResultOutcome::AwaitingPersistence,
                            EnrollmentSettlementStatus::Recorded | EnrollmentSettlementStatus::Failed { .. } => ObserveEnrollmentResultOutcome::Settled,
                        }
                    };
                    prop_assert_eq!(settlement.step(observed(seed, result)), expected_outcome);
                }
                2 => {
                    let expected_outcome = match expected {
                        EnrollmentSettlementStatus::AwaitingAuthorization => RetryEnrollmentPersistenceOutcome::AwaitingAuthorization,
                        EnrollmentSettlementStatus::AwaitingPersistence => RetryEnrollmentPersistenceOutcome::Persist { completion: completion(enrollment) },
                        EnrollmentSettlementStatus::Recorded | EnrollmentSettlementStatus::Failed { .. } => RetryEnrollmentPersistenceOutcome::Settled,
                    };
                    prop_assert_eq!(settlement.step(RetryEnrollmentPersistence), expected_outcome);
                }
                3 => {
                    let expected_outcome = match expected {
                        EnrollmentSettlementStatus::AwaitingAuthorization => RecordEnrollmentPersistenceOutcome::AwaitingAuthorization,
                        EnrollmentSettlementStatus::AwaitingPersistence => { expected = EnrollmentSettlementStatus::Recorded; RecordEnrollmentPersistenceOutcome::Recorded },
                        EnrollmentSettlementStatus::Recorded => RecordEnrollmentPersistenceOutcome::AlreadyRecorded,
                        EnrollmentSettlementStatus::Failed { reason } => RecordEnrollmentPersistenceOutcome::Failed { reason },
                    };
                    prop_assert_eq!(settlement.step(RecordEnrollmentPersistence), expected_outcome);
                }
                _ => {},
            }
            prop_assert_eq!(settlement.step(ReadEnrollmentSettlement), EnrollmentSettlementSnapshot { enrollment, status: expected });
        }
    }
}
