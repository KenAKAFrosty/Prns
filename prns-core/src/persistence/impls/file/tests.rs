use super::*;
use crate::persistence::{
    read_timebase_snapshot, write_timebase_snapshot, SnapshotOpenError, SnapshotReadError,
    TIMEBASE_SNAPSHOT_LEN,
};
use crate::units::InstantMillis;
use std::sync::atomic::{AtomicU32, Ordering};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("prns-store-{}-{}", std::process::id(), unique));
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
struct RestorePermissions {
    path: PathBuf,
    permissions: fs::Permissions,
}

#[cfg(unix)]
impl Drop for RestorePermissions {
    fn drop(&mut self) {
        // Restore traversal and removal permissions even if a regression panics.
        let _ = fs::set_permissions(&self.path, self.permissions.clone());
    }
}

const HIGH_WATER: InstantMillis = InstantMillis(1_770_000_000_000);

fn sealed_timebase() -> Vec<u8> {
    let mut out = [0u8; TIMEBASE_SNAPSHOT_LEN];
    let len = write_timebase_snapshot(HIGH_WATER, &mut out).unwrap();
    out[..len].to_vec()
}

#[test]
fn a_stored_snapshot_round_trips_through_the_trait() {
    let temp = TempDir::new();
    let mut store = FileStore::new(&temp.path);
    let sealed = sealed_timebase();
    store.store(SnapshotRegion::Timebase, &sealed).unwrap();

    assert_eq!(
        store.stored_len(SnapshotRegion::Timebase).unwrap(),
        Some(sealed.len()),
    );
    let mut buf = [0u8; TIMEBASE_SNAPSHOT_LEN];
    let loaded = store
        .load(SnapshotRegion::Timebase, &mut buf)
        .unwrap()
        .unwrap();
    assert_eq!(read_timebase_snapshot(loaded).unwrap(), HIGH_WATER);
}

#[cfg(unix)]
#[test]
fn an_existing_store_beneath_a_search_only_ancestor_can_save() {
    use std::os::unix::fs::MetadataExt as _;

    let temp = TempDir::new();
    let directory = temp.path.join("store");
    fs::create_dir_all(&directory).unwrap();
    let ancestor = fs::metadata(&temp.path).unwrap();
    if ancestor.uid() == 0 {
        // A root-created fixture cannot exercise owner read-permission denial.
        return;
    }
    let _restore = RestorePermissions {
        path: temp.path.clone(),
        permissions: ancestor.permissions(),
    };
    fs::set_permissions(&temp.path, fs::Permissions::from_mode(0o111)).unwrap();
    assert_eq!(
        fs::File::open(&temp.path).unwrap_err().kind(),
        ErrorKind::PermissionDenied,
    );

    // A sandbox may allow the owned store while denying reads of a parent.
    let probe = directory.join("write-probe");
    let mut file = fs::File::create(&probe).unwrap();
    file.write_all(b"writable owned directory").unwrap();
    file.sync_all().unwrap();
    drop(file);
    fs::remove_file(probe).unwrap();

    let mut store = FileStore::new(&directory);
    let sealed = sealed_timebase();
    store.store(SnapshotRegion::Timebase, &sealed).unwrap();
    let mut loaded = [0; TIMEBASE_SNAPSHOT_LEN];
    assert_eq!(
        store.load(SnapshotRegion::Timebase, &mut loaded).unwrap(),
        Some(sealed.as_slice()),
    );
}

#[cfg(unix)]
#[test]
fn a_nested_new_store_stops_confirmation_at_its_existing_writable_parent() {
    use std::os::unix::fs::MetadataExt as _;

    let temp = TempDir::new();
    let app_directory = temp.path.join("app");
    fs::create_dir_all(&app_directory).unwrap();
    let ancestor = fs::metadata(&temp.path).unwrap();
    if ancestor.uid() == 0 {
        // A root-created fixture cannot exercise owner read-permission denial.
        return;
    }
    let _restore = RestorePermissions {
        path: temp.path.clone(),
        permissions: ancestor.permissions(),
    };
    fs::set_permissions(&temp.path, fs::Permissions::from_mode(0o111)).unwrap();
    assert_eq!(
        fs::File::open(&temp.path).unwrap_err().kind(),
        ErrorKind::PermissionDenied,
    );

    let directory = app_directory.join("nested/store");
    assert!(!directory.exists());
    let mut store = FileStore::new(&directory);
    store.prepare_directory().unwrap();
    assert!(directory.is_dir());

    let sealed = sealed_timebase();
    store.store(SnapshotRegion::Timebase, &sealed).unwrap();
    let mut loaded = [0; TIMEBASE_SNAPSHOT_LEN];
    assert_eq!(
        store.load(SnapshotRegion::Timebase, &mut loaded).unwrap(),
        Some(sealed.as_slice()),
    );
}

