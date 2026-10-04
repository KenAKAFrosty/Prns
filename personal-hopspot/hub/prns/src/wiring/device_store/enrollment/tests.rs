#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::ControllerInstallation;
use crate::tests::{RemoteControlPairingAttemptId, paired, target};

fn pending(registry: &mut DeviceRegistry, seed: u8) -> Enrollment {
    let CreateDeviceOutcome::Created { device } = registry
        .step(CreateDevice {
            label: DeviceLabel::new("Pending MCU").unwrap(),
        })
        .unwrap()
    else {
        panic!("creation refused")
    };
    let BeginEnrollmentOutcome::Started { enrollment } = registry
        .step(BeginEnrollment {
            device,
            target: target(seed),
            attempt: RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([seed; 32]),
        })
        .unwrap()
    else {
        panic!("begin refused")
    };
    enrollment
}

fn completion(enrollment: Enrollment, seed: u8) -> CompleteEnrollment {
    CompleteEnrollment {
        enrollment,
        target: target(seed),
    }
}

#[test]
fn persisted_completion_survives_restart_and_retains_other_records() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    let mut registry = DeviceRegistry::try_new(NonZeroU32::new(8).unwrap()).unwrap();
    let _paired = paired(&mut registry, target(9));
    let enrollment = pending(&mut registry, 1);
    let _other = pending(&mut registry, 2);
    assert_eq!(
        installation
            .devices
            .persist_enrollment::<4>(&mut registry, completion(enrollment, 1))
            .unwrap(),
        PersistEnrollmentOutcome::Recorded { enrollment }
    );
    let expected = registry.step(ReadRememberedDevices::<4>);
    drop(installation);
    let installation = ControllerInstallation::open(directory.path()).unwrap();
    let LoadDevicesOutcome::Loaded { mut registry } = installation
        .devices
        .load(NonZeroU32::new(8).unwrap())
        .unwrap()
    else {
        panic!("archive missing")
    };
    assert_eq!(registry.step(ReadRememberedDevices::<4>), expected);
}

#[test]
fn refusals_preserve_the_command_and_do_not_touch_storage() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    let mut registry = DeviceRegistry::try_new(NonZeroU32::new(8).unwrap()).unwrap();
    let enrollment = pending(&mut registry, 1);
    assert_eq!(
        installation
            .devices
            .persist_enrollment::<4>(&mut registry, completion(enrollment, 2))
            .unwrap(),
        PersistEnrollmentOutcome::TargetMismatch {
            rejected: completion(enrollment, 2)
        }
    );
    let _cancelled = registry.step(CancelEnrollment { enrollment });
    assert_eq!(
        installation
            .devices
            .persist_enrollment::<4>(&mut registry, completion(enrollment, 1))
            .unwrap(),
        PersistEnrollmentOutcome::StaleEnrollment {
            rejected: completion(enrollment, 1)
        }
    );
    let _forgotten = registry.step(ForgetDevice {
        device: enrollment.device(),
    });
    assert_eq!(
        installation
            .devices
            .persist_enrollment::<4>(&mut registry, completion(enrollment, 1))
            .unwrap(),
        PersistEnrollmentOutcome::MissingDevice {
            rejected: completion(enrollment, 1)
        }
    );
    let enrollment = pending(&mut registry, 1);
    let device = paired(&mut registry, target(1));
    assert_eq!(
        installation
            .devices
            .persist_enrollment::<4>(&mut registry, completion(enrollment, 1))
            .unwrap(),
        PersistEnrollmentOutcome::TargetAlreadyPaired {
            rejected: completion(enrollment, 1),
            device
        }
    );
    let enrollment = pending(&mut registry, 3);
    let _fourth = pending(&mut registry, 4);
    let _fifth = pending(&mut registry, 5);
    assert_eq!(
        installation
            .devices
            .persist_enrollment::<4>(&mut registry, completion(enrollment, 3))
            .unwrap(),
        PersistEnrollmentOutcome::InsufficientCapacity {
            rejected: completion(enrollment, 3),
            required: 5
        }
    );
    assert!(!directory.path().join(DEVICES_FILE).exists());
}

