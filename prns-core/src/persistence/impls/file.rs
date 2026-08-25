use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::identity::vault::Removal;
use crate::persistence::{PersistedStore, SnapshotRegion};

pub struct FileStore {
    dir: PathBuf,
    dir_ready: bool,
}

#[derive(Debug)]
pub enum FileStoreError {
    Io(std::io::Error),
    SnapshotOutgrewBuffer {
        snapshot_len: usize,
        buffer_len: usize,
    },
}

impl FileStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            dir_ready: false,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, region: SnapshotRegion) -> PathBuf {
        self.dir.join(region_file_name(region))
    }

    fn ensure_dir(&mut self) -> Result<(), FileStoreError> {
        if self.dir_ready {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        let _ = fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700));
        self.dir_ready = true;
        Ok(())
    }
}

fn region_file_name(region: SnapshotRegion) -> &'static str {
    match region {
        SnapshotRegion::Timebase => "timebase",
        SnapshotRegion::RoutingTable => "routing_table",
        SnapshotRegion::Tunnels => "tunnels",
        SnapshotRegion::SelfRatchets => "self_ratchets",
        SnapshotRegion::DestinationIdentities => "known_destinations",
        SnapshotRegion::RemoteControlAccess => "remote_control_access",
    }
}

impl PersistedStore for FileStore {
    type Error = FileStoreError;

    fn stored_len(&self, region: SnapshotRegion) -> Result<Option<usize>, Self::Error> {
        match fs::metadata(self.path_for(region)) {
            Ok(metadata) => usize::try_from(metadata.len())
                .map(Some)
                .map_err(|_| std::io::Error::from(ErrorKind::InvalidData).into()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn load<'b>(
        &self,
        region: SnapshotRegion,
        buf: &'b mut [u8],
    ) -> Result<Option<&'b [u8]>, Self::Error> {
        let mut file = match fs::File::open(self.path_for(region)) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let snapshot_len = usize::try_from(file.metadata()?.len())
            .map_err(|_| std::io::Error::from(ErrorKind::InvalidData))?;
        if snapshot_len > buf.len() {
            return Err(FileStoreError::SnapshotOutgrewBuffer {
                snapshot_len,
                buffer_len: buf.len(),
            });
        }
        file.read_exact(&mut buf[..snapshot_len])?;
        Ok(Some(&buf[..snapshot_len]))
    }

    fn store(&mut self, region: SnapshotRegion, snapshot: &[u8]) -> Result<(), Self::Error> {
        self.ensure_dir()?;
        let final_path = self.path_for(region);
        let staging_path = self.dir.join(format!(
            ".{}.{}.staging",
            region_file_name(region),
            std::process::id()
        ));

        let staged = stage_snapshot(&staging_path, snapshot)
            .and_then(|()| fs::rename(&staging_path, &final_path).map_err(FileStoreError::from));
        if staged.is_err() {
            let _ = fs::remove_file(&staging_path);
        }
        staged
    }

    fn remove(&mut self, region: SnapshotRegion) -> Result<Removal, Self::Error> {
        match fs::remove_file(self.path_for(region)) {
            Ok(()) => Ok(Removal::Removed),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Removal::NothingStored),
            Err(error) => Err(error.into()),
        }
    }
}

fn stage_snapshot(staging_path: &Path, snapshot: &[u8]) -> Result<(), FileStoreError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(staging_path)?;
    file.write_all(snapshot)?;
    file.sync_all()?;
    Ok(())
}

impl From<std::io::Error> for FileStoreError {
    fn from(error: std::io::Error) -> Self {
        FileStoreError::Io(error)
    }
}

impl core::fmt::Display for FileStoreError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FileStoreError::Io(error) => write!(formatter, "{error}"),
            FileStoreError::SnapshotOutgrewBuffer {
                snapshot_len,
                buffer_len,
            } => write!(
                formatter,
                "stored snapshot holds {snapshot_len} bytes, buffer holds {buffer_len}"
            ),
        }
    }
}

impl std::error::Error for FileStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FileStoreError::Io(error) => Some(error),
            FileStoreError::SnapshotOutgrewBuffer { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
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
    fn destination_identity_region_retains_rns_known_destinations_filename() {
        assert_eq!(
            region_file_name(SnapshotRegion::DestinationIdentities),
            "known_destinations"
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
}
