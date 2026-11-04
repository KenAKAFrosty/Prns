//! Construction only: ordinary domain methods and records remain generated from
//! the same owning declarations used by Rust and other native compositions.
#![forbid(unsafe_code)]
use prns_service_runtime::groups::standalone::{
    Error, NetworkChange, NetworkStatus, Options, Owner, Status,
};
use prns_storage::{CloseError, Completion};
use std::sync::Arc;

// Public namespace roots allow a containing image to retain this exact metadata
// without linking a second native image or duplicating any domain declarations.
pub use prns_groups as group;
pub use prns_groups_core as group_core;
pub use prns_groups_service as group_service;
pub use prns_groups_store_redb as group_store_redb;
pub use prns_host_uniffi as host;
pub use prns_identities as identity;
pub use prns_identities_core as identity_core;
pub use prns_identities_runtime as identity_runtime;
pub use prns_identities_uniffi as identities;
pub use prns_service_runtime as runtime;
uniffi::setup_scaffolding!();

// HostConfig includes checked custom fields whose lift error is BindingError.
// UniFFI requires this exact outer error; wrapping it would turn malformed input
// into an internal failed-lift error before our constructor is entered. Only
// synchronous preflight/thread-start failures use this boundary. Retained startup
// and shutdown preserve the canonical ServiceRuntimeError in ready/status/shutdown.
fn construction_error(error: Error) -> host::transport::BindingError {
    use host::transport::BindingError;
    match error {
        Error::InvalidOptions => BindingError::InvalidInput {
            field: "serviceRuntimeOptions".into(),
            detail: "invalid explicit resource budget or policy".into(),
        },
        error => BindingError::Backend {
            detail: error.to_string(),
        },
    }
}

#[derive(uniffi::Object)]
pub struct ServiceRuntime {
    owner: Arc<Owner>,
}
impl ServiceRuntime {
    /// Retain an already-constructed native owner, including its supplied platform
    /// embedding. This consumes ownership; it neither starts nor copies a lane.
    pub fn start_networking_with_embedding(
        &self,
        host: runtime::groups::standalone::HostConfig,
        embedding: runtime::groups::standalone::HostEmbedding,
    ) -> Result<Arc<ServiceRuntimeNetworkChange>, Error> {
        self.owner
            .start_networking_with_embedding(host, embedding)
            .map(|change| {
                Arc::new(ServiceRuntimeNetworkChange {
                    change,
                    owner: self.owner.clone(),
                })
            })
    }
    pub fn from_owner(owner: Owner) -> Self {
        Self {
            owner: Arc::new(owner),
        }
    }
}
#[uniffi::export]
impl ServiceRuntime {
    #[uniffi::constructor]
    pub fn create_new(
        path: String,
        host: host::transport::HostConfig,
        options: Options,
    ) -> Result<Self, host::transport::BindingError> {
        let host = host.try_into()?;
        Owner::create_new(path, host, options)
            .map(Self::from_owner)
            .map_err(construction_error)
    }
    #[uniffi::constructor]
    pub fn open_existing(
        path: String,
        host: host::transport::HostConfig,
        options: Options,
    ) -> Result<Self, host::transport::BindingError> {
        let host = host.try_into()?;
        Owner::open_existing(path, host, options)
            .map(Self::from_owner)
            .map_err(construction_error)
    }
    /// Construct only local data. No network/permission/host preparation occurs.
    #[uniffi::constructor]
    pub fn create_new_offline(path: String, options: Options) -> Result<Self, Error> {
        Owner::create_new_offline(path, options).map(Self::from_owner)
    }
    #[uniffi::constructor]
    pub fn open_existing_offline(path: String, options: Options) -> Result<Self, Error> {
        Owner::open_existing_offline(path, options).map(Self::from_owner)
    }
    pub async fn ready(&self) -> Result<(), Error> {
        self.owner.ready().await
    }
    pub fn status(&self) -> Status {
        self.owner.status()
    }
    pub fn network_status(&self) -> NetworkStatus {
        self.owner.network_status()
    }
    pub async fn network_ready(&self) -> Result<(), Error> {
        self.owner.network_ready().await
    }
    pub fn start_networking(
        &self,
        host: host::transport::HostConfig,
    ) -> Result<Arc<ServiceRuntimeNetworkChange>, host::transport::BindingError> {
        self.owner
            .start_networking(host.try_into()?)
            .map(|change| {
                Arc::new(ServiceRuntimeNetworkChange {
                    change,
                    owner: self.owner.clone(),
                })
            })
            .map_err(construction_error)
    }
    pub fn stop_networking(&self) -> Result<Arc<ServiceRuntimeNetworkChange>, Error> {
        self.owner.stop_networking().map(|change| {
            Arc::new(ServiceRuntimeNetworkChange {
                change,
                owner: self.owner.clone(),
            })
        })
    }
    pub fn announcements(
        &self,
        reference: identity_core::IdentityReference,
    ) -> Result<Arc<identity::announces::Client>, identity_core::ApiError> {
        self.owner
            .announcements(reference)
            .map(Arc::new)
            .map_err(|error| match error {
                Error::Closing => identity_core::ApiError::Closing,
                Error::Identity { error } => error,
                _ => identity_core::ApiError::WorkerStopped,
            })
    }

