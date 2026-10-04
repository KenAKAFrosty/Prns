#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::tests::{paired, target};
use crate::{ControllerInstallation, ControllerInstallationError};
use personal_rns::crypto::sha256;
use proptest::prelude::*;

fn registry() -> DeviceRegistry {
    DeviceRegistry::try_new(NonZeroU32::new(4).unwrap()).unwrap()
}

fn saved(name: &str, seed: u8) -> RememberedDevice {
    RememberedDevice {
        label: DeviceLabel::new(name).unwrap(),
        pairing: RememberedPairing::Paired {
            target: *target(seed).public_keys(),
        },
    }
}

fn sealed(payload: &[u8]) -> alloc::vec::Vec<u8> {
    let mut bytes = payload.to_vec();
    bytes.extend_from_slice(&sha256(payload));
    bytes
}

fn payload(count: u32, records: &[u8]) -> alloc::vec::Vec<u8> {
    let mut bytes = b"HOPDEV01".to_vec();
    bytes.extend_from_slice(&count.to_le_bytes());
    bytes.extend_from_slice(records);
    bytes
}

#[test]
fn archive_roundtrip_preserves_unicode_pairing_and_the_versioned_wire_format() {
    let records = [
        RememberedDevice {
            label: DeviceLabel::new(" A ").unwrap(),
            pairing: RememberedPairing::Unpaired,
        },
        saved("🛰", 7),
        saved(&"Z".repeat(128), 8),
    ];
    let encoded = codec::encode(&records);
    assert_eq!(encoded.get(..17).unwrap(), b"HOPDEV01\x03\0\0\0\x03 A \0");
    assert_eq!(codec::decode(&encoded, 3).unwrap(), records);
    assert_eq!(codec::decode(&codec::encode(&[]), 0).unwrap(), []);
    assert_eq!(
        codec::decode(&encoded, 2),
        Err(DeviceArchiveError::TooManyDevices {
            count: 3,
            maximum: 2
        })
    );
    let mut corrupt = encoded;
    *corrupt.last_mut().unwrap() ^= 1;
    assert_eq!(
        codec::decode(&corrupt, 3),
        Err(DeviceArchiveError::ChecksumMismatch)
    );
}

#[test]
fn malformed_archives_are_rejected_without_partial_decoding() {
    assert_eq!(codec::decode(&[], 1), Err(DeviceArchiveError::Truncated));
    for bytes in [
        b"".as_slice(),
        b"HOPDEV01",
        b"HOPDEV01\x01",
        b"HOPDEV01\x01\0\0\0",
        b"HOPDEV01\x01\0\0\0\x05a",
        b"HOPDEV01\x01\0\0\0\x01a",
        b"HOPDEV01\x01\0\0\0\x01a\x01",
    ] {
        assert_eq!(
            codec::decode(&sealed(bytes), 1),
            Err(DeviceArchiveError::Truncated)
        );
    }
    assert_eq!(
        codec::decode(&sealed(b"HOPDEV02\0\0\0\0"), 1),
        Err(DeviceArchiveError::UnsupportedFormat)
    );
    assert_eq!(
        codec::decode(&sealed(&payload(1, &[1, 0xff, 0])), 1),
        Err(DeviceArchiveError::InvalidLabelUtf8)
    );
    assert_eq!(
        codec::decode(&sealed(&payload(1, &[0, 0])), 1),
        Err(DeviceArchiveError::Label(DeviceLabelError::Blank))
    );
    let mut long = alloc::vec![129];
    long.extend_from_slice(&[b'a'; 129]);
    long.push(0);
    assert_eq!(
        codec::decode(&sealed(&payload(1, &long)), 1),
        Err(DeviceArchiveError::Label(DeviceLabelError::TooLong {
            bytes: 129,
            maximum: 128
        }))
    );
    assert_eq!(
        codec::decode(&sealed(&payload(1, &[1, b'a', 2])), 1),
        Err(DeviceArchiveError::InvalidPairingTag { tag: 2 })
    );
    assert_eq!(
        codec::decode(&sealed(&payload(0, &[9])), 1),
        Err(DeviceArchiveError::TrailingBytes)
    );
    let mut half_key = alloc::vec![1, b'a', 1];
    half_key.extend_from_slice(&[1; 32]);
    assert_eq!(
        codec::decode(&sealed(&payload(1, &half_key)), 1),
        Err(DeviceArchiveError::Truncated)
    );
}

