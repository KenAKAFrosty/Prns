//! Explicit standalone ownership of a local database and its bounded worker.
//!
//! Borrowed composition continues to use `GroupManager` and `Submit` directly.
//! This owner does not create a host, activate networking, install a profile, or
//! initialize an identity catalog.

use crate::{Client, Error, GroupManager, Job, Provisioned, Submit};
use prns_host::IdentityHash;
use prns_storage::{database as physical, files, Completion, StartError, SubmitError, Worker};
use std::{num::NonZeroUsize, path::Path, sync::Arc};

/// A trusted native identity binding and an optional retained dependency.
///
/// Ordinary Rust callers can pass `IdentityHash` directly. Native binding
/// adapters derive the hash from their trusted key capability and retain that
/// capability here, so finalization cannot release it before the worker drains.
/// The group library neither derives nor validates private identity material.
pub struct LocalIdentity {
    hash: IdentityHash,
    _retained: Option<Box<dyn Send + Sync>>,
}
impl From<IdentityHash> for LocalIdentity {
    fn from(hash: IdentityHash) -> Self {
        Self {
            hash,
            _retained: None,
        }
    }
}
impl LocalIdentity {
    pub fn retain(hash: IdentityHash, guard: impl Send + Sync + 'static) -> Self {
        Self {
            hash,
            _retained: Some(Box::new(guard)),
        }
    }
}

/// Resource bounds for this standalone owner. There is no implicit cache budget.
#[derive(Clone, Copy, Debug)]
pub struct LocalOptions {
    /// Queued jobs; one additional job can be executing. Maximum: 1024.
    pub queue_capacity: NonZeroUsize,
    /// redb cache budget, from 1 MiB through 1 GiB, inclusive. This does not bound
    /// every allocation or reserve filesystem space.
    pub cache_bytes: usize,
    validation_cache_entries: Option<NonZeroUsize>,
    client: crate::ClientLimits,
    catalog: crate::enrollment::CatalogLimits,
    #[cfg(feature = "archive")]
    archive: Option<crate::archive::Limits>,
}
impl LocalOptions {
    pub fn new(queue_capacity: NonZeroUsize, cache_bytes: usize) -> Self {
        Self {
            queue_capacity,
            cache_bytes,
            validation_cache_entries: None,
            client: crate::ClientLimits::default(),
            catalog: crate::enrollment::CatalogLimits::default(),
            #[cfg(feature = "archive")]
            archive: None,
        }
    }
    /// Retain at most this many namespace validation proofs on the exclusive
    /// lane. This is a working set, not a group limit or redb page-cache budget.
    /// Omission preserves strict validation on every backend read.
    pub fn with_validation_cache_entries(mut self, entries: NonZeroUsize) -> Self {
        self.validation_cache_entries = Some(entries);
        self
    }

    /// Select one shared input allocation allowance for this owner's lane.
    pub fn with_client_limits(mut self, limits: crate::ClientLimits) -> Self {
        self.client = limits;
        self
    }
    /// Select enrollment capacity for a fresh managed store. Reopen uses its
    /// persisted limits; this never resizes or migrates existing state.
    pub fn with_catalog_limits(mut self, limits: crate::enrollment::CatalogLimits) -> Self {
        self.catalog = limits;
        self
    }
    /// Select local history when creating a fresh store. Reopening always uses
    /// the persisted selection and limits; these options never migrate a store.
    #[cfg(feature = "archive")]
    pub fn with_archive(mut self, limits: crate::archive::Limits) -> Self {
        self.archive = Some(limits);
        self
    }
    fn validate(self) -> Result<Self, OpenError> {
        if self.queue_capacity.get() > 1024
            || !(1024 * 1024..=1024 * 1024 * 1024).contains(&self.cache_bytes)
        {
            return Err(OpenError::InvalidOptions);
        }
        self.catalog
            .validate()
            .map_err(|_| OpenError::InvalidOptions)?;
        self.client
            .validate()
            .map_err(|_| OpenError::InvalidOptions)?;
        #[cfg(feature = "archive")]
        if let Some(limits) = self.archive {
            limits.validate().map_err(|_| OpenError::InvalidOptions)?;
        }
        Ok(self)
    }
}

/// Startup may fail after creating a file or committing its domain catalog.
/// Preserve that path and retry `open_existing` with the same identity; never
/// delete uncertain state or fall back to creation when reopening fails.
#[derive(Debug)]
pub enum OpenError {
    InvalidOptions,
    IdentityMismatch,
    UnsafePath,
    UnsafePermissions,
    UnsupportedPlatform,
    Io(std::io::Error),
    Database(redb::Error),
    Domain(Error),
    WorkerNotReady,
    WorkerPanicked,
}
impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "local group owner: {self:?}")
    }
}
impl std::error::Error for OpenError {}
impl From<std::io::Error> for OpenError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<files::Error> for OpenError {
    fn from(error: files::Error) -> Self {
        match error {
            files::Error::Io(error) => Self::Io(error),
            files::Error::UnsafePath => Self::UnsafePath,
            files::Error::UnsafePermissions => Self::UnsafePermissions,
            files::Error::UnsupportedPlatform => Self::UnsupportedPlatform,
        }
    }
}

struct Lane(prns_storage::Client<Box<Job>>, crate::InputBudget);
impl Submit for Lane {
    fn input_budget(&self) -> &crate::InputBudget {
        &self.1
    }
    fn submit(&self, job: Job) -> Result<(), Error> {
        self.0
            .try_submit(Box::new(job))
            .map_err(|error| match error {
                SubmitError::Full(_) => Error::Busy,
                SubmitError::Closed(_) | SubmitError::Reentrant(_) => Error::Unavailable,
            })
    }
}

enum Mode {
    Create,
    Existing,
    Fixed(Box<Provisioned>),
}

