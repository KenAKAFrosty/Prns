use super::*;
use pipecircuit::storage::warp_table::WarpTableInsertOutcome;

#[test]
fn bounded_records_support_duplicate_labels_and_stable_ids_after_removal() {
    let mut registry = registry(2);
    let first = create(&mut registry, "Roof");
    let second = create(&mut registry, "Roof");
    let label = DeviceLabel::new("Spare").unwrap();
    assert_eq!(
        registry.step(CreateDevice {
            label: label.clone()
        }),
        CreateDeviceOutcome::AtCapacity {
            rejected: CreateDevice { label },
            maximum_devices: NonZeroU32::new(2).unwrap(),
        }
    );
    assert_eq!(
        registry.step(ForgetDevice { device: first }),
        ForgetDeviceOutcome::Forgotten {
            device: first,
            enrollment: EnrollmentState::Planned,
        }
    );
    let replacement = create(&mut registry, "Spare");
    assert_ne!(replacement, first);
    assert_eq!(
        registry.step(ReadDevice { device: first }),
        ReadDeviceOutcome::MissingDevice { device: first }
    );
    assert_eq!(
        registry.step(RenameDevice {
            device: second,
            label: DeviceLabel::new("Hill").unwrap()
        }),
        RenameDeviceOutcome::Renamed { device: second }
    );
    assert_eq!(list::<2>(&mut registry).as_slice(), &[second, replacement]);
    assert_eq!(
        read(&mut registry, second),
        DeviceSnapshot {
            id: second,
            label: DeviceLabel::new("Hill").unwrap(),
            enrollment: EnrollmentState::Planned,
        }
    );
    assert_eq!(read(&mut registry, replacement).label.as_str(), "Spare");
    assert_eq!(
        registry.step(ForgetDevice { device: first }),
        ForgetDeviceOutcome::MissingDevice { device: first }
    );
    let label = DeviceLabel::new("Lost").unwrap();
    assert_eq!(
        registry.step(RenameDevice {
            device: first,
            label: label.clone()
        }),
        RenameDeviceOutcome::MissingDevice {
            rejected: RenameDevice {
                device: first,
                label
            },
        }
    );
}

#[test]
fn query_steps_own_snapshots_and_refuse_incomplete_lists() {
    let mut registry = registry(3);
    assert!(list::<0>(&mut registry).is_empty());
    let first = create(&mut registry, "First");
    let snapshot = read(&mut registry, first);
    let second = create(&mut registry, "Second");
    let third = create(&mut registry, "Third");
    assert_eq!(
        registry.step(ListDevices::<0>),
        ListDevicesOutcome::InsufficientCapacity { required: 3 }
    );
    assert_eq!(
        registry.step(ListDevices::<1>),
        ListDevicesOutcome::InsufficientCapacity { required: 3 }
    );
    assert_eq!(
        registry.step(ListDevices::<2>),
        ListDevicesOutcome::InsufficientCapacity { required: 3 }
    );
    assert_eq!(list::<3>(&mut registry).as_slice(), &[first, second, third]);
    assert_eq!(list::<4>(&mut registry).as_slice(), &[first, second, third]);
    assert_eq!(
        registry.step(RenameDevice {
            device: first,
            label: DeviceLabel::new("Renamed").unwrap()
        }),
        RenameDeviceOutcome::Renamed { device: first }
    );
    assert_eq!(
        snapshot,
        DeviceSnapshot {
            id: first,
            label: DeviceLabel::new("First").unwrap(),
            enrollment: EnrollmentState::Planned
        }
    );
    assert_eq!(read(&mut registry, first).label.as_str(), "Renamed");
}

#[test]
fn exhausted_storage_returns_the_unmodified_rejected_label() {
    let mut registry = registry(1);
    let label = DeviceLabel::new(" Unstarted 🛰 ").unwrap();
    let outcome = registry.created(WarpTableInsertOutcome::IdentifiersExhausted {
        rejected: DeviceRecord {
            label: label.clone(),
            enrollment: EnrollmentState::Planned,
        },
    });
    assert_eq!(
        outcome,
        CreateDeviceOutcome::IdentifiersExhausted {
            rejected: CreateDevice { label }
        }
    );
    assert!(list::<0>(&mut registry).is_empty());
}

#[test]
fn construction_failure_retains_its_original_storage_diagnostic() {
    let source: StorageCreationError =
        WarpTableCreationError::PrimaryIndexCapacityOverflow { maximum_rows: 37 };
    let error = DeviceRegistryCreationError::from_storage(source);
    assert_eq!(
        alloc::format!("{:?}", error.source()),
        "PrimaryIndexCapacityOverflow { maximum_rows: 37 }"
    );
}
