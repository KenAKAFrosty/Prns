//! Installation paths and process ownership for the app's selected native image.
//!
//! Creation and reopening are distinct. An incomplete installation is preserved;
//! neither missing keys nor failed startup automatically permits replacement.
//! Explicit reset captures the native platform's exact installation and waits for
//! all owners to close before removing it. The native owner remains the sole
//! startup/cleanup authority across JS reloads.
#![allow(
    clippy::result_large_err,
    reason = "Infrequent bootstrap and reset failures retain the canonical cleanup status for recovery"
)]
use prns_core::identity::IdentityHash;
use prns_host::{
    HostConfig, HostRole, IdentityConfig, IdentitySecret, PersistenceConfig, PrnsLimits,
};
use prns_host_runtime::connections::{Profile, ProfileOwner};
use prns_host_runtime::HostEmbedding;
use prns_service_runtime_uniffi::{
    host::{connections_value::ConnectionError, ConnectionProfileClient},
    host::{transport::BindingError, RemoteControlOperations},
    identities::KeyError,
    identity::keys::KeyOwner,
    runtime::groups::standalone::{self, Phase, Status},
    ServiceRuntime, ServiceRuntimeClose, ServiceRuntimeNetworkChange,
};
use prns_storage::files;
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

mod provisioning;
use provisioning::Provisioning;
pub use provisioning::{
    app_cancel_phone_identity_import, app_pending_phone_identity_import,
    app_prepare_retained_phone_identity_import, app_retain_phone_identity_import,
    app_review_phone_identity_file, app_start_retained_phone_identity_import, PhoneIdentityImport,
    PhoneIdentityImportError, PhoneIdentityImportSelection,
};

mod reset;
pub use reset::{app_capture_reset_root, app_prepare_reset, app_reset, AppResetReview};

const DIRECTORY: &str = "v2";
const MARKER: &str = "installation";
const MAGIC: &[u8; 8] = b"PRNSAPP\0";
const VERSION: u32 = 2;
const MARKER_BYTES: usize = 8 + 4 + 16 + 16;

#[derive(Debug, uniffi::Error)]
pub enum AppBootstrapError {
    NotStarted,
    StartInProgress,
    DifferentInstallation,
    MissingInstallation,
    InvalidInstallation,
    ImportRequiresNewInstallation,
    StaleImportReview,
    UnsupportedInstallation { version: u32 },
    InvalidPath,
    StaleResetReview,
    CleanupPending { status: Status },
    ControllerKey { error: KeyError },
    ManagementTargetKey { error: KeyError },
    Connections { error: ConnectionError },
    Native { error: standalone::Error },
}
impl std::fmt::Display for AppBootstrapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "app installation: {self:?}")
    }
}
impl std::error::Error for AppBootstrapError {}
impl From<files::Error> for AppBootstrapError {
    fn from(error: files::Error) -> Self {
        Self::Native {
            error: error.into(),
        }
    }
}
impl From<std::io::Error> for AppBootstrapError {
    fn from(error: std::io::Error) -> Self {
        Self::Native {
            error: standalone::Error::Io {
                code: error.raw_os_error(),
            },
        }
    }
}
impl From<standalone::Error> for AppBootstrapError {
    fn from(error: standalone::Error) -> Self {
        Self::Native { error }
    }
}
impl From<prns_host_runtime::connections::Error> for AppBootstrapError {
    fn from(error: prns_host_runtime::connections::Error) -> Self {
        Self::Connections {
            error: error.into(),
        }
    }
}
type Result<T> = std::result::Result<T, AppBootstrapError>;
type InitialProfiles =
    fn() -> std::result::Result<Vec<Profile>, prns_host_runtime::connections::Error>;

struct Installation {
    owner: Arc<ServiceRuntime>,
    profiles: Arc<ProfileOwner>,
}
struct State {
    // Retained even when initial construction fails before an owner is installed.
    // Only native start/open supplies this private root; JS reset accepts no path.
    known_root: Option<PathBuf>,
    construction_generation: u64,
    failed_profiles: Option<Arc<ProfileOwner>>,
    pending_import: Option<provisioning::RetainedPhoneIdentityImport>,
    // Process-local results survive JS leases and installation Stop/Start.
    remote_operations: Option<Arc<RemoteControlOperations>>,
    installation: Option<Installation>,
    // Only serializes construction before a canonical Owner exists. Native
    // startup, readiness, cleanup and failure remain in that Owner's status.
    construction: Option<PathBuf>,
}