#[test]
fn failed_write_leaves_enrollment_pending_and_can_be_retried() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    let mut registry = DeviceRegistry::try_new(NonZeroU32::new(4).unwrap()).unwrap();
    let enrollment = pending(&mut registry, 1);
    let before = registry.step(ReadDevice {
        device: enrollment.device(),
    });
    let path = directory.path().join(DEVICES_FILE);
    std::fs::create_dir(&path).unwrap();
    let error = installation
        .devices
        .persist_enrollment::<4>(&mut registry, completion(enrollment, 1))
        .unwrap_err();
    let PersistEnrollmentError::Store {
        completion: rejected,
        source: DeviceStoreError::Io(_),
    } = error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(*rejected, completion(enrollment, 1));
    assert_eq!(
        registry.step(ReadDevice {
            device: enrollment.device()
        }),
        before
    );
    assert!(path.is_dir());
    std::fs::remove_dir(path).unwrap();
    assert_eq!(
        installation
            .devices
            .persist_enrollment::<4>(&mut registry, *rejected)
            .unwrap(),
        PersistEnrollmentOutcome::Recorded { enrollment }
    );
}

fn publish_without_confirmation(
    store: &mut DeviceStore,
    devices: &[RememberedDevice],
) -> Result<(), DeviceStoreError> {
    let mut staged = NamedTempFile::new_in(&store.directory)?;
    stage_snapshot(
        staged.as_file_mut(),
        &codec::encode(devices),
        File::sync_all,
    )?;
    store.publish(staged, |_| {
        Err(std::io::Error::other("directory sync failed"))
    })
}

#[test]
fn uncertain_publication_preserves_pending_memory_and_exact_retry_input() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    let mut registry = DeviceRegistry::try_new(NonZeroU32::new(4).unwrap()).unwrap();
    let enrollment = pending(&mut registry, 1);
    let before = registry.step(ReadDevice {
        device: enrollment.device(),
    });
    let error = installation
        .devices
        .persist_enrollment_with::<4>(
            &mut registry,
            completion(enrollment, 1),
            publish_without_confirmation,
        )
        .unwrap_err();
    let PersistEnrollmentError::Store {
        completion: rejected,
        source: DeviceStoreError::PublishedDurabilityUnconfirmed(error),
    } = error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(error.to_string(), "directory sync failed");
    assert_eq!(*rejected, completion(enrollment, 1));
    assert_eq!(
        registry.step(ReadDevice {
            device: enrollment.device()
        }),
        before
    );
    let LoadDevicesOutcome::Loaded {
        registry: mut restored,
    } = installation
        .devices
        .load(NonZeroU32::new(4).unwrap())
        .unwrap()
    else {
        panic!("published file missing")
    };
    let ReadRememberedDevicesOutcome::Read { devices } = restored.step(ReadRememberedDevices::<4>)
    else {
        panic!("read refused")
    };
    assert_eq!(
        devices.first().unwrap().pairing,
        RememberedPairing::Paired {
            target: *target(1).public_keys()
        }
    );
    assert_eq!(
        installation
            .devices
            .persist_enrollment::<4>(&mut registry, *rejected)
            .unwrap(),
        PersistEnrollmentOutcome::Recorded { enrollment }
    );
}

#[test]
fn impossible_post_write_refusal_is_an_invariant_error() {
    let mut registry = DeviceRegistry::try_new(NonZeroU32::MIN).unwrap();
    let enrollment = pending(&mut registry, 1);
    let outcome = CompleteEnrollmentOutcome::StaleEnrollment {
        rejected: completion(enrollment, 1),
    };
    let Err(PersistEnrollmentError::CompletionInvariant { outcome: actual }) =
        settle_completion(outcome)
    else {
        panic!("refusal lost")
    };
    assert_eq!(
        *actual,
        CompleteEnrollmentOutcome::StaleEnrollment {
            rejected: completion(enrollment, 1)
        }
    );
}