#[test]
fn save_and_restart_restore_records_and_replace_renames_and_forgetting() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    assert!(matches!(
        installation.devices.load(NonZeroU32::MIN),
        Ok(LoadDevicesOutcome::Missing)
    ));
    assert_eq!(
        installation.devices.save::<0>(&mut registry()).unwrap(),
        SaveDevicesOutcome::Saved
    );
    std::fs::remove_file(directory.path().join(DEVICES_FILE)).unwrap();
    let mut original = registry();
    let id = paired(&mut original, target(7));
    assert_eq!(
        installation.devices.save::<0>(&mut original).unwrap(),
        SaveDevicesOutcome::InsufficientCapacity { required: 1 }
    );
    assert!(matches!(
        installation.devices.load(NonZeroU32::MIN),
        Ok(LoadDevicesOutcome::Missing)
    ));
    assert_eq!(
        installation.devices.save::<4>(&mut original).unwrap(),
        SaveDevicesOutcome::Saved
    );
    let expected = original.step(ReadRememberedDevices::<4>);
    drop(installation);
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    let LoadDevicesOutcome::Loaded { mut registry } = installation
        .devices
        .load(NonZeroU32::new(4).unwrap())
        .unwrap()
    else {
        panic!("saved file missing")
    };
    assert_eq!(registry.step(ReadRememberedDevices::<4>), expected);
    let ListDevicesOutcome::Listed { devices } = registry.step(ListDevices::<4>) else {
        panic!("list refused")
    };
    let restored = *devices.first().unwrap();
    let ReadDeviceOutcome::Found { device } = registry.step(ReadDevice { device: restored }) else {
        panic!("restored record missing")
    };
    assert_eq!(device.connection, ConnectionState::NotConnected);
    let _renamed = original.step(RenameDevice {
        device: id,
        label: DeviceLabel::new("New label").unwrap(),
    });
    assert_eq!(
        installation.devices.save::<4>(&mut original).unwrap(),
        SaveDevicesOutcome::Saved
    );
    let LoadDevicesOutcome::Loaded { mut registry } = installation
        .devices
        .load(NonZeroU32::new(4).unwrap())
        .unwrap()
    else {
        panic!("replacement missing")
    };
    assert_eq!(
        registry.step(ReadRememberedDevices::<4>),
        original.step(ReadRememberedDevices::<4>)
    );
    let _forgotten = original.step(ForgetDevice { device: id });
    assert_eq!(
        installation.devices.save::<4>(&mut original).unwrap(),
        SaveDevicesOutcome::Saved
    );
    let LoadDevicesOutcome::Loaded { mut registry } = installation
        .devices
        .load(NonZeroU32::new(4).unwrap())
        .unwrap()
    else {
        panic!("empty archive missing")
    };
    let ReadRememberedDevicesOutcome::Read { devices } = registry.step(ReadRememberedDevices::<0>)
    else {
        panic!("empty registry refused")
    };
    assert!(devices.is_empty());
}