#[cfg(unix)]
#[test]
fn failed_initial_confirmation_retries_the_original_creation_boundary() {
    let temp = TempDir::new();
    fs::create_dir(&temp.path).unwrap();
    let existing = fs::canonicalize(&temp.path).unwrap();
    let parent = existing.join("nested");
    let directory = parent.join("store");
    let mut store = FileStore::new(&directory);
    let mut attempted = Vec::new();
    let error = store
        .prepare_directory_with(|path| {
            attempted.push(path.to_path_buf());
            if path == parent {
                return Err(std::io::Error::from(ErrorKind::Other));
            }
            confirm_directory(path)
        })
        .unwrap_err();
    assert!(matches!(
        error,
        FileStoreError::DurabilityUnconfirmed(error) if error.kind() == ErrorKind::Other
    ));
    assert_eq!(attempted, [directory.clone(), parent.clone()]);
    assert!(directory.is_dir());

    // Creation has already happened, but its original parent obligations remain.
    let mut retried = Vec::new();
    store
        .prepare_directory_with(|path| {
            retried.push(path.to_path_buf());
            confirm_directory(path)
        })
        .unwrap();
    assert_eq!(retried, [directory, parent, existing]);
    store
        .prepare_directory_with(|_| panic!("successful preparation must cache readiness"))
        .unwrap();

    store
        .store(SnapshotRegion::Timebase, &sealed_timebase())
        .unwrap();
}

#[test]
fn io_error_conversion_preserves_kind_and_typed_failure() {
    for (failure, expected_kind) in [
        (
            FileStoreError::Io(std::io::Error::from(ErrorKind::PermissionDenied)),
            ErrorKind::PermissionDenied,
        ),
        (
            FileStoreError::DurabilityUnconfirmed(std::io::Error::from(ErrorKind::Other)),
            ErrorKind::Other,
        ),
        (
            FileStoreError::PublishedDurabilityUnconfirmed(std::io::Error::from(
                ErrorKind::Interrupted,
            )),
            ErrorKind::Interrupted,
        ),
        (
            FileStoreError::SnapshotOutgrewBuffer {
                snapshot_len: 10,
                buffer_len: 1,
            },
            ErrorKind::InvalidData,
        ),
    ] {
        let variant = std::mem::discriminant(&failure);
        let detail = failure.to_string();
        let converted = std::io::Error::from(failure);
        assert_eq!(converted.kind(), expected_kind);
        let retained = converted
            .get_ref()
            .unwrap()
            .downcast_ref::<FileStoreError>()
            .unwrap();
        assert_eq!(std::mem::discriminant(retained), variant);
        assert_eq!(retained.to_string(), detail);
    }
}

#[test]
fn a_missing_region_is_a_clean_miss_not_an_error() {
    let temp = TempDir::new();
    let store = FileStore::new(&temp.path);
    assert_eq!(store.stored_len(SnapshotRegion::Timebase).unwrap(), None);
    let mut buf = [0u8; TIMEBASE_SNAPSHOT_LEN];
    assert!(store
        .load(SnapshotRegion::Timebase, &mut buf)
        .unwrap()
        .is_none());
}

