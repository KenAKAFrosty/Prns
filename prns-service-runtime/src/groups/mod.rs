//! Catalog-managed local groups on the existing identity lane.
//!
//! This adapter owns durable local schemas and authoritative admission. It has
//! no network attachment: enabling configured groups fails explicitly and never
//! reports an active service. Local history and retained-result recovery remain
//! usable while paused. The optional `network` module supplies a concrete driver
//! with staged host endpoints and a retained runner on one supplied executor.
mod admission;
mod client;
mod dependencies;
mod driver;
mod factory;
#[cfg(feature = "groups-network")]
pub mod network;
#[cfg(feature = "groups-network")]
mod observations;
mod recovery;
#[cfg(feature = "standalone")]
pub mod standalone;
pub use client::{client, network_client, Command, Submission, SubmissionState};
pub use factory::{create_new, open_existing, Options, Owner};
mod schema;
use crate::stores::{self, Stores};
use prns_groups as groups;
use prns_identities::owned;
use prns_identities_core as core;
use prns_identities_runtime as runtime;
pub use schema::SchemaError;
use std::{
    sync::Arc,
    task::{Context, Waker},
};

/// Stable catalog service identifier, independent of product labels/profiles.
pub const SERVICE: core::ServiceId = core::ServiceId(0x6772_7073);
#[derive(Debug)]
pub enum Error {
    Store(stores::Error<SchemaError>),
    Schema(SchemaError),
    Runtime(runtime::Fault),
    UnsupportedService(core::ServiceId),
    NetworkUnavailable,
    Observing,
    Allocation,
    Observation(Arc<stores::Error<SchemaError>>),
    Capacity,
}
impl Error {
    pub(crate) fn service_failure(&self) -> core::ServiceFailure {
        match self {
            Self::NetworkUnavailable => core::ServiceFailure::Unavailable(1),
            _ => core::ServiceFailure::StorageUnavailable,
        }
    }
}
impl From<SchemaError> for Error {
    fn from(error: SchemaError) -> Self {
        Self::Schema(error)
    }
}
impl From<runtime::Fault> for Error {
    fn from(error: runtime::Fault) -> Self {
        Self::Runtime(error)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native groups attachment: {self:?}")
    }
}
impl std::error::Error for Error {}