/// Native platform integration owns one registry for its process. This type is
/// not exported to foreign consumers; the production instance below survives JS
/// reloads. Separate instances are useful for isolated installation tests.
pub struct Registry {
    state: Mutex<State>,
    initial_profiles: InitialProfiles,
}
impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
impl Registry {
    pub const fn new() -> Self {
        Self::with_initial_connections(crate::policy::initial_connection_profiles)
    }
    /// Select initial product policy before construction. This is a Rust-only
    /// seam for isolated offline fixtures; reopening never invokes the factory.
    pub const fn with_initial_connections(initial_profiles: InitialProfiles) -> Self {
        Self {
            state: Mutex::new(State {
                known_root: None,
                construction_generation: 0,
                failed_profiles: None,
                pending_import: None,
                remote_operations: None,
                installation: None,
                construction: None,
            }),
            initial_profiles,
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn remote_control_operations(
        &self,
    ) -> std::result::Result<Arc<RemoteControlOperations>, BindingError> {
        let mut state = self.lock();
        if state.construction.is_some() {
            return Err(BindingError::Busy);
        }
        if let Some(operations) = &state.remote_operations {
            return Ok(operations.clone());
        }
        // This canonical constructor only allocates the bounded record owner;
        // it starts no runtime, dispatch, persistence, or foreign callbacks.
        let operations = RemoteControlOperations::new(crate::policy::REMOTE_CONTROL_OPERATIONS)?;
        state.remote_operations = Some(operations.clone());
        Ok(operations)
    }
    pub fn owner(&self) -> Result<Arc<ServiceRuntime>> {
        let state = self.lock();
        // A previous, already-closed generation may remain installed while
        // strict reopening constructs its replacement. Never let Stop observe
        // that old completed close as success for the pending new generation.
        if state.construction.is_some() {
            return Err(AppBootstrapError::StartInProgress);
        }
        state
            .installation
            .as_ref()
            .map(|entry| entry.owner.clone())
            .ok_or(AppBootstrapError::NotStarted)
    }
    /// Saved settings outlive each host generation. This borrowed client has no
    /// authority to close the store or its current network binding.
    pub fn connection_profiles(&self) -> Result<Arc<ConnectionProfileClient>> {
        let profiles = {
            let state = self.lock();
            if state.construction.is_some() {
                return Err(AppBootstrapError::StartInProgress);
            }
            state
                .installation
                .as_ref()
                .map(|entry| entry.profiles.clone())
                .ok_or(AppBootstrapError::NotStarted)?
        };
        Ok(ConnectionProfileClient::borrow(profiles.client()))
    }
    pub fn start_new(
        &self,
        root: impl AsRef<Path>,
        embedding: HostEmbedding,
    ) -> Result<Arc<ServiceRuntime>> {
        self.start(root.as_ref(), Provisioning::Create, Some(embedding))
    }
    pub fn open_existing(
        &self,
        root: impl AsRef<Path>,
        embedding: HostEmbedding,
    ) -> Result<Arc<ServiceRuntime>> {
        self.start(root.as_ref(), Provisioning::Open, Some(embedding))
    }
    /// Native platform preparation before foreground-service or radio admission.
    pub fn prepare_new(&self, root: impl AsRef<Path>) -> Result<Arc<ServiceRuntime>> {
        self.start(root.as_ref(), Provisioning::Create, None)
    }
    pub fn prepare_existing(&self, root: impl AsRef<Path>) -> Result<Arc<ServiceRuntime>> {
        self.start(root.as_ref(), Provisioning::Open, None)
    }
    pub fn stop(&self) -> Result<Arc<ServiceRuntimeNetworkChange>> {
        Ok(self.owner()?.stop_networking()?)
    }
    /// Full installation close is separate from stopping networking.
    pub fn shutdown(&self) -> Result<Arc<ServiceRuntimeClose>> {
        Ok(self.owner()?.shutdown())
    }
    fn start(
        &self,
        root: &Path,
        provisioning: Provisioning<'_>,
        embedding: Option<HostEmbedding>,
    ) -> Result<Arc<ServiceRuntime>> {
        // The platform supplies its existing private application directory. The
        // canonical path binds subsequent calls; symlink roots are rejected.
        files::check_private_directory(root)?;
        let root = std::fs::canonicalize(root)?;
        if root.to_str().is_none() {
            return Err(AppBootstrapError::InvalidPath);
        }
        let (existing, mut profiles) = {
            let mut state = self.lock();
            if let Some(known) = &state.known_root {
                if known != &root {
                    return Err(AppBootstrapError::DifferentInstallation);
                }
            }
            if state.construction.is_some() {
                return Err(AppBootstrapError::StartInProgress);
            }
            provisioning.validate_import(&state)?;
            state.known_root = Some(root.clone());
            if provisioning.imports() && state.installation.is_some() {
                return Err(AppBootstrapError::ImportRequiresNewInstallation);
            }
            let existing = state
                .installation
                .as_ref()
                .filter(|entry| {
                    matches!(entry.owner.status().phase, Phase::Starting | Phase::Ready)
                })
                .map(|entry| entry.owner.clone());
            if existing.is_none() {
                if let Some(entry) = &state.installation {
                    let status = entry.owner.status();
                    if !can_replace(&status) {
                        return Err(AppBootstrapError::CleanupPending { status });
                    }
                }
                state.construction_generation = state
                    .construction_generation
                    .checked_add(1)
                    .ok_or(AppBootstrapError::InvalidInstallation)?;
            }
            let profiles = state
                .installation
                .as_ref()
                .map(|entry| entry.profiles.clone())
                .or_else(|| state.failed_profiles.clone());
            // One admission covers the existing-owner decision and construction.
            // A concurrent local prepare cannot slip between two checks and turn
            // this network start into an accidental offline success.
            state.construction = Some(root.clone());
            (existing, profiles)
        };
        let claim = Construction { registry: self };
        if let Some(owner) = existing {
            if let Some(embedding) = embedding {
                if !matches!(
                    owner.network_status().phase,
                    standalone::NetworkPhase::Starting | standalone::NetworkPhase::Ready
                ) {
                    let (host, embedding) = reopen_embedding(&root, embedding)?;
                    owner.start_networking_with_embedding(host, embedding)?;
                }
            }
            return Ok(owner);
        }
        if provisioning.imports() {
            match std::fs::symlink_metadata(root.join(DIRECTORY)) {
                Ok(_) => return Err(AppBootstrapError::ImportRequiresNewInstallation),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        // No registry lock spans filesystem I/O, native construction, callbacks,
        // or destruction of the previous owner / unused embedding resources.
        let owner = match construct(&root, &provisioning, &mut profiles, self.initial_profiles) {
            Ok(owner) => owner,
            Err(error) => {
                let retired = std::mem::replace(&mut self.lock().failed_profiles, profiles);
                drop(retired);
                return Err(error);
            }
        };
        let profiles = profiles.ok_or(AppBootstrapError::InvalidInstallation)?;
        let owner = Arc::new(ServiceRuntime::from_owner(owner));
        let retired = {
            let mut state = self.lock();
            state.failed_profiles = None;
            (
                state.installation.replace(Installation {
                    owner: owner.clone(),
                    profiles,
                }),
                provisioning.finish_import(&mut state, owner.clone()),
            )
        };
        drop(retired);
        if let Some(embedding) = embedding {
            let (host, embedding) = reopen_embedding(&root, embedding)?;
            owner.start_networking_with_embedding(host, embedding)?;
        }
        drop(claim);
        Ok(owner)
    }
}
struct Construction<'a> {
    registry: &'a Registry,
}
impl Drop for Construction<'_> {
    fn drop(&mut self) {
        let retired = self.registry.lock().construction.take();
        drop(retired);
    }
}
fn can_replace(status: &Status) -> bool {
    status.cleanup_complete && matches!(status.phase, Phase::Closed | Phase::Failed)
}

fn construct(
    root: &Path,
    provisioning: &Provisioning<'_>,
    profiles: &mut Option<Arc<ProfileOwner>>,
    initial_profiles: InitialProfiles,
) -> Result<standalone::Owner> {
    let directory = root.join(DIRECTORY);
    let controller = directory.join("controller");
    let target = directory.join("management-target");
    let host_directory = directory.join("host");
    let identities = directory.join("identities");
    let create = provisioning.creates();
    let (key, target) = if create {
        files::create_private_directory(&directory)?;
        let key = provisioning.create_controller(&controller)?;
        let target = KeyOwner::create_new(&target).map_err(target_error)?;
        files::create_private_directory(&host_directory)?;
        write_marker(&directory, key.identity_hash(), target.identity_hash())?;
        (key, target)
    } else {
        files::check_private_directory(&directory)?;
        let expected = read_marker(&directory)?;
        files::check_private_directory(&host_directory)?;
        (
            KeyOwner::open_existing(&controller, expected.0).map_err(controller_error)?,
            KeyOwner::open_existing(&target, expected.1).map_err(target_error)?,
        )
    };
    // Offline preparation validates immutable keys but retains no host lease.
    // A requested network start reopens these exact bindings independently.
    drop((key, target));
    let options = crate::policy::application_options();
    if profiles.is_none() {
        let path = directory.join("connections.redb");
        *profiles = Some(Arc::new(if create {
            ProfileOwner::create_new(
                path,
                initial_profiles()?,
                crate::policy::connection_options(),
            )?
        } else {
            ProfileOwner::open_existing(path, crate::policy::connection_options())?
        }));
    }
    let profiles = profiles
        .as_ref()
        .ok_or(AppBootstrapError::InvalidInstallation)?;
    let owner = if create {
        standalone::Owner::create_new_offline_with_connections(
            identities,
            options,
            profiles.clone(),
        )
    } else {
        standalone::Owner::open_existing_offline_with_connections(
            identities,
            options,
            profiles.clone(),
        )
    }?;
    Ok(owner)
}
fn reopen_embedding(
    root: &Path,
    mut embedding: HostEmbedding,
) -> Result<(HostConfig, HostEmbedding)> {
    let directory = root.join(DIRECTORY);
    files::check_private_directory(&directory)?;
    let expected = read_marker(&directory)?;
    let key = KeyOwner::open_existing(directory.join("controller"), expected.0)
        .map_err(controller_error)?;
    let target = KeyOwner::open_existing(directory.join("management-target"), expected.1)
        .map_err(target_error)?;
    let lease = key.lease();
    let host = HostConfig {
        identity: IdentityConfig::Existing(IdentitySecret::new(*lease.material().as_bytes())),
        persistence: PersistenceConfig::Directory {
            path: directory
                .join("host")
                .to_str()
                .ok_or(AppBootstrapError::InvalidPath)?
                .to_owned(),
        },
        role: HostRole::Endpoint,
        destinations: vec![],
        required_capabilities: vec![],
        limits: PrnsLimits::balanced(),
    };
    crate::remote_control::configure(&mut embedding, lease, target.lease());
    Ok((host, embedding))
}

fn controller_error(error: prns_service_runtime_uniffi::identity::Error) -> AppBootstrapError {
    AppBootstrapError::ControllerKey {
        error: error.into(),
    }
}
fn target_error(error: prns_service_runtime_uniffi::identity::Error) -> AppBootstrapError {
    AppBootstrapError::ManagementTargetKey {
        error: error.into(),
    }
}
fn write_marker(directory: &Path, identity: IdentityHash, target: IdentityHash) -> Result<()> {
    let mut bytes = [0_u8; MARKER_BYTES];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
    bytes[12..28].copy_from_slice(identity.as_bytes());
    bytes[28..].copy_from_slice(target.as_bytes());
    let mut file = files::create_private_file(&directory.join(MARKER))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    files::sync_directory(directory)?;
    Ok(())
}
fn read_marker(directory: &Path) -> Result<(IdentityHash, IdentityHash)> {
    let path = directory.join(MARKER);
    if !files::check_private_file(&path)? {
        return Err(AppBootstrapError::MissingInstallation);
    }
    let mut file = File::open(path)?;
    if file.metadata()?.len() != MARKER_BYTES as u64 {
        return Err(AppBootstrapError::InvalidInstallation);
    }
    let mut bytes = [0_u8; MARKER_BYTES];
    file.read_exact(&mut bytes)?;
    if file.read(&mut [0_u8; 1])? != 0 || &bytes[..8] != MAGIC {
        return Err(AppBootstrapError::InvalidInstallation);
    }
    let version = u32::from_le_bytes(
        bytes[8..12]
            .try_into()
            .map_err(|_| AppBootstrapError::InvalidInstallation)?,
    );
    if version != VERSION {
        return Err(AppBootstrapError::UnsupportedInstallation { version });
    }
    Ok((
        IdentityHash::new(
            bytes[12..28]
                .try_into()
                .map_err(|_| AppBootstrapError::InvalidInstallation)?,
        ),
        IdentityHash::new(
            bytes[28..]
                .try_into()
                .map_err(|_| AppBootstrapError::InvalidInstallation)?,
        ),
    ))
}

static PROCESS: Registry = Registry::new();

/// The platform performs only a presence check; the native owner validates this
/// exact format and its two immutable management identity bindings on reopen.
#[uniffi::export]
pub fn app_installation_directory() -> String {
    DIRECTORY.into()
}

/// The ordinary shared host operation owner, retained for this process. Results
/// survive JS reload and installation Stop/Start, never a process restart. Each
/// submission still supplies its captured canonical HostClientHandle.
#[uniffi::export]
pub fn app_remote_control_operations(
) -> std::result::Result<Arc<RemoteControlOperations>, BindingError> {
    PROCESS.remote_control_operations()
}

/// Native platform startup supplies trusted paths and platform delegates.
pub fn start_new(root: impl AsRef<Path>, embedding: HostEmbedding) -> Result<Arc<ServiceRuntime>> {
    PROCESS.start_new(root, embedding)
}
pub fn open_existing(
    root: impl AsRef<Path>,
    embedding: HostEmbedding,
) -> Result<Arc<ServiceRuntime>> {
    PROCESS.open_existing(root, embedding)
}
pub fn stop() -> Result<Arc<ServiceRuntimeNetworkChange>> {
    PROCESS.stop()
}

/// Borrow the process-owned canonical owner. Reloading or releasing a JS client
/// never relinquishes the registry's ownership or starts another installation.
#[uniffi::export]
pub fn app_owner() -> std::result::Result<Arc<ServiceRuntime>, AppBootstrapError> {
    PROCESS.owner()
}

#[uniffi::export]
pub fn app_connection_profiles(
) -> std::result::Result<Arc<ConnectionProfileClient>, AppBootstrapError> {
    PROCESS.connection_profiles()
}

/// Local data preparation precedes optional OS networking admission. The native
/// platform supplies its private root; JavaScript never selects a storage path.
#[uniffi::export]
pub fn app_prepare_new(
    storage_root: String,
) -> std::result::Result<Arc<ServiceRuntime>, AppBootstrapError> {
    PROCESS.prepare_new(storage_root)
}
#[uniffi::export]
pub fn app_prepare_existing(
    storage_root: String,
) -> std::result::Result<Arc<ServiceRuntime>, AppBootstrapError> {
    PROCESS.prepare_existing(storage_root)
}

/// Platform-generated construction hook. Swift/Kotlin derive and prepare the
/// private root; product JavaScript calls the platform's path-free start bridge.
/// The returned owner is retained immediately; observe its canonical ready()
/// method without blocking the platform lifecycle queue.
#[uniffi::export]
pub fn app_start_new(
    storage_root: String,
) -> std::result::Result<Arc<ServiceRuntime>, AppBootstrapError> {
    start_new(storage_root, platform_embedding())
}

/// Strict counterpart for an installation that already exists. A failure never
/// selects create_new or replaces missing/corrupt controller material.
#[uniffi::export]
pub fn app_open_existing(
    storage_root: String,
) -> std::result::Result<Arc<ServiceRuntime>, AppBootstrapError> {
    open_existing(storage_root, platform_embedding())
}

/// Fence synchronously and return the existing retained cleanup observer.
/// Platform teardown observes wait()/status() and retains ownership on failure.
#[uniffi::export]
pub fn app_stop() -> std::result::Result<Arc<ServiceRuntimeNetworkChange>, AppBootstrapError> {
    stop()
}

#[uniffi::export]
pub fn app_shutdown() -> std::result::Result<Arc<ServiceRuntimeClose>, AppBootstrapError> {
    PROCESS.shutdown()
}

fn platform_embedding() -> HostEmbedding {
    HostEmbedding {
        auto_lan_factory: prns_host_runtime::platform::mobile_auto_lan_factory(),
        ..HostEmbedding::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_confirmed_native_cleanup_allows_replacement() {
        for phase in [
            Phase::Starting,
            Phase::Ready,
            Phase::Closing,
            Phase::Failed,
            Phase::Unresolved,
            Phase::Closed,
        ] {
            for cleanup_complete in [false, true] {
                let status = Status {
                    phase,
                    failure: None,
                    cleanup_complete,
                    latest_identity_diagnostic: None,
                };
                assert_eq!(
                    can_replace(&status),
                    cleanup_complete && matches!(phase, Phase::Closed | Phase::Failed)
                );
            }
        }
    }
    #[cfg(unix)]
    #[test]
    #[allow(clippy::unwrap_used, clippy::panic)]
    fn stop_during_replacement_never_reports_the_old_completed_generation() {
        use std::{
            future::Future,
            sync::{
                atomic::{AtomicUsize, Ordering},
                Barrier,
            },
            task::{Context, Poll, Wake, Waker},
            time::{Duration, Instant},
        };
        struct ThreadWake(std::thread::Thread);
        impl Wake for ThreadWake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        fn wait<F: Future>(future: F) -> F::Output {
            let mut future = std::pin::pin!(future);
            let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if let Poll::Ready(result) = future.as_mut().poll(&mut Context::from_waker(&waker))
                {
                    return result;
                }
                assert!(Instant::now() < deadline, "bootstrap lifecycle stalled");
                std::thread::park_timeout(Duration::from_millis(5));
            }
        }
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "prns-app-replacement-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        files::create_private_directory(&path).unwrap();
        let registry = Arc::new(Registry::with_initial_connections(|| Ok(vec![])));
        let old = registry.start_new(&path, HostEmbedding::default()).unwrap();
        wait(old.ready()).unwrap();
        wait(registry.shutdown().unwrap().wait()).unwrap();
        assert!(old.status().cleanup_complete);

        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let thread = {
            let registry = registry.clone();
            let path = std::fs::canonicalize(&path).unwrap();
            let entered = entered.clone();
            let release = release.clone();
            std::thread::spawn(move || {
                // Hold the same private claim as start(), without a production
                // fault hook or filesystem-timing assumption.
                registry.lock().construction = Some(path);
                let claim = Construction {
                    registry: &registry,
                };
                entered.wait();
                release.wait();
                drop(claim);
            })
        };
        entered.wait();
        let lookup = registry.owner();
        let stop = registry.shutdown();
        release.wait();
        thread.join().unwrap();
        assert!(matches!(lookup, Err(AppBootstrapError::StartInProgress)));
        assert!(matches!(stop, Err(AppBootstrapError::StartInProgress)));

        let reopened = registry
            .open_existing(&path, HostEmbedding::default())
            .unwrap();
        assert!(!Arc::ptr_eq(&old, &reopened));
        wait(reopened.ready()).unwrap();
        wait(registry.shutdown().unwrap().wait()).unwrap();
        assert!(reopened.status().cleanup_complete);
        drop((reopened, old, registry));
        std::fs::remove_dir_all(path).unwrap();
    }
}
