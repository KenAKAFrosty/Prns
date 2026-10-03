use super::*;

#[test]
fn two_devices_pair_independently_and_reject_replacement() {
    let mut registry = registry(2);
    let first = create(&mut registry, "First");
    let second = create(&mut registry, "Second");
    let first_attempt = begin(&mut registry, first, 1);
    let second_attempt = begin(&mut registry, second, 2);
    assert_eq!(first_attempt.device(), first);
    assert_eq!(first_attempt.attempt(), begin_input(first, 1).attempt);
    assert_eq!(
        registry.step(complete(second_attempt, 2)),
        CompleteEnrollmentOutcome::Recorded {
            enrollment: second_attempt
        }
    );
    assert_eq!(
        read(&mut registry, first).enrollment,
        EnrollmentState::Pairing {
            enrollment: first_attempt,
            target: *target(1).public_keys(),
        }
    );
    assert_eq!(
        registry.step(complete(first_attempt, 1)),
        CompleteEnrollmentOutcome::Recorded {
            enrollment: first_attempt
        }
    );
    assert_eq!(
        read(&mut registry, first).enrollment,
        EnrollmentState::Paired {
            target: *target(1).public_keys()
        }
    );
    assert_eq!(
        registry.step(complete(first_attempt, 1)),
        CompleteEnrollmentOutcome::StaleEnrollment {
            rejected: complete(first_attempt, 1)
        }
    );
    assert_eq!(
        registry.step(begin_input(first, 2)),
        Ok(BeginEnrollmentOutcome::AlreadyPaired {
            rejected: begin_input(first, 2)
        })
    );
    assert_eq!(
        registry.step(CancelEnrollment {
            enrollment: first_attempt
        }),
        CancelEnrollmentOutcome::StaleEnrollment {
            enrollment: first_attempt
        }
    );
    assert_eq!(
        registry.step(FailEnrollment {
            enrollment: first_attempt,
            reason: EnrollmentFailure::Expired
        }),
        FailEnrollmentOutcome::StaleEnrollment {
            rejected: FailEnrollment {
                enrollment: first_attempt,
                reason: EnrollmentFailure::Expired
            },
        }
    );
    assert_eq!(
        registry.step(ForgetDevice { device: first }),
        ForgetDeviceOutcome::Forgotten {
            connection: ConnectionState::NotConnected,
            device: first,
            enrollment: EnrollmentState::Paired {
                target: *target(1).public_keys()
            },
        }
    );
    assert_eq!(
        read(&mut registry, second).enrollment,
        EnrollmentState::Paired {
            target: *target(2).public_keys()
        }
    );
}

#[test]
fn cancellation_and_failure_preserve_reasons_and_reject_old_generations() {
    let mut registry = registry(1);
    let device = create(&mut registry, "Test");
    let old = begin(&mut registry, device, 1);
    assert_eq!(
        registry.step(begin_input(device, 1)),
        Ok(BeginEnrollmentOutcome::AlreadyPairing {
            rejected: begin_input(device, 1),
            active: old
        })
    );
    assert_eq!(
        registry.step(CancelEnrollment { enrollment: old }),
        CancelEnrollmentOutcome::Cancelled { enrollment: old }
    );
    assert_eq!(
        registry.step(CancelEnrollment { enrollment: old }),
        CancelEnrollmentOutcome::StaleEnrollment { enrollment: old }
    );
    assert_eq!(
        registry.step(complete(old, 1)),
        CompleteEnrollmentOutcome::StaleEnrollment {
            rejected: complete(old, 1)
        }
    );
    for reason in [
        EnrollmentFailure::Expired,
        EnrollmentFailure::Rejected,
        EnrollmentFailure::ConnectionLost,
        EnrollmentFailure::PersistenceFailed,
    ] {
        let active = begin(&mut registry, device, 1);
        assert_ne!(active, old);
        assert_eq!(
            registry.step(complete(old, 1)),
            CompleteEnrollmentOutcome::StaleEnrollment {
                rejected: complete(old, 1)
            }
        );
        assert_eq!(
            registry.step(CancelEnrollment { enrollment: old }),
            CancelEnrollmentOutcome::StaleEnrollment { enrollment: old }
        );
        assert_eq!(
            registry.step(FailEnrollment {
                enrollment: old,
                reason
            }),
            FailEnrollmentOutcome::StaleEnrollment {
                rejected: FailEnrollment {
                    enrollment: old,
                    reason
                }
            }
        );
        assert_eq!(
            read(&mut registry, device).enrollment,
            EnrollmentState::Pairing {
                enrollment: active,
                target: *target(1).public_keys()
            }
        );
        assert_eq!(
            registry.step(FailEnrollment {
                enrollment: active,
                reason
            }),
            FailEnrollmentOutcome::Failed {
                enrollment: active,
                reason
            }
        );
        assert_eq!(
            read(&mut registry, device).enrollment,
            EnrollmentState::Planned
        );
        assert_eq!(
            registry.step(FailEnrollment {
                enrollment: active,
                reason
            }),
            FailEnrollmentOutcome::StaleEnrollment {
                rejected: FailEnrollment {
                    enrollment: active,
                    reason
                }
            }
        );
    }
}

