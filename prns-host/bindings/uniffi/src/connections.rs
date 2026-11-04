//! Foreign construction and lifetime adapters. Ordinary client methods and all
//! value inventories are generated from the owning native declarations.
use crate::connections_value::{
    ConnectionBindingStatus, ConnectionError, ConnectionOptions, ConnectionProfile,
};
use crate::HostSession;
use prns_host_runtime::connections as native;
use prns_storage::{CloseError, Completion};
use std::sync::Arc;

#[derive(uniffi::Object)]
pub struct ConnectionProfiles {
    owner: Arc<native::ProfileOwner>,
}
impl ConnectionProfiles {
    pub fn from_owner(owner: Arc<native::ProfileOwner>) -> Self {
        Self { owner }
    }
    /// Construction-only native capability; not part of the foreign API.
    pub fn owner(&self) -> Arc<native::ProfileOwner> {
        self.owner.clone()
    }
}
#[uniffi::export]
impl ConnectionProfiles {
    #[uniffi::constructor]
    pub fn create_new(
        path: String,
        profiles: Vec<ConnectionProfile>,
        options: ConnectionOptions,
    ) -> Result<Self, ConnectionError> {
        let profiles = profiles
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, ConnectionError>>()?;
        native::ProfileOwner::create_new(path, profiles, options.try_into()?)
            .map(|owner| Self::from_owner(Arc::new(owner)))
            .map_err(Into::into)
    }
    #[uniffi::constructor]
    pub fn open_existing(
        path: String,
        options: ConnectionOptions,
    ) -> Result<Self, ConnectionError> {
        native::ProfileOwner::open_existing(path, options.try_into()?)
            .map(|owner| Self::from_owner(Arc::new(owner)))
            .map_err(Into::into)
    }
    pub fn client(&self) -> Arc<ConnectionProfileClient> {
        ConnectionProfileClient::borrow(self.owner.client())
    }
    pub async fn bind_host(
        &self,
        host: Arc<HostSession>,
    ) -> Result<Arc<ConnectionHostBinding>, ConnectionError> {
        self.owner
            .bind(host.owner())
            .await
            .map(|binding| {
                Arc::new(ConnectionHostBinding {
                    binding: Arc::new(binding),
                })
            })
            .map_err(Into::into)
    }
    pub fn shutdown(&self) -> Arc<ConnectionProfilesClose> {
        Arc::new(ConnectionProfilesClose {
            completion: self.owner.close(),
        })
    }
}
#[derive(uniffi::Object)]
pub struct ConnectionProfileClient {
    pub(crate) client: native::Client,
}
impl ConnectionProfileClient {
    pub fn borrow(client: native::Client) -> Arc<Self> {
        Arc::new(Self { client })
    }
}
#[derive(uniffi::Object)]
pub struct ConnectionBindingClient {
    pub(crate) client: native::BindingClient,
}
impl ConnectionBindingClient {
    pub fn borrow(client: native::BindingClient) -> Arc<Self> {
        Arc::new(Self { client })
    }
}
/// The independent foreign owner. Composed factories return only its borrowed
/// client and retain this ownership natively for ordered protocol shutdown.
#[derive(uniffi::Object)]
pub struct ConnectionHostBinding {
    binding: Arc<native::HostBinding>,
}
#[uniffi::export]
impl ConnectionHostBinding {
    pub fn client(&self) -> Arc<ConnectionBindingClient> {
        ConnectionBindingClient::borrow(self.binding.client())
    }
    pub fn fence(&self) {
        self.binding.fence();
    }
    pub fn shutdown(&self) -> Arc<ConnectionBindingClose> {
        // The native owner retains the accepted cleanup even without a waiter.
        drop(self.binding.close());
        Arc::new(ConnectionBindingClose {
            binding: self.binding.clone(),
        })
    }
}
#[derive(uniffi::Object)]
pub struct ConnectionProfilesClose {
    completion: Completion<native::Error>,
}
#[uniffi::export]
impl ConnectionProfilesClose {
    pub fn finished(&self) -> bool {
        self.completion.status().is_some()
    }
    pub async fn wait(&self) -> Result<(), ConnectionError> {
        self.completion.clone().await.map_err(|error| match error {
            CloseError::Failed(error) => error.as_ref().clone().into(),
            CloseError::Panicked => native::Error::WorkerFailed.into(),
            CloseError::Reentrant => native::Error::Reentrant.into(),
        })
    }
}
/// Retained observation of the binding's cleanup state. Explicit later shutdown
/// may retry a failed retirement; status retains the latest historical failure.
#[derive(uniffi::Object)]
pub struct ConnectionBindingClose {
    binding: Arc<native::HostBinding>,
}
#[uniffi::export]
impl ConnectionBindingClose {
    pub fn status(&self) -> ConnectionBindingStatus {
        self.binding.status().into()
    }
    pub async fn wait(&self) -> Result<(), ConnectionError> {
        self.binding.closed().await.map_err(Into::into)
    }
}
#[uniffi::export]
pub fn new_connection_profile_id() -> Result<Vec<u8>, ConnectionError> {
    native::ProfileId::generate()
        .map(|id| id.bytes().to_vec())
        .map_err(Into::into)
}