#[test]
fn archive_limits_duplicates_and_io_failures_do_not_silently_reset_storage() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    let path = directory.path().join(DEVICES_FILE);
    std::fs::write(&path, codec::encode(&[saved(&"x".repeat(128), 9)])).unwrap();
    assert!(matches!(
        installation.devices.load(NonZeroU32::MIN),
        Ok(LoadDevicesOutcome::Loaded { .. })
    ));
    std::fs::write(&path, [0; 239]).unwrap();
    assert!(matches!(
        installation.devices.load(NonZeroU32::MIN),
        Err(DeviceStoreError::ArchiveTooLarge { maximum_bytes: 238 })
    ));
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 239);
    let bad = codec::encode(&[saved("Duplicate A", 1), saved("Duplicate B", 1)]);
    std::fs::write(&path, &bad).unwrap();
    assert!(
        matches!(installation.devices.load(NonZeroU32::new(2).unwrap()), Err(DeviceStoreError::RestoreRefused(ref outcome)) if matches!(outcome.as_ref(), RestoreRememberedDeviceOutcome::TargetAlreadyPaired { .. }))
    );
    assert_eq!(std::fs::read(&path).unwrap(), bad);
    std::fs::write(&path, b"broken").unwrap();
    assert!(matches!(
        installation.devices.load(NonZeroU32::MIN),
        Err(DeviceStoreError::Archive(DeviceArchiveError::Truncated))
    ));
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(matches!(
        installation.devices.load(NonZeroU32::MIN),
        Err(DeviceStoreError::Io(_))
    ));
    assert!(matches!(
        installation.devices.save::<4>(&mut registry()),
        Err(DeviceStoreError::Io(_))
    ));
    std::fs::remove_dir(&path).unwrap();
    assert!(!directory.path().read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp")
    }));
    std::fs::remove_dir_all(directory.path()).unwrap();
    assert!(matches!(
        installation.devices.save::<4>(&mut registry()),
        Err(DeviceStoreError::Io(_))
    ));
    std::fs::write(directory.path(), b"not a directory").unwrap();
    assert!(matches!(
        installation.devices.load(NonZeroU32::MIN),
        Err(DeviceStoreError::Io(_))
    ));
    std::fs::remove_file(directory.path()).unwrap();
    #[cfg(unix)]
    assert!(confirm_directory(directory.path()).is_err());
}

#[test]
fn published_durability_failure_preserves_the_new_file_and_retains_the_lock() {
    let directory = tempfile::tempdir().unwrap();
    let installation = ControllerInstallation::open(directory.path()).unwrap();
    let mut staged = NamedTempFile::new_in(directory.path()).unwrap();
    staged.write_all(b"replacement").unwrap();
    let failed = installation.devices.publish(staged, |_| {
        Err(std::io::Error::other("directory sync refused"))
    });
    assert!(matches!(
        failed,
        Err(DeviceStoreError::PublishedDurabilityUnconfirmed(_))
    ));
    assert_eq!(
        std::fs::read(directory.path().join(DEVICES_FILE)).unwrap(),
        b"replacement"
    );
    let ControllerInstallation {
        devices,
        state_lock,
        identity,
        persistence,
    } = installation;
    drop((state_lock, identity, persistence));
    assert!(matches!(
        ControllerInstallation::open(directory.path()),
        Err(ControllerInstallationError::Lock(
            std::fs::TryLockError::WouldBlock
        ))
    ));
    drop(devices);
    assert!(ControllerInstallation::open(directory.path()).is_ok());
}

