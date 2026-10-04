#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::tests::{RemoteControlPairingAttemptId, target};
use crate::{ControllerInstallation, LoadDevicesOutcome};
use core::num::NonZeroU32;
use personal_rns::identity::IdentityHash;
use personal_rns::remote_control::{RemoteControlPairingContext, RemoteControlPairingIdentity};

fn attempt(seed: u8) -> RemoteControlPairingAttemptId {
    RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([seed; 32])
}

fn completion(enrollment: Enrollment, seed: u8) -> CompleteEnrollment {
    CompleteEnrollment {
        enrollment,
        target: target(seed),
    }
}

fn begin(registry: &mut DeviceRegistry, device: DeviceId, seed: u8) -> Enrollment {
    let BeginEnrollmentOutcome::Started { enrollment } = registry
        .step(BeginEnrollment {
            device,
            target: target(seed),
            attempt: attempt(seed),
        })
        .unwrap()
    else {
        panic!("begin refused")
    };
    enrollment
}

fn pending(registry: &mut DeviceRegistry, seed: u8) -> Enrollment {
    let CreateDeviceOutcome::Created { device } = registry
        .step(CreateDevice {
            label: DeviceLabel::new("MCU").unwrap(),
        })
        .unwrap()
    else {
        panic!("create refused")
    };
    begin(registry, device, seed)
}

fn registry() -> DeviceRegistry {
    DeviceRegistry::try_new(NonZeroU32::new(8).unwrap()).unwrap()
}

fn persisted(seed: u8) -> Message<'static> {
    Message::RemoteControlControllerPairingAuthorizationPersisted {
        attempt_id: attempt(seed),
    }
}

fn context() -> RemoteControlPairingContext {
    RemoteControlPairingContext::new(
        RemoteControlPairingIdentity::new(IdentityHash::new([1; 16])).endpoint(),
        crate::tests::LINK,
    )
}

#[test]
fn matching_authorization_persists_before_success_and_duplicate_events_do_not_write() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    let mut registry = registry();
    let first = pending(&mut registry, 1);
    let second = pending(&mut registry, 2);
    let second_before = registry.step(ReadDevice {
        device: second.device(),
    });
    let mut coordinator = EnrollmentCoordinator::new(completion(first, 1));
    assert_eq!(
        coordinator
            .retry::<4>(&mut registry, &mut installation.devices)
            .unwrap(),
        EnrollmentCoordinatorOutcome::AwaitingAuthorization
    );
    assert_eq!(
        coordinator
            .receive::<4>(
                &Message::RemoteControlTargetPairingAuthorizationPersisted {
                    attempt_id: attempt(1)
                },
                &mut registry,
                &mut installation.devices
            )
            .unwrap(),
        EnrollmentCoordinatorOutcome::Unhandled
    );
    assert_eq!(
        coordinator
            .receive::<4>(&persisted(2), &mut registry, &mut installation.devices)
            .unwrap(),
        EnrollmentCoordinatorOutcome::UnrelatedAttempt {
            rejected: ObserveEnrollmentResult {
                attempt: attempt(2),
                result: EnrollmentProtocolResult::AuthorizationPersisted
            }
        }
    );
    assert!(!directory.path().join("devices.hopspot").exists());
    assert_eq!(
        coordinator
            .receive::<4>(&persisted(1), &mut registry, &mut installation.devices)
            .unwrap(),
        EnrollmentCoordinatorOutcome::Recorded { enrollment: first }
    );
    assert_eq!(
        coordinator.inspect(),
        EnrollmentSettlementSnapshot {
            enrollment: first,
            status: EnrollmentSettlementStatus::Recorded
        }
    );
    assert_eq!(
        registry.step(ReadDevice {
            device: second.device()
        }),
        second_before
    );
    let LoadDevicesOutcome::Loaded {
        registry: mut restored,
    } = installation
        .devices
        .load(NonZeroU32::new(8).unwrap())
        .unwrap()
    else {
        panic!("archive missing")
    };
    assert_eq!(
        restored.step(ReadRememberedDevices::<4>),
        registry.step(ReadRememberedDevices::<4>)
    );
    let path = directory.path().join("devices.hopspot");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        coordinator
            .receive::<4>(&persisted(1), &mut registry, &mut installation.devices)
            .unwrap(),
        EnrollmentCoordinatorOutcome::Settled
    );
    assert_eq!(
        coordinator
            .retry::<4>(&mut registry, &mut installation.devices)
            .unwrap(),
        EnrollmentCoordinatorOutcome::Settled
    );
    assert!(path.is_dir());
}

