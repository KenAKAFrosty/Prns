//! Explicit product deletion, bound to a native-captured installation.
//!
//! A review is authority over exactly one directory identity, not an arbitrary
//! path supplied by JavaScript. Cleanup is completed before filesystem removal.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}
impl FileIdentity {
    fn read(path: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::symlink_metadata(path)?;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(AppBootstrapError::InvalidPath)
        }
    }
}
#[derive(PartialEq, Eq)]
struct MarkerIdentity {
    file: FileIdentity,
    length: u64,
    modified: std::time::SystemTime,
}
impl MarkerIdentity {
    fn read(path: &Path) -> Result<Option<Self>> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        // An invalid file can be explicitly reset; a marker directory cannot
        // masquerade as the installation's immutable marker.
        if metadata.is_dir() {
            return Err(AppBootstrapError::InvalidInstallation);
        }
        Ok(Some(Self {
            file: FileIdentity::read(path)?,
            length: metadata.len(),
            modified: metadata.modified()?,
        }))
    }
}

/// Opaque review captured from the platform's previously admitted private root.
/// Retaining this object does not start cleanup or retain additional key leases.
#[derive(uniffi::Object)]
pub struct AppResetReview {
    root: PathBuf,
    root_identity: FileIdentity,
    directory: FileIdentity,
    marker: Option<MarkerIdentity>,
    owner: Option<Arc<ServiceRuntime>>,
    construction_generation: u64,
    marker_removed: AtomicBool,
    directory_removed: AtomicBool,
    completed: AtomicBool,
}
impl AppResetReview {
    fn matches(&self, state: &State) -> bool {
        state.known_root.as_ref() == Some(&self.root)
            && state.construction_generation == self.construction_generation
            && match (&self.owner, &state.installation) {
                (Some(owner), Some(current)) => Arc::ptr_eq(owner, &current.owner),
                (None, None) => true,
                _ => false,
            }
    }
    fn verify(&self) -> Result<()> {
        files::check_private_directory(&self.root)?;
        if FileIdentity::read(&self.root)? != self.root_identity {
            return Err(AppBootstrapError::StaleResetReview);
        }
        let directory = self.root.join(DIRECTORY);
        if self.directory_removed.load(Ordering::Acquire) {
            return if std::fs::symlink_metadata(directory)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                Ok(())
            } else {
                Err(AppBootstrapError::StaleResetReview)
            };
        }
        files::check_private_directory(&directory)?;
        if FileIdentity::read(&directory)? != self.directory {
            return Err(AppBootstrapError::StaleResetReview);
        }
        let marker = MarkerIdentity::read(&directory.join(MARKER))?;
        let expected = if self.marker_removed.load(Ordering::Acquire) {
            None
        } else {
            self.marker.as_ref()
        };
        if marker.as_ref() != expected {
            return Err(AppBootstrapError::StaleResetReview);
        }
        Ok(())
    }
    fn remove_files(&self) -> Result<()> {
        self.verify()?;
        if !self.directory_removed.load(Ordering::Acquire) {
            let directory = self.root.join(DIRECTORY);
            // Leave the immutable marker until every other entry has gone. On
            // failure the same review can resume; no deleted entry is recreated.
            for entry in std::fs::read_dir(&directory)? {
                let entry = entry?;
                if entry.file_name() == MARKER {
                    continue;
                }
                if entry.file_type()?.is_dir() {
                    std::fs::remove_dir_all(entry.path())?;
                } else {
                    // Includes symlinks: unlink the entry, never its target.
                    std::fs::remove_file(entry.path())?;
                }
            }
            files::sync_directory(&directory)?;
            if !self.marker_removed.load(Ordering::Acquire) {
                if self.marker.is_some() {
                    std::fs::remove_file(directory.join(MARKER))?;
                }
                self.marker_removed.store(true, Ordering::Release);
            }
            files::sync_directory(&directory)?;
            std::fs::remove_dir(&directory)?;
            self.directory_removed.store(true, Ordering::Release);
        }
        files::sync_directory(&self.root)?;
        Ok(())
    }
}