    pub fn identities(&self) -> Result<Arc<identity::owned::Client>, Error> {
        self.owner.identities().map(Arc::new)
    }
    // This exact outer error preserves checked IdentityReference lifting.
    pub fn groups(
        &self,
        reference: identity_core::IdentityReference,
    ) -> Result<Arc<group::Client>, identity_core::ApiError> {
        self.owner.groups(reference).map(Arc::new)
    }
    pub fn host(&self) -> Result<Arc<host::HostClientHandle>, Error> {
        self.owner.host().map(host::HostClientHandle::borrow)
    }
    pub fn shutdown(&self) -> Arc<ServiceRuntimeClose> {
        Arc::new(ServiceRuntimeClose {
            completion: self.owner.close(),
            owner: self.owner.clone(),
        })
    }
}
#[cfg(feature = "connections")]
#[uniffi::export]
impl ServiceRuntime {
    #[uniffi::constructor]
    pub fn create_new_with_connections(
        path: String,
        host: host::transport::HostConfig,
        options: Options,
        profiles: Arc<host::ConnectionProfiles>,
    ) -> Result<Self, host::transport::BindingError> {
        Owner::create_new_with_connections(
            path,
            host.try_into()?,
            options,
            Default::default(),
            profiles.owner(),
        )
        .map(Self::from_owner)
        .map_err(construction_error)
    }
    #[uniffi::constructor]
    pub fn open_existing_with_connections(
        path: String,
        host: host::transport::HostConfig,
        options: Options,
        profiles: Arc<host::ConnectionProfiles>,
    ) -> Result<Self, host::transport::BindingError> {
        Owner::open_existing_with_connections(
            path,
            host.try_into()?,
            options,
            Default::default(),
            profiles.owner(),
        )
        .map(Self::from_owner)
        .map_err(construction_error)
    }
    pub fn connection_binding(&self) -> Result<Arc<host::ConnectionBindingClient>, Error> {
        self.owner
            .connection_binding()
            .map(host::ConnectionBindingClient::borrow)
    }
}
/// Observes the already-fenced owner. Retaining this handle preserves diagnostic
/// state; its loss never cancels cleanup or releases the executor prematurely.
#[derive(uniffi::Object)]
pub struct ServiceRuntimeClose {
    completion: Completion<Error>,
    owner: Arc<Owner>,
}
#[uniffi::export]
impl ServiceRuntimeClose {
    pub fn status(&self) -> Status {
        self.owner.status()
    }
    pub async fn wait(&self) -> Result<(), Error> {
        self.completion.clone().await.map_err(|error| match error {
            CloseError::Failed(error) => error.as_ref().clone(),
            CloseError::Panicked => Error::LifecyclePanicked,
            CloseError::Reentrant => Error::Reentrant,
        })
    }
}

/// The retained network transition, independent of the local data owner's life.
#[derive(uniffi::Object)]
pub struct ServiceRuntimeNetworkChange {
    change: NetworkChange,
    owner: Arc<Owner>,
}
#[uniffi::export]
impl ServiceRuntimeNetworkChange {
    pub fn status(&self) -> NetworkStatus {
        self.owner.network_status()
    }
    pub async fn wait(&self) -> Result<(), Error> {
        self.change.wait().await
    }
}