#[test]
fn wrong_target_and_duplicate_binding_leave_attempts_unchanged() {
    let mut registry = registry(2);
    let first = create(&mut registry, "First");
    let second = create(&mut registry, "Second");
    let first_attempt = begin(&mut registry, first, 1);
    let second_attempt = begin(&mut registry, second, 1);
    assert_eq!(
        registry.step(complete(first_attempt, 2)),
        CompleteEnrollmentOutcome::TargetMismatch {
            rejected: complete(first_attempt, 2)
        }
    );
    assert_eq!(
        read(&mut registry, first).enrollment,
        EnrollmentState::Pairing {
            enrollment: first_attempt,
            target: *target(1).public_keys()
        }
    );
    assert_eq!(
        registry.step(complete(first_attempt, 1)),
        CompleteEnrollmentOutcome::Recorded {
            enrollment: first_attempt
        }
    );
    assert_eq!(
        registry.step(complete(second_attempt, 1)),
        CompleteEnrollmentOutcome::TargetAlreadyPaired {
            rejected: complete(second_attempt, 1),
            device: first
        }
    );
    assert_eq!(
        read(&mut registry, second).enrollment,
        EnrollmentState::Pairing {
            enrollment: second_attempt,
            target: *target(1).public_keys()
        }
    );
}

#[test]
fn forgetting_invalidates_every_enrollment_step_even_after_row_reuse() {
    let mut registry = registry(1);
    let old = create(&mut registry, "Old");
    let enrollment = begin(&mut registry, old, 1);
    assert_eq!(
        registry.step(ForgetDevice { device: old }),
        ForgetDeviceOutcome::Forgotten {
            connection: ConnectionState::NotConnected,
            device: old,
            enrollment: EnrollmentState::Pairing {
                enrollment,
                target: *target(1).public_keys()
            },
        }
    );
    let new = create(&mut registry, "New");
    assert_eq!(
        registry.step(complete(enrollment, 1)),
        CompleteEnrollmentOutcome::MissingDevice {
            rejected: complete(enrollment, 1)
        }
    );
    assert_eq!(
        registry.step(begin_input(old, 1)),
        Ok(BeginEnrollmentOutcome::MissingDevice {
            rejected: begin_input(old, 1)
        })
    );
    assert_eq!(
        registry.step(CancelEnrollment { enrollment }),
        CancelEnrollmentOutcome::MissingDevice { enrollment }
    );
    assert_eq!(
        registry.step(FailEnrollment {
            enrollment,
            reason: EnrollmentFailure::ConnectionLost
        }),
        FailEnrollmentOutcome::MissingDevice {
            rejected: FailEnrollment {
                enrollment,
                reason: EnrollmentFailure::ConnectionLost
            },
        }
    );
    assert_eq!(
        read(&mut registry, new).enrollment,
        EnrollmentState::Planned
    );
}

#[test]
fn enrollment_generations_exhaust_without_wrapping_or_changing_state() {
    let mut registry = registry(1);
    let device = create(&mut registry, "Last");
    registry.next_enrollment = Some(NonZeroU64::MAX);
    let last = begin(&mut registry, device, 1);
    assert_eq!(
        registry.step(CancelEnrollment { enrollment: last }),
        CancelEnrollmentOutcome::Cancelled { enrollment: last }
    );
    assert_eq!(
        registry.step(begin_input(device, 1)),
        Err(BeginEnrollmentError::IdentifiersExhausted {
            rejected: begin_input(device, 1)
        })
    );
    assert_eq!(
        read(&mut registry, device).enrollment,
        EnrollmentState::Planned
    );
}