/// Owns one physical group database. Clients borrow submission authority and do
/// not keep admission open after this owner closes or drops.
///
/// Construction is blocking and should run off the UI thread. Close only fences
/// synchronously; its returned future observes cleanup on the existing worker.
pub struct LocalOwner {
    worker: Worker<Box<Job>, OpenError>,
    client: Client,
}
impl LocalOwner {
    /// Create a new private database and managed catalog. The immediate parent
    /// must already be a private directory. Existing paths are never overwritten.
    pub fn create_new(
        path: impl AsRef<Path>,
        identity: impl Into<LocalIdentity>,
        options: LocalOptions,
    ) -> Result<Self, OpenError> {
        Self::start(path.as_ref(), identity.into(), Mode::Create, options)
    }

    /// Reopen an existing managed catalog with the expected identity. Missing,
    /// partial, unsupported, or mismatched domain state is never initialized.
    /// redb may recover its own committed database state during ordinary open.
    pub fn open_existing(
        path: impl AsRef<Path>,
        identity: impl Into<LocalIdentity>,
        options: LocalOptions,
    ) -> Result<Self, OpenError> {
        Self::start(path.as_ref(), identity.into(), Mode::Existing, options)
    }

    /// Attach an already installed fixed membership. This neither installs the
    /// supplied provisioning nor creates a managed enrollment catalog.
    pub fn open_fixed(
        path: impl AsRef<Path>,
        provisioned: Provisioned,
        identity: impl Into<LocalIdentity>,
        options: LocalOptions,
    ) -> Result<Self, OpenError> {
        let identity = identity.into();
        if identity.hash != provisioned.local_reticulum_identity {
            return Err(OpenError::IdentityMismatch);
        }
        Self::start(
            path.as_ref(),
            identity,
            Mode::Fixed(Box::new(provisioned)),
            options,
        )
    }

    fn start(
        path: &Path,
        identity: LocalIdentity,
        mode: Mode,
        options: LocalOptions,
    ) -> Result<Self, OpenError> {
        let options = options.validate()?;
        let budget =
            crate::InputBudget::new(options.client).map_err(|_| OpenError::InvalidOptions)?;
        let path = physical::private_path(path)?;
        let worker = Worker::start(
            "prns-local-groups",
            options.queue_capacity,
            move |inbox: prns_storage::Inbox<Box<Job>>| {
                // Capture the whole binding, not just its copyable hash. Its
                // guard outlives all resources borrowing this identity.
                let identity = identity;
                let storage_options = physical::Options {
                    cache_bytes: options.cache_bytes,
                };
                let database = if matches!(mode, Mode::Create) {
                    physical::Owner::create_new(&path, storage_options, |tx| {
                        crate::enrollment::Enrollment::initialize_with_limits_in(
                            tx,
                            identity.hash,
                            options.catalog,
                        )
                        .map_err(Error::Enrollment)?;
                        #[cfg(feature = "archive")]
                        if let Some(limits) = options.archive {
                            crate::archive::initialize_in(tx, identity.hash, limits)?;
                        }
                        Ok::<_, Error>(())
                    })
                } else {
                    // The concrete manager below validates its own complete
                    // managed/fixed state before readiness is published.
                    physical::Owner::open_existing(&path, storage_options, |_| Ok::<_, Error>(()))
                }
                .map_err(physical_error)?;
                // Declared before the borrowing manager so errors/unwind also
                // release the retained database handle before physical cleanup.
                let mut validation = options
                    .validation_cache_entries
                    .map(|capacity| {
                        prns_groups_store_redb::ValidationCache::new(
                            database.database_handle(),
                            capacity,
                        )
                    })
                    .transpose()
                    .map_err(|error| OpenError::Domain(Error::Storage(error)))?;
                let mut manager = match mode {
                    Mode::Create | Mode::Existing => {
                        GroupManager::open_managed(database.database(), identity.hash)
                    }
                    Mode::Fixed(provisioned) => {
                        GroupManager::open_fixed(database.database(), *provisioned)
                    }
                }
                .map_err(OpenError::Domain)?;
                if let Some(cache) = validation.as_mut() {
                    manager = manager
                        .with_validation_cache(cache)
                        .map_err(OpenError::Domain)?;
                }
                inbox.run(|job| manager.execute(*job));
                // Drop in this order before retained cleanup becomes ready.
                // The same lexical order also holds during failure/unwind.
                drop(manager);
                drop(validation);
                drop(database);
                drop(identity);
                Ok(())
            },
        )
        .map_err(|error| match error {
            StartError::Spawn(error) => OpenError::Io(error),
            StartError::Initialization(error) => error,
            StartError::NotReady => OpenError::WorkerNotReady,
            StartError::Panicked => OpenError::WorkerPanicked,
        })?;
        let client = Client::new(Arc::new(Lane(worker.client(), budget)));
        Ok(Self { worker, client })
    }

    pub fn client(&self) -> Client {
        self.client.clone()
    }

    /// Fence immediately, drain accepted jobs, and report resource cleanup.
    /// Repeated calls observe the same terminal outcome. Dropping this future or
    /// owner never cancels accepted work. Job errors remain their own results;
    /// cleanup completion does not assert that every domain operation succeeded.
    /// This does not join the OS thread or stop a separately attached host.
    pub fn close(&self) -> Completion<OpenError> {
        self.worker.request_close();
        self.worker.closed()
    }
}

fn physical_error(error: physical::Error<Error>) -> OpenError {
    match error {
        physical::Error::InvalidOptions => OpenError::InvalidOptions,
        physical::Error::File(error) => error.into(),
        physical::Error::Database(error) => OpenError::Database(error),
        physical::Error::Domain(error) => OpenError::Domain(error),
    }
}
