use super::*;
use crate::domain_primitives::RememberedPairing;
use proptest::prelude::*;

#[test]
fn preparation_preserves_exact_refusals_and_does_not_change_pending_records() {
    let mut registry = registry(8);
    let device = create(&mut registry, "First");
    let enrollment = begin(&mut registry, device, 1);
    let before = read(&mut registry, device);
    assert_eq!(
        registry.step(PrepareEnrollmentCompletion::<4> {
            completion: complete(enrollment, 2)
        }),
        PrepareEnrollmentCompletionOutcome::TargetMismatch {
            rejected: complete(enrollment, 2)
        }
    );
    assert_eq!(read(&mut registry, device), before);
    assert_eq!(
        registry.step(CancelEnrollment { enrollment }),
        CancelEnrollmentOutcome::Cancelled { enrollment }
    );
    assert_eq!(
        registry.step(PrepareEnrollmentCompletion::<4> {
            completion: complete(enrollment, 1)
        }),
        PrepareEnrollmentCompletionOutcome::StaleEnrollment {
            rejected: complete(enrollment, 1)
        }
    );
    let active = begin(&mut registry, device, 1);
    assert_eq!(
        registry.step(PrepareEnrollmentCompletion::<4> {
            completion: complete(enrollment, 1)
        }),
        PrepareEnrollmentCompletionOutcome::StaleEnrollment {
            rejected: complete(enrollment, 1)
        }
    );
    let other = create(&mut registry, "Other");
    let paired = begin(&mut registry, other, 1);
    assert_eq!(
        registry.step(complete(paired, 1)),
        CompleteEnrollmentOutcome::Recorded { enrollment: paired }
    );
    assert_eq!(
        registry.step(PrepareEnrollmentCompletion::<4> {
            completion: complete(active, 1)
        }),
        PrepareEnrollmentCompletionOutcome::TargetAlreadyPaired {
            rejected: complete(active, 1),
            device: other
        }
    );
    let _forgotten = registry.step(ForgetDevice { device });
    assert_eq!(
        registry.step(PrepareEnrollmentCompletion::<4> {
            completion: complete(active, 1)
        }),
        PrepareEnrollmentCompletionOutcome::MissingDevice {
            rejected: complete(active, 1)
        }
    );
    let device = create(&mut registry, "New");
    let enrollment = begin(&mut registry, device, 2);
    for _ in 0..3 {
        let _device = create(&mut registry, "Overflow");
    }
    let before = read(&mut registry, device);
    assert_eq!(
        registry.step(PrepareEnrollmentCompletion::<4> {
            completion: complete(enrollment, 2)
        }),
        PrepareEnrollmentCompletionOutcome::InsufficientCapacity {
            rejected: complete(enrollment, 2),
            required: 5
        }
    );
    assert_eq!(read(&mut registry, device), before);
}

proptest! {
    #[test]
    fn prospective_records_equal_completed_export_without_activating_enrollment(seed in any::<u8>(), name in "[a-zA-Z🛰]{1,20}", pending in any::<bool>()) {
        let mut registry = registry(4);
        let first = create(&mut registry, "Already paired");
        let prior = begin(&mut registry, first, seed.wrapping_add(1));
        prop_assert_eq!(registry.step(complete(prior, seed.wrapping_add(1))), CompleteEnrollmentOutcome::Recorded { enrollment: prior });
        let device = create(&mut registry, &name);
        let enrollment = begin(&mut registry, device, seed);
        let other = create(&mut registry, "Other");
        if pending { let _enrollment = begin(&mut registry, other, seed.wrapping_add(2)); }
        let before = read(&mut registry, device);
        let other_before = read(&mut registry, other);
        let PrepareEnrollmentCompletionOutcome::Prepared { completion, devices } = registry.step(PrepareEnrollmentCompletion::<4> { completion: complete(enrollment, seed) }) else { prop_assert!(false, "preparation refused"); return Ok(()); };
        prop_assert_eq!(&completion, &complete(enrollment, seed));
        prop_assert_eq!(read(&mut registry, device), before);
        prop_assert_eq!(read(&mut registry, other), other_before);
        prop_assert_eq!(&devices.first().unwrap().pairing, &RememberedPairing::Paired { target: *target(seed.wrapping_add(1)).public_keys() });
        prop_assert_eq!(&devices.get(1).unwrap().pairing, &RememberedPairing::Paired { target: *target(seed).public_keys() });
        prop_assert_eq!(&devices.get(1).unwrap().label, &DeviceLabel::new(&name).unwrap());
        prop_assert_eq!(&devices.get(2).unwrap().pairing, &RememberedPairing::Unpaired);
        prop_assert_eq!(registry.step(completion), CompleteEnrollmentOutcome::Recorded { enrollment });
        prop_assert_eq!(registry.step(ReadRememberedDevices::<4>), ReadRememberedDevicesOutcome::Read { devices });
    }
}
