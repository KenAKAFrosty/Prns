use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use super::fake_sam::{oracle_private_destination, private_destination};
use crate::i2p::destination::{
    load_destination, persist_destination, I2pDestinationKeyPath, I2pDestinationStorageError,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("prns-i2p-{label}-{}-{serial}", std::process::id()));
        fs::create_dir(&path).expect("the unique test directory is created");
        Self(path)
    }

    fn key_path(&self) -> I2pDestinationKeyPath {
        I2pDestinationKeyPath::new(self.0.join("i2p.key"))
            .expect("the test key path has a file name")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn private_destinations_round_trip_without_format_drift() {
    let directory = TestDirectory::new("round-trip");
    let path = directory.key_path();
    let generated = private_destination(0x18);

    let persisted = persist_destination(&path, generated.clone()).expect("persistence succeeds");
    let loaded = load_destination(&path)
        .expect("loading succeeds")
        .expect("the destination exists");
    let raw = fs::read_to_string(path.as_path()).expect("the destination file is readable");

    assert_eq!(persisted, generated);
    assert_eq!(loaded, generated);
    assert_eq!(raw, generated.as_str());
    assert!(!raw.ends_with('\n'));
}

#[test]
fn an_existing_destination_wins_without_overwrite() {
    let directory = TestDirectory::new("no-clobber");
    let path = directory.key_path();
    let winner = private_destination(0x21);
    let contender = private_destination(0x22);

    persist_destination(&path, winner.clone()).expect("the first destination is persisted");
    let selected =
        persist_destination(&path, contender).expect("the existing destination is loaded");

    assert_eq!(selected, winner);
    assert_eq!(
        fs::read_to_string(path.as_path()).expect("the destination file is readable"),
        winner.as_str()
    );
}

#[test]
fn concurrent_creators_converge_on_one_persistent_identity() {
    let directory = TestDirectory::new("race");
    let path = directory.key_path();
    let barrier = Arc::new(Barrier::new(3));
    let contenders = [private_destination(0x29), private_destination(0x2a)];
    let handles = contenders.map(|contender| {
        let path = path.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            persist_destination(&path, contender)
        })
    });
    barrier.wait();
    let selected = handles.map(|handle| {
        handle
            .join()
            .expect("the persistence thread finishes")
            .expect("the contender converges on an identity")
    });

    assert_eq!(selected[0], selected[1]);
    assert_eq!(
        load_destination(&path).expect("loading succeeds"),
        Some(selected[0].clone())
    );
}

#[test]
fn invalid_existing_material_is_reported_and_preserved() {
    let directory = TestDirectory::new("invalid-existing");
    let path = directory.key_path();
    fs::write(path.as_path(), "not-a-private-destination").expect("the invalid fixture is written");

    let result = persist_destination(&path, private_destination(0x33));

    assert!(matches!(
        result,
        Err(I2pDestinationStorageError::Invalid { .. })
    ));
    assert_eq!(
        fs::read_to_string(path.as_path()).expect("the invalid fixture remains readable"),
        "not-a-private-destination"
    );
}

#[cfg(unix)]
#[test]
fn new_destination_material_is_owner_only() {
    let directory = TestDirectory::new("permissions");
    let path = directory.key_path();

    persist_destination(&path, private_destination(0x39)).expect("persistence succeeds");
    let mode = fs::metadata(path.as_path())
        .expect("destination metadata is readable")
        .permissions()
        .mode();

    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn reference_destination_derivation_matches_python_i2plib() {
    let private = oracle_private_destination();
    let public = private
        .public_destination()
        .expect("the reference public destination is derivable");
    let base32 = public
        .base32_address()
        .expect("the reference base32 address is derivable");

    assert_eq!(public.as_str().len(), 516);
    assert_eq!(
        base32.as_str(),
        "jzxapbvh6g2paauj637fnsblh3imrzsl5wgp73nqvnppl7hbuhcq.b32.i2p"
    );
}

#[test]
fn private_destination_debug_output_is_redacted() {
    let private = private_destination(0x41);
    let rendered = format!("{private:?}");

    assert_eq!(rendered, "I2pPrivateDestination(\"[REDACTED]\")");
    assert!(!rendered.contains(private.as_str()));
}

#[test]
fn destination_paths_require_a_file_name() {
    assert!(I2pDestinationKeyPath::new(Path::new("")).is_err());
    assert!(I2pDestinationKeyPath::new(Path::new("/")).is_err());
}
