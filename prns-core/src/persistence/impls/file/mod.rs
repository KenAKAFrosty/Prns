use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::identity::vault::Removal;
use crate::persistence::{PersistedStore, SnapshotRegion};

/// File contents are synced before replacement. Unix additionally syncs namespace
/// mutations; other platforms retain file-sync-and-rename semantics only.
pub struct FileStore {
    dir: PathBuf,
    dir_ready: bool,
}

#[derive(Debug)]
pub enum FileStoreError {
    Io(std::io::Error),
    DurabilityUnconfirmed(std::io::Error),
    /// Replacement has occurred; this error must not be treated as a failed write.
    PublishedDurabilityUnconfirmed(std::io::Error),
    SnapshotOutgrewBuffer {
        snapshot_len: usize,
        buffer_len: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum FileStoreConfirmation {
    Confirmed,
    Missing,
    Different,
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
        #[cfg(unix)]
        {
            // A previous failed attempt may already have created any of these directories.
            // Confirm the full chain before caching readiness, including on retries.
            let absolute = fs::canonicalize(&self.dir)?;
            for directory in absolute.ancestors() {
                sync_directory(directory)?;
            }
        }
        self.dir_ready = true;
        Ok(())
    }

    fn store_with_confirmation(
        &mut self,
        region: SnapshotRegion,
        snapshot: &[u8],
        confirm: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<(), FileStoreError> {
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
        staged?;
        confirm(&self.dir).map_err(FileStoreError::PublishedDurabilityUnconfirmed)
    }

    /// Confirms the exact published value without rewriting it. The owner must
    /// exclude concurrent writes and removals until confirmation and activation settle.
    /// Unix confirms directory durability; other platforms retain file-sync semantics.
    pub fn confirm_store(
        &self,
        region: SnapshotRegion,
        snapshot: &[u8],
    ) -> Result<FileStoreConfirmation, FileStoreError> {
        self.confirm_store_with(region, snapshot, confirm_directory)
    }

    fn confirm_store_with(
        &self,
        region: SnapshotRegion,
        snapshot: &[u8],
        confirm: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<FileStoreConfirmation, FileStoreError> {
        let mut file = match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.path_for(region))
        {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(FileStoreConfirmation::Missing);
            }
            Err(error) => return Err(error.into()),
        };
        const COMPARISON_CHUNK_LEN: usize = 512;
        let mut buffer = [0; COMPARISON_CHUNK_LEN];
        for chunk in snapshot.chunks(COMPARISON_CHUNK_LEN) {
            match file.read_exact(&mut buffer[..chunk.len()]) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::UnexpectedEof => {
                    return Ok(FileStoreConfirmation::Different);
                }
                Err(error) => return Err(error.into()),
            }
            if &buffer[..chunk.len()] != chunk {
                return Ok(FileStoreConfirmation::Different);
            }
        }
        if file.read(&mut buffer[..1])? != 0 {
            return Ok(FileStoreConfirmation::Different);
        }
        file.sync_all()
            .and_then(|()| confirm(&self.dir))
            .map_err(FileStoreError::PublishedDurabilityUnconfirmed)?;
        Ok(FileStoreConfirmation::Confirmed)
    }
}

fn confirm_directory(directory: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(directory)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
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
        SnapshotRegion::RemoteControlControllerGrants => "remote_control_controller_grants",
        SnapshotRegion::RemoteControlTargetAccesses => "remote_control_target_accesses",
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
        self.store_with_confirmation(region, snapshot, confirm_directory)
    }

    fn remove(&mut self, region: SnapshotRegion) -> Result<Removal, Self::Error> {
        match fs::remove_file(self.path_for(region)) {
            Ok(()) => {
                #[cfg(unix)]
                sync_directory(&self.dir)?;
                Ok(Removal::Removed)
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                #[cfg(unix)]
                if self.dir.try_exists()? {
                    sync_directory(&self.dir)?;
                }
                Ok(Removal::NothingStored)
            }
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

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), FileStoreError> {
    confirm_directory(path).map_err(FileStoreError::DurabilityUnconfirmed)
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
            FileStoreError::DurabilityUnconfirmed(error) => write!(
                formatter,
                "snapshot namespace durability is unconfirmed: {error}"
            ),
            FileStoreError::PublishedDurabilityUnconfirmed(error) => write!(
                formatter,
                "snapshot was published but durability is unconfirmed: {error}"
            ),
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
            FileStoreError::Io(error)
            | FileStoreError::DurabilityUnconfirmed(error)
            | FileStoreError::PublishedDurabilityUnconfirmed(error) => Some(error),
            FileStoreError::SnapshotOutgrewBuffer { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests;
