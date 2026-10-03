use super::*;
use proptest::prelude::*;

proptest! {
    #[test]
    fn arbitrary_cancel_retry_sequences_never_accept_superseded_results(
        rounds in prop::collection::vec((any::<bool>(), 1u8..=255), 1..40),
    ) {
        let mut registry = registry(2);
        let first = create(&mut registry, "First");
        let other = create(&mut registry, "Other");
        let other_enrollment = begin(&mut registry, other, 0);
        let mut retired = alloc::vec::Vec::new();
        for (cancel, seed) in rounds {
            let active = begin(&mut registry, first, seed);
            for &old in &retired {
                prop_assert_eq!(registry.step(complete(old, seed)), CompleteEnrollmentOutcome::StaleEnrollment { rejected: complete(old, seed) });
            }
            prop_assert_eq!(read(&mut registry, first).enrollment, EnrollmentState::Pairing {
                enrollment: active, target: *target(seed).public_keys(),
            });
            if cancel {
                prop_assert_eq!(registry.step(CancelEnrollment { enrollment: active }), CancelEnrollmentOutcome::Cancelled { enrollment: active });
            } else {
                prop_assert_eq!(registry.step(FailEnrollment { enrollment: active, reason: EnrollmentFailure::ConnectionLost }), FailEnrollmentOutcome::Failed { enrollment: active, reason: EnrollmentFailure::ConnectionLost });
            }
            prop_assert_eq!(read(&mut registry, first).enrollment, EnrollmentState::Planned);
            prop_assert_eq!(read(&mut registry, other).enrollment, EnrollmentState::Pairing {
                enrollment: other_enrollment, target: *target(0).public_keys(),
            });
            retired.push(active);
        }
    }

    #[test]
    fn arbitrary_labels_and_removals_agree_with_an_independent_record_model(
        actions in prop::collection::vec((any::<bool>(), "[a-z]{1,16}"), 1..80),
    ) {
        let mut registry = registry(4);
        let mut model = alloc::collections::BTreeMap::new();
        let mut retired = alloc::vec::Vec::new();
        for (remove, name) in actions {
            if remove && !model.is_empty() {
                let (&key, &(id, _)) = model.first_key_value().unwrap();
                prop_assert_eq!(registry.step(ForgetDevice { device: id }), ForgetDeviceOutcome::Forgotten { device: id, enrollment: EnrollmentState::Planned });
                model.remove(&key);
                retired.push(id);
            } else if model.len() < 4 {
                let id = create(&mut registry, &name);
                prop_assert!(!retired.contains(&id));
                model.insert(id.0.get(), (id, name));
            } else {
                let label = DeviceLabel::new(&name).unwrap();
                prop_assert_eq!(registry.step(CreateDevice { label: label.clone() }), CreateDeviceOutcome::AtCapacity {
                    rejected: CreateDevice { label },
                    maximum_devices: NonZeroU32::new(4).unwrap(),
                });
            }
            let listed = list::<4>(&mut registry);
            prop_assert_eq!(listed.len(), model.len());
            for (id, name) in model.values() {
                prop_assert!(listed.contains(id));
                let snapshot = read(&mut registry, *id);
                prop_assert_eq!(snapshot.label.as_str(), name);
            }
            for &id in &retired { prop_assert_eq!(registry.step(ReadDevice { device: id }), ReadDeviceOutcome::MissingDevice { device: id }); }
        }
    }
}
