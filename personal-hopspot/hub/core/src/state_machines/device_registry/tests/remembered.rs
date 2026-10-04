use super::*;
use crate::{
    ReadRememberedDevices, ReadRememberedDevicesOutcome, RememberedDevice, RememberedPairing,
    RestoreRememberedDevice, RestoreRememberedDeviceError, RestoreRememberedDeviceOutcome,
};
use pipecircuit::storage::warp_table::WarpTableInsertOutcome;
use proptest::prelude::*;

fn remembered(name: &str, seed: u8) -> RememberedDevice {
    RememberedDevice {
        label: DeviceLabel::new(name).unwrap(),
        pairing: RememberedPairing::Paired {
            target: *target(seed).public_keys(),
        },
    }
}

fn restore(registry: &mut DeviceRegistry, remembered: RememberedDevice) -> DeviceId {
    let Ok(RestoreRememberedDeviceOutcome::Restored { device }) =
        registry.step(RestoreRememberedDevice { remembered })
    else {
        panic!("restore refused")
    };
    device
}

#[test]
fn remembered_records_preserve_labels_and_pairings_without_live_state() {
    let mut original = registry(4);
    let planned = create(&mut original, "Planned");
    let pending = create(&mut original, "Pending");
    let _enrollment = begin(&mut original, pending, 2);
    let connected = restore(&mut original, remembered("Paired", 3));
    let Ok(BeginConnectionOutcome::Connect { connection }) =
        original.step(BeginConnection { device: connected })
    else {
        panic!("connect refused")
    };
    let _connected = original.step(ConfirmConnection {
        connection,
        target: target(3),
        link: prns_core::routing::links::LinkId::new([4; 16]),
    });
    let original_snapshot = read(&mut original, connected);
    assert_eq!(
        original.step(ReadRememberedDevices::<1>),
        ReadRememberedDevicesOutcome::InsufficientCapacity { required: 3 }
    );
    let ReadRememberedDevicesOutcome::Read { devices } = original.step(ReadRememberedDevices::<4>)
    else {
        panic!("read refused")
    };
    assert_eq!(
        devices.as_slice(),
        &[
            RememberedDevice {
                label: DeviceLabel::new("Planned").unwrap(),
                pairing: RememberedPairing::Unpaired
            },
            RememberedDevice {
                label: DeviceLabel::new("Pending").unwrap(),
                pairing: RememberedPairing::Unpaired
            },
            remembered("Paired", 3),
        ]
    );
    let mut restarted = registry(4);
    for saved in devices.clone() {
        let expected = match saved.pairing {
            RememberedPairing::Unpaired => EnrollmentState::Planned,
            RememberedPairing::Paired { target } => EnrollmentState::Paired { target },
        };
        let id = restore(&mut restarted, saved.clone());
        assert_eq!(
            read(&mut restarted, id),
            DeviceSnapshot {
                id,
                label: saved.label,
                enrollment: expected,
                connection: ConnectionState::NotConnected
            }
        );
    }
    assert_eq!(
        restarted.step(ReadRememberedDevices::<4>),
        ReadRememberedDevicesOutcome::Read {
            devices: devices.clone()
        }
    );
    let _renamed = original.step(RenameDevice {
        device: planned,
        label: DeviceLabel::new("Changed").unwrap(),
    });
    assert_eq!(devices.first().unwrap().label.as_str(), "Planned");
    assert_eq!(read(&mut original, connected), original_snapshot);
}

#[test]
fn restore_refuses_duplicates_and_capacity_without_changing_either_record() {
    let mut registry = registry(1);
    let id = restore(&mut registry, remembered("First", 1));
    let expected = read(&mut registry, id);
    assert!(matches!(
        registry.step(ReadRememberedDevices::<1>),
        ReadRememberedDevicesOutcome::Read { .. }
    ));
    let duplicate = RestoreRememberedDevice {
        remembered: remembered("Second", 1),
    };
    assert_eq!(
        registry.step(RestoreRememberedDevice {
            remembered: duplicate.remembered.clone()
        }),
        Ok(RestoreRememberedDeviceOutcome::TargetAlreadyPaired {
            rejected: duplicate,
            device: id
        })
    );
    let excess = RestoreRememberedDevice {
        remembered: remembered("Excess", 2),
    };
    assert_eq!(
        registry.step(RestoreRememberedDevice {
            remembered: excess.remembered.clone()
        }),
        Ok(RestoreRememberedDeviceOutcome::AtCapacity {
            rejected: excess,
            maximum_devices: NonZeroU32::MIN
        })
    );
    assert_eq!(read(&mut registry, id), expected);
    assert_eq!(
        registry.step(ReadRememberedDevices::<0>),
        ReadRememberedDevicesOutcome::InsufficientCapacity { required: 1 }
    );
    let _forgotten = registry.step(ForgetDevice { device: id });
    let restored = restore(&mut registry, remembered("Again", 1));
    assert_ne!(restored, id);
    let Ok(BeginConnectionOutcome::Connect { connection }) =
        registry.step(BeginConnection { device: restored })
    else {
        panic!("restore must allow connection")
    };
    assert_eq!(connection.target(), target(1));
}

#[test]
fn restore_exhaustion_retains_the_complete_rejected_record() {
    let registry = registry(1);
    let saved = remembered("Never inserted", 8);
    assert_eq!(
        registry.restored(WarpTableInsertOutcome::IdentifiersExhausted {
            rejected: DeviceRecord {
                label: saved.label.clone(),
                enrollment: EnrollmentState::Paired {
                    target: *target(8).public_keys()
                },
                connection: ConnectionState::NotConnected
            }
        }),
        Err(RestoreRememberedDeviceError::IdentifiersExhausted {
            rejected: RestoreRememberedDevice { remembered: saved }
        })
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn arbitrary_remembered_labels_and_targets_roundtrip_with_duplicate_refusal(
        records in prop::collection::vec(("[a-zA-Z ]{1,24}", 0u8..8), 0..24),
    ) {
        let mut registry = registry(24);
        let mut expected = alloc::vec::Vec::new();
        for (name, seed) in records {
            let saved = remembered(&alloc::format!("{name}!"), seed);
            let duplicate = expected.iter().any(|record: &RememberedDevice| record.pairing == saved.pairing);
            let outcome = registry.step(RestoreRememberedDevice { remembered: saved.clone() }).unwrap();
            if duplicate { prop_assert!(matches!(outcome, RestoreRememberedDeviceOutcome::TargetAlreadyPaired { .. }), "duplicate was accepted"); }
            else { prop_assert!(matches!(outcome, RestoreRememberedDeviceOutcome::Restored { .. }), "unique target refused"); expected.push(saved); }
        }
        let ReadRememberedDevicesOutcome::Read { devices } = registry.step(ReadRememberedDevices::<24>) else { panic!("bounded export refused") };
        prop_assert_eq!(devices.as_slice(), expected.as_slice());
    }
}