#[test]
fn write_failure_retains_authorization_across_abort_and_explicit_retry() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    let mut registry = registry();
    let enrollment = pending(&mut registry, 1);
    let before = registry.step(ReadDevice {
        device: enrollment.device(),
    });
    let mut coordinator = EnrollmentCoordinator::new(completion(enrollment, 1));
    let path = directory.path().join("devices.hopspot");
    std::fs::create_dir(&path).unwrap();
    let error = coordinator
        .receive::<4>(&persisted(1), &mut registry, &mut installation.devices)
        .unwrap_err();
    let EnrollmentCoordinatorError::Persistence(PersistEnrollmentError::Store {
        completion: rejected,
        source: crate::DeviceStoreError::Io(_),
    }) = error
    else {
        panic!("wrong error {error:?}")
    };
    assert_eq!(*rejected, completion(enrollment, 1));
    assert_eq!(
        registry.step(ReadDevice {
            device: enrollment.device()
        }),
        before
    );
    assert_eq!(
        coordinator.inspect(),
        EnrollmentSettlementSnapshot {
            enrollment,
            status: EnrollmentSettlementStatus::AwaitingPersistence
        }
    );
    for message in [
        persisted(1),
        Message::RemoteControlControllerPairingLinkClosed {
            aborted: RemoteControlControllerPairingAborted::AwaitingCompletion {
                attempt_id: attempt(1),
                context: context(),
            },
        },
        Message::RemoteControlControllerPairingAuthorizationPersistenceFailed {
            attempt_id: attempt(1),
        },
    ] {
        assert_eq!(
            coordinator
                .receive::<4>(&message, &mut registry, &mut installation.devices)
                .unwrap(),
            EnrollmentCoordinatorOutcome::AwaitingPersistence
        );
    }
    std::fs::remove_dir(&path).unwrap();
    assert_eq!(
        coordinator
            .retry::<4>(&mut registry, &mut installation.devices)
            .unwrap(),
        EnrollmentCoordinatorOutcome::Recorded { enrollment }
    );
}

#[test]
fn protocol_failures_keep_their_reasons_and_leave_other_devices_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    for (message, reason) in [
        (
            Message::RemoteControlControllerPairingAuthorizationPersistenceFailed {
                attempt_id: attempt(1),
            },
            EnrollmentFailure::PersistenceFailed,
        ),
        (
            Message::RemoteControlControllerPairingExpired {
                aborted: RemoteControlControllerPairingAborted::AwaitingApproval {
                    attempt_id: attempt(1),
                    context: context(),
                },
            },
            EnrollmentFailure::Expired,
        ),
        (
            Message::RemoteControlControllerPairingLinkClosed {
                aborted: RemoteControlControllerPairingAborted::AwaitingCompletion {
                    attempt_id: attempt(1),
                    context: context(),
                },
            },
            EnrollmentFailure::ConnectionLost,
        ),
    ] {
        let mut registry = registry();
        let enrollment = pending(&mut registry, 1);
        let other = pending(&mut registry, 2);
        let before = registry.step(ReadDevice {
            device: other.device(),
        });
        let mut coordinator = EnrollmentCoordinator::new(completion(enrollment, 1));
        assert_eq!(
            coordinator
                .receive::<4>(&message, &mut registry, &mut installation.devices)
                .unwrap(),
            EnrollmentCoordinatorOutcome::Failed { enrollment, reason }
        );
        assert_eq!(
            coordinator.inspect(),
            EnrollmentSettlementSnapshot {
                enrollment,
                status: EnrollmentSettlementStatus::Failed { reason }
            }
        );
        let ReadDeviceOutcome::Found { device } = registry.step(ReadDevice {
            device: enrollment.device(),
        }) else {
            panic!("missing device")
        };
        assert_eq!(device.enrollment, EnrollmentState::Planned);
        assert_eq!(
            registry.step(ReadDevice {
                device: other.device()
            }),
            before
        );
        assert_eq!(
            coordinator
                .receive::<4>(&persisted(1), &mut registry, &mut installation.devices)
                .unwrap(),
            EnrollmentCoordinatorOutcome::Settled
        );
        assert_eq!(
            coordinator
                .retry::<4>(&mut registry, &mut installation.devices)
                .unwrap(),
            EnrollmentCoordinatorOutcome::Settled
        );
    }
    assert!(!directory.path().join("devices.hopspot").exists());
    assert_eq!(
        protocol_result(&Message::RemoteControlControllerPairingExpired {
            aborted: RemoteControlControllerPairingAborted::AwaitingOffer { context: context() }
        }),
        None
    );
}

