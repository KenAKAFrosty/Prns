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