#[test]
fn failed_namespace_confirmation_retains_the_published_snapshot_and_typed_error() {
    let temp = TempDir::new();
    let mut store = FileStore::new(temp.path.join("nested/store"));
    store
        .store(SnapshotRegion::Timebase, &sealed_timebase())
        .unwrap();
    let mut candidate = [0u8; TIMEBASE_SNAPSHOT_LEN];
    let len = write_timebase_snapshot(InstantMillis(HIGH_WATER.0 + 1), &mut candidate).unwrap();
    let candidate = &candidate[..len];
    let error = store
        .store_with_confirmation(SnapshotRegion::Timebase, candidate, |directory| {
            assert_eq!(fs::read(directory.join("timebase")).unwrap(), candidate);
            Err(std::io::Error::from(ErrorKind::Other))
        })
        .unwrap_err();
    assert!(
        matches!(error, FileStoreError::PublishedDurabilityUnconfirmed(error) if error.kind() == ErrorKind::Other)
    );
    let mut loaded = [0u8; TIMEBASE_SNAPSHOT_LEN];
    assert_eq!(
        FileStore::new(store.dir())
            .load(SnapshotRegion::Timebase, &mut loaded)
            .unwrap(),
        Some(candidate)
    );
    assert_eq!(
        store
            .confirm_store(SnapshotRegion::Timebase, candidate)
            .unwrap(),
        FileStoreConfirmation::Confirmed
    );
    assert_eq!(
        store.load(SnapshotRegion::Timebase, &mut loaded).unwrap(),
        Some(candidate)
    );
}

#[cfg(unix)]
#[test]
fn directory_confirmation_errors_are_not_plain_write_failures() {
    let temp = TempDir::new();
    assert!(matches!(
        sync_directory(&temp.path),
        Err(FileStoreError::DurabilityUnconfirmed(_))
    ));
}

#[test]
fn confirmation_compares_the_whole_value_without_replacing_files() {
    let temp = TempDir::new();
    let mut store = FileStore::new(&temp.path);
    let region = SnapshotRegion::RemoteControlControllerGrants;
    assert_eq!(
        store.confirm_store(region, b"candidate").unwrap(),
        FileStoreConfirmation::Missing
    );
    assert!(!temp.path.exists());

    for len in [0, 1, 511, 512, 513, 1025] {
        let candidate: Vec<_> = (0..len).map(|index| (index % 251) as u8).collect();
        store.store(region, &candidate).unwrap();
        let path = store.path_for(region);
        let before = fs::metadata(&path).unwrap();
        let staging = store.dir.join(format!(
            ".{}.{}.staging",
            region_file_name(region),
            std::process::id()
        ));
        fs::write(&staging, b"untouched staging").unwrap();
        for _ in 0..2 {
            assert_eq!(
                store.confirm_store(region, &candidate).unwrap(),
                FileStoreConfirmation::Confirmed
            );
        }
        let mut different = candidate.clone();
        different.push(7);
        assert_eq!(
            store.confirm_store(region, &different).unwrap(),
            FileStoreConfirmation::Different
        );
        if let Some((_, shorter)) = candidate.split_last() {
            assert_eq!(
                store.confirm_store(region, shorter).unwrap(),
                FileStoreConfirmation::Different
            );
        }
        for index in 0..candidate.len() {
            let mut changed = candidate.clone();
            changed[index] ^= 1;
            assert_eq!(
                store.confirm_store(region, &changed).unwrap(),
                FileStoreConfirmation::Different
            );
        }
        assert_eq!(fs::read(&path).unwrap(), candidate);
        assert_eq!(fs::read(&staging).unwrap(), b"untouched staging");
        let after = fs::metadata(&path).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        }
        fs::remove_file(staging).unwrap();
    }
}

#[test]
fn failed_confirmation_is_retryable_and_region_specific() {
    let temp = TempDir::new();
    let mut store = FileStore::new(&temp.path);
    for region in [
        SnapshotRegion::RemoteControlControllerGrants,
        SnapshotRegion::RemoteControlTargetAccesses,
    ] {
        let candidate = region_file_name(region).as_bytes();
        store.store(region, candidate).unwrap();
        for _ in 0..2 {
            assert!(matches!(
                store.confirm_store_with(region, candidate, |_| {
                    Err(std::io::Error::from(ErrorKind::Other))
                }),
                Err(FileStoreError::PublishedDurabilityUnconfirmed(_))
            ));
        }
        assert_eq!(
            FileStore::new(&temp.path)
                .confirm_store(region, candidate)
                .unwrap(),
            FileStoreConfirmation::Confirmed
        );
        assert_eq!(
            store
                .confirm_store_with(region, b"wrong", |_| {
                    panic!("mismatched content must not reach durability confirmation")
                })
                .unwrap(),
            FileStoreConfirmation::Different
        );
    }
}