#[test]
fn stale_generation_and_forgotten_records_cannot_be_completed_or_failed_by_old_coordinators() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    for forgotten in [false, true] {
        for successful in [false, true] {
            let mut registry = registry();
            let old = pending(&mut registry, 1);
            let mut coordinator = EnrollmentCoordinator::new(completion(old, 1));
            let _cancelled = registry.step(CancelEnrollment { enrollment: old });
            let new = begin(&mut registry, old.device(), 1);
            let before = registry.step(ReadDevice {
                device: new.device(),
            });
            if forgotten {
                let _forgotten = registry.step(ForgetDevice {
                    device: old.device(),
                });
            }
            let message = if successful {
                persisted(1)
            } else {
                Message::RemoteControlControllerPairingAuthorizationPersistenceFailed {
                    attempt_id: attempt(1),
                }
            };
            assert_eq!(
                coordinator
                    .receive::<4>(&message, &mut registry, &mut installation.devices)
                    .unwrap(),
                if forgotten {
                    EnrollmentCoordinatorOutcome::MissingDevice { enrollment: old }
                } else {
                    EnrollmentCoordinatorOutcome::StaleEnrollment { enrollment: old }
                }
            );
            if !forgotten {
                assert_eq!(
                    registry.step(ReadDevice {
                        device: new.device()
                    }),
                    before
                );
            }
        }
    }
    assert!(!directory.path().join("devices.hopspot").exists());
}

#[test]
fn capacity_refusal_is_retryable_and_target_refusals_are_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    let mut registry = registry();
    let enrollment = pending(&mut registry, 1);
    let mut mismatch = EnrollmentCoordinator::new(completion(enrollment, 2));
    assert_eq!(
        mismatch
            .receive::<4>(&persisted(1), &mut registry, &mut installation.devices)
            .unwrap(),
        EnrollmentCoordinatorOutcome::TargetMismatch {
            rejected: completion(enrollment, 2)
        }
    );
    let duplicate = crate::tests::paired(&mut registry, target(1));
    let mut coordinator = EnrollmentCoordinator::new(completion(enrollment, 1));
    assert_eq!(
        coordinator
            .receive::<4>(&persisted(1), &mut registry, &mut installation.devices)
            .unwrap(),
        EnrollmentCoordinatorOutcome::TargetAlreadyPaired {
            rejected: completion(enrollment, 1),
            device: duplicate
        }
    );
    let _forgotten = registry.step(ForgetDevice { device: duplicate });
    let mut others = alloc::vec::Vec::new();
    for seed in 2..6 {
        others.push(pending(&mut registry, seed));
    }
    assert_eq!(
        coordinator
            .retry::<4>(&mut registry, &mut installation.devices)
            .unwrap(),
        EnrollmentCoordinatorOutcome::InsufficientCapacity {
            rejected: completion(enrollment, 1),
            required: 5
        }
    );
    let _forgotten = registry.step(ForgetDevice {
        device: others.first().unwrap().device(),
    });
    assert_eq!(
        coordinator
            .retry::<4>(&mut registry, &mut installation.devices)
            .unwrap(),
        EnrollmentCoordinatorOutcome::Recorded { enrollment }
    );
}

#[test]
fn unexpected_settlement_acknowledgment_preserves_the_invariant_outcome() {
    let mut registry = registry();
    let enrollment = pending(&mut registry, 1);
    let error = settle_recording(
        RecordEnrollmentPersistenceOutcome::AwaitingAuthorization,
        enrollment,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        EnrollmentCoordinatorError::SettlementInvariant {
            outcome: RecordEnrollmentPersistenceOutcome::AwaitingAuthorization
        }
    ));
}
