//! Construction only. The non-generic native Client owns every catalog method.
use prns_identities::{
    api::{ApiError, Client, SubmissionFailure},
    owned::{Driver, Error, IdentityOnly, Options, Owner},
};
use prns_identities_runtime::Fault;
use prns_storage::{CloseError, Completion};
use std::sync::Arc;

// Deliberate standalone profile: four identities, 32 retained operations, four
// service declarations per identity and 16 runtime leases. No service is wired
// by this identity-only factory; compositions select their own owning driver.
type ServiceRuntime = Owner<4, 32, 4, 16, IdentityOnly>;
fn public_error(error: &Error<Fault>) -> ApiError {
    error.api_error(|error| <IdentityOnly as Driver<4>>::classify_failure(&IdentityOnly, error))
}

/// Owns one private catalog directory and its bounded worker. Construction is
/// blocking; call it off the UI thread. Opening never activates saved identities.
#[derive(uniffi::Object)]
pub struct IdentityCatalogOwner {
    owner: ServiceRuntime,
}
#[uniffi::export]
impl IdentityCatalogOwner {
    #[uniffi::constructor]
    pub fn create_new(path: String, options: Options) -> Result<Self, ApiError> {
        ServiceRuntime::create_new(path, options, |_| Ok(IdentityOnly))
            .map(|owner| Self { owner })
            .map_err(|error| public_error(&error))
    }
    #[uniffi::constructor]
    pub fn open_existing(path: String, options: Options) -> Result<Self, ApiError> {
        ServiceRuntime::open_existing(path, options, |_| Ok(IdentityOnly))
            .map(|owner| Self { owner })
            .map_err(|error| public_error(&error))
    }
    pub fn client(&self) -> Arc<Client> {
        Arc::new(self.owner.client())
    }
    /// Fence synchronously. Loss of this observer or its wait future cannot
    /// cancel accepted work or release resources before worker drainage.
    pub fn shutdown(&self) -> Arc<IdentityCatalogClose> {
        Arc::new(IdentityCatalogClose {
            completion: self.owner.close(),
        })
    }
}

#[derive(uniffi::Object)]
pub struct IdentityCatalogClose {
    completion: Completion<Error<Fault>>,
}
#[uniffi::export]
impl IdentityCatalogClose {
    pub async fn wait(&self) -> Result<(), ApiError> {
        self.completion.clone().await.map_err(|error| match error {
            CloseError::Failed(error) => public_error(&error),
            CloseError::Panicked => ApiError::WorkerStopped,
            CloseError::Reentrant => ApiError::Submission {
                reason: SubmissionFailure::Reentrant,
            },
        })
    }
}