#[test]
fn failure_before_publication_never_reports_a_published_candidate() {
    let temp = TempDir::new();
    fs::write(&temp.path, b"not a directory").unwrap();
    let mut store = FileStore::new(&temp.path);
    let result = store.store_with_confirmation(SnapshotRegion::Timebase, b"candidate", |_| {
        panic!("failed staging must not reach publication confirmation")
    });
    assert!(matches!(result, Err(FileStoreError::Io(_))));
    assert_eq!(fs::read(&temp.path).unwrap(), b"not a directory");
    fs::remove_file(&temp.path).unwrap();
}

#[test]
fn failed_staging_preserves_the_previous_snapshot() {
    let temp = TempDir::new();
    let mut store = FileStore::new(&temp.path);
    let region = SnapshotRegion::RemoteControlTargetAccesses;
    store.store(region, b"previous").unwrap();
    let staging = store.dir.join(format!(
        ".{}.{}.staging",
        region_file_name(region),
        std::process::id()
    ));
    fs::create_dir(&staging).unwrap();
    assert!(matches!(
        store.store_with_confirmation(region, b"candidate", |_| {
            panic!("failed staging must not reach publication confirmation")
        }),
        Err(FileStoreError::Io(_))
    ));
    assert_eq!(fs::read(store.path_for(region)).unwrap(), b"previous");
    assert_eq!(
        store.confirm_store(region, b"candidate").unwrap(),
        FileStoreConfirmation::Different
    );
}

#[test]
fn destination_identity_region_retains_rns_known_destinations_filename() {
    assert_eq!(
        region_file_name(SnapshotRegion::DestinationIdentities),
        "known_destinations"
    );
}

#[test]
fn remote_control_regions_have_distinct_directional_filenames() {
    assert_eq!(
        region_file_name(SnapshotRegion::RemoteControlControllerGrants),
        "remote_control_controller_grants"
    );
    assert_eq!(
        region_file_name(SnapshotRegion::RemoteControlTargetAccesses),
        "remote_control_target_accesses"
    );
}

#[test]
fn a_buffer_shorter_than_the_snapshot_is_refused_by_name() {
    let temp = TempDir::new();
    let mut store = FileStore::new(&temp.path);
    let sealed = sealed_timebase();
    store.store(SnapshotRegion::Timebase, &sealed).unwrap();

    let mut short = [0u8; 4];
    match store.load(SnapshotRegion::Timebase, &mut short) {
        Err(FileStoreError::SnapshotOutgrewBuffer {
            snapshot_len,
            buffer_len,
        }) => {
            assert_eq!(snapshot_len, sealed.len());
            assert_eq!(buffer_len, 4);
        }
        other => panic!("expected SnapshotOutgrewBuffer, got {other:?}"),
    }
}

#[test]
fn on_disk_bit_rot_refuses_at_the_envelope() {
    let temp = TempDir::new();
    let mut store = FileStore::new(&temp.path);
    store
        .store(SnapshotRegion::Timebase, &sealed_timebase())
        .unwrap();

    let path = temp.path.join("timebase");
    let mut rotted = fs::read(&path).unwrap();
    let last = rotted.len() - 5;
    rotted[last] ^= 0x40;
    fs::write(&path, &rotted).unwrap();

    let mut buf = [0u8; TIMEBASE_SNAPSHOT_LEN];
    let loaded = store
        .load(SnapshotRegion::Timebase, &mut buf)
        .unwrap()
        .unwrap();
    assert_eq!(
        read_timebase_snapshot(loaded),
        Err(SnapshotReadError::Envelope(
            SnapshotOpenError::ChecksumMismatch
        )),
    );
}

#[cfg(unix)]
#[test]
fn a_stored_snapshot_is_owner_only_on_disk() {
    let temp = TempDir::new();
    let mut store = FileStore::new(&temp.path);
    store
        .store(SnapshotRegion::Timebase, &sealed_timebase())
        .unwrap();
    let mode = fs::metadata(temp.path.join("timebase"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn remove_reports_presence_then_absence() {
    let temp = TempDir::new();
    let mut store = FileStore::new(&temp.path);
    store
        .store(SnapshotRegion::Timebase, &sealed_timebase())
        .unwrap();
    assert_eq!(
        store.remove(SnapshotRegion::Timebase).unwrap(),
        Removal::Removed,
    );
    assert_eq!(
        store.remove(SnapshotRegion::Timebase).unwrap(),
        Removal::NothingStored,
    );
}
