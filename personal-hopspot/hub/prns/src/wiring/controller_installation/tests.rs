#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use personal_rns::identity::vault::IdentityOrigin;
use proptest::prelude::*;

#[test]
fn reopening_preserves_distinct_identities_and_lock_file_contents() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("installation");
    let first = ControllerInstallation::open(&directory).unwrap();
    let identities = first.identity.secrets().identities();
    assert_ne!(
        identities.controller().identity_hash(),
        identities.target().identity_hash()
    );
    assert_eq!(
        first.identity.origins().controller(),
        IdentityOrigin::Generated
    );
    assert_eq!(first.identity.origins().target(), IdentityOrigin::Generated);
    assert!(directory.join("retained").is_dir());
    assert!(directory.join("remote_control/controller").is_file());
    assert!(directory.join("remote_control/target").is_file());
    std::fs::write(directory.join("hub.lock"), b"preserve").unwrap();
    assert!(matches!(
        ControllerInstallation::open(&directory),
        Err(ControllerInstallationError::Lock(TryLockError::WouldBlock))
    ));
    assert_eq!(
        std::fs::read(directory.join("hub.lock")).unwrap(),
        b"preserve"
    );
    drop(first);
    let second = ControllerInstallation::open(&directory).unwrap();
    assert_eq!(second.identity.secrets().identities(), identities);
    assert_eq!(
        second.identity.origins().controller(),
        IdentityOrigin::Loaded
    );
    assert_eq!(second.identity.origins().target(), IdentityOrigin::Loaded);
    assert_eq!(
        std::fs::read(directory.join("hub.lock")).unwrap(),
        b"preserve"
    );
}

#[test]
fn startup_errors_preserve_their_stage_and_release_the_installation_lock() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("installation");
    std::fs::write(&directory, b"file").unwrap();
    assert!(matches!(
        ControllerInstallation::open(&directory),
        Err(ControllerInstallationError::StateDirectory(_))
    ));
    std::fs::remove_file(&directory).unwrap();
    std::fs::create_dir_all(directory.join("hub.lock")).unwrap();
    assert!(matches!(
        ControllerInstallation::open(&directory),
        Err(ControllerInstallationError::LockFile(_))
    ));
    std::fs::remove_dir(directory.join("hub.lock")).unwrap();
    std::fs::write(directory.join("retained"), b"file").unwrap();
    assert!(matches!(
        ControllerInstallation::open(&directory),
        Err(ControllerInstallationError::Persistence(_))
    ));
    std::fs::remove_file(directory.join("retained")).unwrap();
    let recovered = ControllerInstallation::open(&directory).unwrap();
    assert_eq!(
        recovered.identity.origins().controller(),
        IdentityOrigin::Loaded
    );
}

#[test]
fn corrupt_identity_material_is_refused_without_replacement() {
    for name in ["controller", "target"] {
        let root = tempfile::tempdir().unwrap();
        let initial = ControllerInstallation::open(root.path()).unwrap();
        drop(initial);
        let path = root.path().join("remote_control").join(name);
        std::fs::write(&path, b"short").unwrap();
        for _ in 0..2 {
            assert!(matches!(
                ControllerInstallation::open(root.path()),
                Err(ControllerInstallationError::Identity(
                    RemoteControlFileIdentityBootstrapError::Bootstrap(_)
                ))
            ));
            assert_eq!(std::fs::read(&path).unwrap(), b"short");
        }
    }
}

#[cfg(unix)]
#[test]
fn installation_directory_and_new_lock_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    let _installation = ControllerInstallation::open(root.path()).unwrap();
    assert_eq!(
        std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(root.path().join("hub.lock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn arbitrary_reopens_and_competing_owners_preserve_one_identity(actions in prop::collection::vec(any::<bool>(), 0..40)) {
        let root = tempfile::tempdir().unwrap();
        let initial = ControllerInstallation::open(root.path()).unwrap();
        let expected = initial.identity.secrets().identities();
        let mut owner = Some(initial);
        for release in actions {
            if release {
                drop(owner.take());
            } else {
                let opened = ControllerInstallation::open(root.path());
                if owner.is_some() {
                    prop_assert!(matches!(opened, Err(ControllerInstallationError::Lock(TryLockError::WouldBlock))));
                } else {
                    let acquired = opened.unwrap();
                    prop_assert_eq!(&acquired.identity.secrets().identities(), &expected);
                    owner = Some(acquired);
                }
            }
        }
        drop(owner);
        prop_assert_eq!(ControllerInstallation::open(root.path()).unwrap().identity.secrets().identities(), expected);
    }
}