struct Entry {
    identity: core::IdentityReference,
    quiesced: Option<runtime::OwnerToken>,
    attachment: groups::Attachment,
    // Resource ownership only: the identity runtime retains the sole mutation
    // registry, and Attachment owns the exact physical recovery evidence.
    carrier: Option<admission::PendingCarrier>,
    dirty: bool,
    failure: Option<Arc<stores::Error<SchemaError>>>,
    // The scan and exact complete result remain here between bounded owner polls.
    scan: Option<groups::DependencyScan>,
    dependencies: Option<groups::Dependencies>,
}
/// Selected concrete adapter. Store ownership is independent from live private
/// key use, so keeping historical groups open cannot itself prevent Pause.
pub struct Local<const I: usize> {
    // Attachments/subscriptions close before their physical databases.
    entries: Vec<Entry>,
    stores: Stores<I>,
    limits: groups::retained::Limits,
    catalog: groups::enrollment::CatalogLimits,
    state: SubmissionState,
    validation_cache_entries: std::num::NonZeroUsize,
    profile_support: Option<groups::ProfileSupport>,
    #[cfg(feature = "archive")]
    archive: Option<groups::archive::Limits>,
    wake: Waker,
    next_scan: usize,
    next_recovery: usize,
}
impl<const I: usize> Local<I> {
    pub fn new(
        stores: Stores<I>,
        limits: groups::retained::Limits,
        wake: Waker,
        state: SubmissionState,
    ) -> Result<Self, Error> {
        limits.validate().map_err(SchemaError::Group)?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(I)
            .map_err(|_| Error::Allocation)?;
        Ok(Self {
            stores,
            limits,
            catalog: groups::enrollment::CatalogLimits::default(),
            state,
            validation_cache_entries: std::num::NonZeroUsize::MIN,
            profile_support: None,
            #[cfg(feature = "archive")]
            archive: None,
            entries,
            wake,
            next_scan: 0,
            next_recovery: 0,
        })
    }
    /// Share the same submission state with ordinary and activated carrier
    /// clients in a custom embedding; never allocate one per identity/client.
    pub fn submission<J: Send + 'static>(&self, commands: owned::Commands<J>) -> Submission<J> {
        Submission::new(commands, self.state.clone())
    }
    pub fn with_validation_cache_entries(mut self, entries: std::num::NonZeroUsize) -> Self {
        self.validation_cache_entries = entries;
        self
    }
    /// Apply the owning transport's capabilities to every persisted and future
    /// attachment before this adapter is published as a network service.
    #[cfg(feature = "groups-network")]
    fn with_profile_support(mut self, support: groups::ProfileSupport) -> Result<Self, Error> {
        for entry in &mut self.entries {
            let database = self
                .stores
                .database_handle(entry.identity)
                .map_err(Error::Store)?;
            entry
                .attachment
                .set_profile_support(&database, support)
                .map_err(SchemaError::Group)?;
        }
        self.profile_support = Some(support);
        Ok(self)
    }
    /// Composition may use the same physical attachment for another selected
    /// private service. It still owns that service's schema and admission rules.
    pub fn stores(&mut self) -> &mut Stores<I> {
        &mut self.stores
    }
    /// Configure explicit fresh initialization; a persisted catalog owns its
    /// selection on reopen and interrupted AddService reconciliation.
    pub fn with_catalog_limits(
        mut self,
        limits: groups::enrollment::CatalogLimits,
    ) -> Result<Self, Error> {
        limits
            .validate()
            .map_err(|error| SchemaError::Group(groups::Error::Enrollment(error)))?;
        self.catalog = limits;
        Ok(self)
    }
    /// Configure fresh service initialization. Existing attachments retain the
    /// archive policy already committed in their shared physical database.
    #[cfg(feature = "archive")]
    pub fn with_archive(mut self, limits: groups::archive::Limits) -> Result<Self, Error> {
        limits
            .validate()
            .map_err(|error| SchemaError::Group(groups::Error::Storage(error)))?;
        self.archive = Some(limits);
        Ok(self)
    }
    /// Strict startup validation of an actually configured group service.
    /// A product selecting several services routes each ID to its real adapter.
    pub fn attach(&mut self, identity: core::IdentityReference) -> Result<(), Error> {
        if self.entries.iter().any(|entry| entry.identity == identity) {
            return Ok(());
        }
        if self.entries.len() == I {
            return Err(Error::Capacity);
        }
        let database = self
            .stores
            .database_handle(identity)
            .map_err(Error::Store)?;
        schema::validate(&database, hash(identity))?;
        let mut attachment = groups::Attachment::open_cached(
            database.clone(),
            hash(identity),
            self.validation_cache_entries,
        )
        .map_err(SchemaError::Group)?;
        if let Some(support) = self.profile_support {
            attachment
                .set_profile_support(&database, support)
                .map_err(SchemaError::Group)?;
        }
        self.entries.push(Entry {
            identity,
            attachment,
            carrier: None,
            quiesced: None,
            dirty: true,
            failure: None,
            scan: None,
            dependencies: None,
        });
        self.wake.wake_by_ref();
        Ok(())
    }
    fn invalidate(&mut self, identity: core::IdentityReference) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.identity == identity)
        {
            entry.dirty = true;
            entry.failure = None;
            self.wake.wake_by_ref();
        }
    }
    pub fn initialize(
        &mut self,
        ticket: core::EffectTicket,
        identity: core::IdentityGuard,
    ) -> Result<core::InitializedService, Error> {
        let reference = reference(identity);
        let local = hash(reference);
        let limits = self.limits;
        let catalog = self.catalog;
        #[cfg(feature = "archive")]
        let archive = self.archive;
        self.stores
            .prepare(
                reference,
                |write| {
                    schema::initialize(
                        write,
                        local,
                        limits,
                        catalog,
                        #[cfg(feature = "archive")]
                        archive,
                    )
                },
                |database| {
                    schema::reconcile(
                        database,
                        local,
                        limits,
                        catalog,
                        #[cfg(feature = "archive")]
                        archive,
                    )
                },
            )
            .map_err(Error::Store)?;
        self.attach(reference)?;
        Ok(core::InitializedService {
            ticket,
            service: SERVICE,
        })
    }
    pub fn command(&mut self, command: Command, authority: &mut dyn owned::Admission) {
        // Every queued runner callback belongs to its captured activation, even
        // when its domain operation is a read or recovery helper. Exact pending
        // physical recovery runs through poll_recovery without this fresh gate.
        if let Some(network) = command.network {
            if network.service() != SERVICE
                || reference(network.admission().identity) != command.identity
            {
                command.job.fail(groups::Error::Configuration);
                return;
            }
            if let Err(error) = authority.validate_network(network) {
                command.job.fail(admission::map_error(error));
                return;
            }
        }
        // Check the immutable reference before any lazy history attachment. This
        // grants no dispatch permission; the group handler derives that itself.
        let binding = match authority.acquire(command.identity, owned::Access::History) {
            Ok(permit) => permit,
            Err(error) => {
                command.job.fail(admission::map_error(error));
                return;
            }
        };
        if authority
            .settle(binding, runtime::Settlement::Rejected)
            .is_err()
        {
            command.job.fail(groups::Error::Unavailable);
            return;
        }
        if self.attach(command.identity).is_err() {
            command.job.fail(groups::Error::Unavailable);
            return;
        }
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.identity == command.identity)
        else {
            command.job.fail(groups::Error::Unavailable);
            return;
        };
        let mut job = Some(command.job);
        let result: Result<(), stores::Error<SchemaError>> =
            self.stores.with_existing(command.identity, |database| {
                let mut manager =
                    groups::GroupManager::open_attached(database, &mut entry.attachment)
                        .map_err(SchemaError::Group)?;
                if let Some(job) = job.take() {
                    manager.execute_authorized(
                        job,
                        &mut admission::Gate {
                            identity: command.identity,
                            admission: authority,
                            network: command.network,
                            pending: &mut entry.carrier,
                        },
                    );
                }
                Ok(())
            });
        if result.is_err() {
            if let Some(job) = job {
                job.fail(groups::Error::Unavailable);
            }
        }
        // A failed commit can still have changed durable facts. Re-observation
        // uses the domain's exact stamp and never turns the error into success.
        self.invalidate(command.identity);
    }
    pub fn reject(command: Command) {
        command.reject();
    }
}
fn hash(reference: core::IdentityReference) -> groups::IdentityHash {
    groups::IdentityHash::new(reference.public_hash.0)
}
fn reference(guard: core::IdentityGuard) -> core::IdentityReference {
    core::IdentityReference {
        id: guard.id,
        public_hash: guard.public_hash,
    }
}
