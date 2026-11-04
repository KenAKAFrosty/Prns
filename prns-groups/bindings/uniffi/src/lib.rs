//! Standalone construction and retained cleanup for the ordinary group Client.
//! Domain methods/models are generated from their core/service/native owners.
#![forbid(unsafe_code)]

use prns_groups::{
    owned::{LocalIdentity, LocalOptions, LocalOwner, OpenError},
    Client,
};
use prns_groups_core::ApiError;
use prns_identities_uniffi::KeyLease;
use prns_storage::{CloseError, Completion};
use std::{num::NonZeroUsize, sync::Arc};

uniffi::setup_scaffolding!();

/// Startup and cleanup evidence; I/O and database failures can follow durable
/// changes. Preserve the path and reopen explicitly with the same key capability.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum GroupOpenError {
    #[error("invalid explicit worker/cache budget")]
    InvalidOptions,
    #[error("group identity does not match the retained key")]
    IdentityMismatch,
    #[error("group path is unsafe")]
    UnsafePath,
    #[error("group path permissions are unsafe")]
    UnsafePermissions,
    #[error("platform does not support the storage durability contract")]
    UnsupportedPlatform,
    #[error("group storage I/O failed: {detail}")]
    Io { code: Option<i32>, detail: String },
    #[error("group database failed: {detail}")]
    Database { detail: String },
    #[error("group domain initialization failed: {error}")]
    Domain { error: ApiError },
    #[error("group worker did not become ready")]
    WorkerNotReady,
    #[error("group worker panicked; recover durable outcomes before retrying")]
    WorkerPanicked,
    #[error("a worker cannot wait for its own cleanup")]
    Reentrant,
}
impl From<&OpenError> for GroupOpenError {
    fn from(error: &OpenError) -> Self {
        match error {
            OpenError::InvalidOptions => Self::InvalidOptions,
            OpenError::IdentityMismatch => Self::IdentityMismatch,
            OpenError::UnsafePath => Self::UnsafePath,
            OpenError::UnsafePermissions => Self::UnsafePermissions,
            OpenError::UnsupportedPlatform => Self::UnsupportedPlatform,
            OpenError::Io(error) => Self::Io {
                code: error.raw_os_error(),
                detail: error.to_string(),
            },
            OpenError::Database(error) => Self::Database {
                detail: error.to_string(),
            },
            OpenError::Domain(error) => Self::Domain {
                error: ApiError::from(error),
            },
            OpenError::WorkerNotReady => Self::WorkerNotReady,
            OpenError::WorkerPanicked => Self::WorkerPanicked,
        }
    }
}
fn options(queue_capacity: u32, cache_bytes: u64) -> Result<LocalOptions, GroupOpenError> {
    let queue_capacity = usize::try_from(queue_capacity)
        .ok()
        .and_then(NonZeroUsize::new)
        .ok_or(GroupOpenError::InvalidOptions)?;
    let cache_bytes = usize::try_from(cache_bytes).map_err(|_| GroupOpenError::InvalidOptions)?;
    // Resource policy is checked once by LocalOwner before filesystem access.
    Ok(LocalOptions::new(queue_capacity, cache_bytes))
}
fn retained_identity(key: &KeyLease) -> LocalIdentity {
    let lease = key.key_lease();
    LocalIdentity::retain(
        prns_groups::IdentityHash::new(*lease.identity_hash().as_bytes()),
        lease,
    )
}

/// Owns one local database. Borrowed clients do not keep admission open after
/// this owner closes or is finalized. The worker retains the key through cleanup.
#[derive(uniffi::Object)]
pub struct GroupLocalOwner {
    owner: LocalOwner,
}
#[uniffi::export]
impl GroupLocalOwner {
    /// Blocking native construction; call off the UI thread. Parent directory
    /// must already be private. Existing files are never replaced.
    #[uniffi::constructor]
    pub fn create_new(
        path: String,
        key: Arc<KeyLease>,
        queue_capacity: u32,
        cache_bytes: u64,
    ) -> Result<Self, GroupOpenError> {
        let options = options(queue_capacity, cache_bytes)?;
        LocalOwner::create_new(path, retained_identity(&key), options)
            .map(|owner| Self { owner })
            .map_err(|error| GroupOpenError::from(&error))
    }
    /// Open only existing domain state bound to this retained key. Opening never
    /// falls back to creation or silently replaces missing/corrupt state.
    #[uniffi::constructor]
    pub fn open_existing(
        path: String,
        key: Arc<KeyLease>,
        queue_capacity: u32,
        cache_bytes: u64,
    ) -> Result<Self, GroupOpenError> {
        let options = options(queue_capacity, cache_bytes)?;
        LocalOwner::open_existing(path, retained_identity(&key), options)
            .map(|owner| Self { owner })
            .map_err(|error| GroupOpenError::from(&error))
    }
    pub fn client(&self) -> Arc<Client> {
        Arc::new(self.owner.client())
    }
    /// Fence synchronously and retain the cleanup outcome. Dropping this
    /// observer, its wait future, or this owner never cancels accepted work.
    pub fn shutdown(&self) -> Arc<GroupClose> {
        Arc::new(GroupClose {
            completion: self.owner.close(),
        })
    }
}

/// Retained observation of the existing storage worker's resource cleanup.
#[derive(uniffi::Object)]
pub struct GroupClose {
    completion: Completion<OpenError>,
}
#[uniffi::export]
impl GroupClose {
    pub async fn wait(&self) -> Result<(), GroupOpenError> {
        self.completion.clone().await.map_err(|error| match error {
            CloseError::Failed(error) => GroupOpenError::from(error.as_ref()),
            CloseError::Panicked => GroupOpenError::WorkerPanicked,
            CloseError::Reentrant => GroupOpenError::Reentrant,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn foreign_numeric_conversion_is_checked_without_platform_access() {
        assert!(matches!(
            options(0, 1 << 20),
            Err(GroupOpenError::InvalidOptions)
        ));
        assert!(options(1, 1 << 20).is_ok());
        assert!(options(1024, 1 << 30).is_ok());
        if usize::BITS < 64 {
            assert!(options(1, u64::MAX).is_err());
        }
    }
    #[test]
    fn io_categories_do_not_claim_a_definite_operation_outcome() {
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::AlreadyExists,
            std::io::ErrorKind::Other,
        ] {
            assert!(matches!(
                GroupOpenError::from(&OpenError::Io(std::io::Error::new(kind, "uncertain"))),
                GroupOpenError::Io { .. }
            ));
        }
    }
}