impl Registry {
    /// Native platform authority for reset recovery before any owner was opened.
    /// This validates existing private directories and never creates or opens data.
    pub fn capture_reset_root(&self, root: impl AsRef<Path>) -> Result<()> {
        let root = root.as_ref();
        files::check_private_directory(root)?;
        let root = std::fs::canonicalize(root)?;
        files::check_private_directory(&root.join(DIRECTORY))?;
        let mut state = self.lock();
        if state.construction.is_some() {
            return Err(AppBootstrapError::StartInProgress);
        }
        if state
            .known_root
            .as_ref()
            .is_some_and(|known| known != &root)
        {
            return Err(AppBootstrapError::DifferentInstallation);
        }
        state.known_root = Some(root);
        Ok(())
    }
    /// Capture only a root already supplied by native startup. This also works
    /// after failed initial construction, without a registered ServiceRuntime.
    pub fn prepare_reset(&self) -> Result<Arc<AppResetReview>> {
        let (root, owner, construction_generation) = {
            let state = self.lock();
            if state.construction.is_some() {
                return Err(AppBootstrapError::StartInProgress);
            }
            (
                state
                    .known_root
                    .clone()
                    .ok_or(AppBootstrapError::NotStarted)?,
                state.installation.as_ref().map(|entry| entry.owner.clone()),
                state.construction_generation,
            )
        };
        files::check_private_directory(&root)?;
        let directory = root.join(DIRECTORY);
        files::check_private_directory(&directory)?;
        Ok(Arc::new(AppResetReview {
            root_identity: FileIdentity::read(&root)?,
            directory: FileIdentity::read(&directory)?,
            marker: MarkerIdentity::read(&directory.join(MARKER))?,
            root,
            owner,
            construction_generation,
            marker_removed: AtomicBool::new(false),
            directory_removed: AtomicBool::new(false),
            completed: AtomicBool::new(false),
        }))
    }
    /// Stop every installation owner before deleting its captured v2 directory.
    /// Dropping this future can finish shutdown but never continues deletion.
    /// Unresolved cleanup preserves files. A historical startup failure does not
    /// prevent explicit deletion once canonical cleanup_complete proves release.
    /// A filesystem error may have removed
    /// some entries; retry this review to finish without relaxing its identity.
    pub async fn reset(&self, review: Arc<AppResetReview>) -> Result<()> {
        let (profiles, completed) = {
            let mut state = self.lock();
            if state.construction.is_some() {
                return Err(AppBootstrapError::StartInProgress);
            }
            let completed = review.completed.load(Ordering::Acquire);
            if completed {
                if state.known_root.as_ref() != Some(&review.root)
                    || state.construction_generation != review.construction_generation
                    || state.installation.is_some()
                    || state.failed_profiles.is_some()
                {
                    return Err(AppBootstrapError::StaleResetReview);
                }
            } else if !review.matches(&state) {
                return Err(AppBootstrapError::StaleResetReview);
            }
            state.construction = Some(review.root.clone());
            (
                state
                    .installation
                    .as_ref()
                    .map(|entry| entry.profiles.clone())
                    .or_else(|| state.failed_profiles.clone()),
                completed,
            )
        };
        let _claim = Construction { registry: self };
        review.verify()?;
        if completed {
            return Ok(());
        }
        if let Some(owner) = &review.owner {
            let outcome = if owner.status().cleanup_complete {
                Ok(())
            } else {
                owner.shutdown().wait().await
            };
            let status = owner.status();
            if !status.cleanup_complete {
                return Err(outcome
                    .err()
                    .map(AppBootstrapError::from)
                    .unwrap_or(AppBootstrapError::CleanupPending { status }));
            }
        }
        if let Some(profiles) = profiles {
            profiles.close().await.map_err(|error| match error {
                prns_storage::CloseError::Failed(error) => {
                    AppBootstrapError::from(error.as_ref().clone())
                }
                prns_storage::CloseError::Panicked => {
                    AppBootstrapError::from(prns_host_runtime::connections::Error::WorkerFailed)
                }
                prns_storage::CloseError::Reentrant => {
                    AppBootstrapError::from(prns_host_runtime::connections::Error::Reentrant)
                }
            })?;
        }
        review.remove_files()?;
        let retired = {
            let mut state = self.lock();
            // The construction claim excludes replacement throughout awaits.
            (
                state.installation.take(),
                state.failed_profiles.take(),
                state.remote_operations.take(),
                state.pending_import.take(),
            )
        };
        drop(retired);
        review.completed.store(true, Ordering::Release);
        Ok(())
    }
}

#[uniffi::export]
pub fn app_prepare_reset() -> std::result::Result<Arc<AppResetReview>, AppBootstrapError> {
    PROCESS.prepare_reset()
}
#[uniffi::export]
pub async fn app_reset(review: Arc<AppResetReview>) -> std::result::Result<(), AppBootstrapError> {
    PROCESS.reset(review).await
}

/// Platform-only hook: the path comes from its existing private app directory.
/// Product JavaScript uses platform.prepareReset(), never supplies this path.
#[uniffi::export]
pub fn app_capture_reset_root(storage_root: String) -> std::result::Result<(), AppBootstrapError> {
    PROCESS.capture_reset_root(storage_root)
}