#[cfg(unix)]
#[test]
fn stored_records_and_replacements_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    for _ in 0..2 {
        assert_eq!(
            installation.devices.save::<4>(&mut registry()).unwrap(),
            SaveDevicesOutcome::Saved
        );
        assert_eq!(
            std::fs::metadata(directory.path().join(DEVICES_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn arbitrary_valid_records_roundtrip_and_single_bit_corruption_is_detected(
        labels in prop::collection::vec("[^\\p{C}]{1,31}", 0..8),
        offset in any::<usize>(),
    ) {
        let records: alloc::vec::Vec<_> = labels.iter().enumerate().map(|(i, label)| saved(&alloc::format!("{label}!"), i as u8)).collect();
        let mut bytes = codec::encode(&records);
        prop_assert_eq!(codec::decode(&bytes, 8).unwrap(), records);
        let position = offset.checked_rem(bytes.len()).unwrap();
        *bytes.get_mut(position).unwrap() ^= 1;
        prop_assert_eq!(codec::decode(&bytes, 8), Err(DeviceArchiveError::ChecksumMismatch));
    }
}

#[test]
fn staged_write_and_sync_failures_preserve_the_published_archive() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    assert_eq!(
        installation.devices.save::<4>(&mut registry()).unwrap(),
        SaveDevicesOutcome::Saved
    );
    let path = directory.path().join(DEVICES_FILE);
    let published = std::fs::read(&path).unwrap();
    let mut read_only = File::open(&path).unwrap();
    assert!(
        stage_snapshot(&mut read_only, b"replacement", |_| panic!(
            "cannot sync a failed write"
        ))
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), published);
    let mut staged = NamedTempFile::new_in(directory.path()).unwrap();
    let staged_path = staged.path().to_path_buf();
    let failure = stage_snapshot(staged.as_file_mut(), b"replacement", |file| {
        assert_eq!(file.metadata().unwrap().len(), 11);
        Err(std::io::Error::other("file sync refused"))
    });
    assert!(failure.is_err());
    drop(staged);
    assert_eq!(std::fs::read(&path).unwrap(), published);
    assert!(!staged_path.exists());
}

#[test]
fn restoration_failure_translation_retains_invariants_and_refused_records() {
    let rejected = RestoreRememberedDevice {
        remembered: saved("Exhausted", 3),
    };
    let result = settle_restoration(Err(RestoreRememberedDeviceError::IdentifiersExhausted {
        rejected,
    }));
    let Err(DeviceStoreError::Restore(error)) = result else {
        panic!("invariant hidden")
    };
    assert_eq!(
        *error,
        RestoreRememberedDeviceError::IdentifiersExhausted {
            rejected: RestoreRememberedDevice {
                remembered: saved("Exhausted", 3)
            }
        }
    );
    let result = settle_restoration(Ok(RestoreRememberedDeviceOutcome::AtCapacity {
        rejected: RestoreRememberedDevice {
            remembered: saved("Full", 4),
        },
        maximum_devices: NonZeroU32::MIN,
    }));
    let Err(DeviceStoreError::RestoreRefused(outcome)) = result else {
        panic!("refusal hidden")
    };
    assert_eq!(
        *outcome,
        RestoreRememberedDeviceOutcome::AtCapacity {
            rejected: RestoreRememberedDevice {
                remembered: saved("Full", 4)
            },
            maximum_devices: NonZeroU32::MIN
        }
    );
}

#[test]
fn insufficient_snapshot_capacity_never_replaces_an_existing_archive() {
    let directory = tempfile::tempdir().unwrap();
    let mut installation = ControllerInstallation::open(directory.path()).unwrap();
    assert_eq!(
        installation.devices.save::<0>(&mut registry()).unwrap(),
        SaveDevicesOutcome::Saved
    );
    let path = directory.path().join(DEVICES_FILE);
    let before = std::fs::read(&path).unwrap();
    let mut records = DeviceRegistry::try_new(NonZeroU32::new(5).unwrap()).unwrap();
    for _ in 0..5 {
        assert!(matches!(
            records
                .step(CreateDevice {
                    label: DeviceLabel::new("Planned").unwrap()
                })
                .unwrap(),
            CreateDeviceOutcome::Created { .. }
        ));
    }
    assert_eq!(
        installation.devices.save::<1>(&mut records).unwrap(),
        SaveDevicesOutcome::InsufficientCapacity { required: 5 }
    );
    assert_eq!(
        installation.devices.save::<4>(&mut records).unwrap(),
        SaveDevicesOutcome::InsufficientCapacity { required: 5 }
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
