use super::super::network;
use prns_host_runtime::owner::SessionError;
use prns_identities_core::ApiError;
use prns_storage::{files, CloseError};

mod attachment;
#[cfg(test)]
mod tests;

/// Diagnostic cause of selected-service attachment failure. These categories
/// do not authorize retry, replacement, or deletion of saved state.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "ServiceAttachmentFailure"))]
pub enum AttachmentFailure {
    Group { error: prns_groups::ApiError },
    Storage { reason: StoreFailure },
    Identity { error: ApiError },
    Capacity,
    Allocation,
    Configuration,
    Entropy,
    Network,
}

/// Foreign-safe classification of the physical store's existing failures.
/// Database implementation details remain diagnostic text, not a retry signal.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "ServiceStorageFailure"))]
pub enum StoreFailure {
    InvalidOptions,
    Allocation,
    Capacity,
    InvalidScope,
    MissingBinding,
    WrongBinding,
    Io { code: Option<i32> },
    UnsafePath,
    UnsafePermissions,
    UnsupportedPlatform,
    Database,
}

/// Stable construction/lifetime failures. Domain operations retain their own
/// canonical errors. I/O and failed startup may follow partial durable changes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Error))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "ServiceRuntimeError"))]
pub enum Error {
    InvalidOptions,
    NotReady,
    ObserverCapacity,
    Reentrant,
    Closing,
    Io {
        code: Option<i32>,
    },
    UnsafePath,
    UnsafePermissions,
    UnsupportedPlatform,
    Identity {
        error: ApiError,
    },
    Storage {
        reason: StoreFailure,
        detail: String,
    },
    /// Retained construction cause, including at most 1024 UTF-8 bytes of detail.
    /// The identity admission classifier remains independent of this diagnostic.
    Attachment {
        reason: AttachmentFailure,
        detail: String,
    },
    HostBusy,
    ConnectionsUnavailable,
    Connections {
        detail: String,
    },
    ConnectionDrainUnresolved {
        detail: String,
    },
    HostStart {
        detail: String,
    },
    /// The host thread joined, but reported failed shutdown.
    HostShutdown {
        detail: String,
    },
    /// No joined shutdown proof; resources remain retained and close is pending.
    HostOwnershipUnavailable,
    LifecyclePanicked,
    StartupCancelled,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native owner: {self:?}")
    }
}
impl std::error::Error for Error {}
impl From<files::Error> for Error {
    fn from(error: files::Error) -> Self {
        Self::from(&error)
    }
}
impl From<&files::Error> for Error {
    fn from(error: &files::Error) -> Self {
        match error {
            files::Error::Io(error) => Self::Io {
                code: error.raw_os_error(),
            },
            files::Error::UnsafePath => Self::UnsafePath,
            files::Error::UnsafePermissions => Self::UnsafePermissions,
            files::Error::UnsupportedPlatform => Self::UnsupportedPlatform,
        }
    }
}
impl From<SessionError> for Error {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::Busy => Self::HostBusy,
            SessionError::Start(error) => Self::HostStart {
                detail: format!("{error:?}"),
            },
            SessionError::Stop(error) => Self::HostShutdown {
                detail: format!("{error:?}"),
            },
            _ => Self::HostOwnershipUnavailable,
        }
    }
}
impl From<network::DriverError> for Error {
    fn from(error: network::DriverError) -> Self {
        Self::from(&error)
    }
}
impl From<&network::DriverError> for Error {
    fn from(error: &network::DriverError) -> Self {
        Self::Attachment {
            reason: attachment::classify(error),
            detail: attachment::detail(error),
        }
    }
}
impl From<crate::local::Error<network::DriverError>> for Error {
    fn from(error: crate::local::Error<network::DriverError>) -> Self {
        Self::from(&error)
    }
}
impl From<&crate::local::Error<network::DriverError>> for Error {
    fn from(error: &crate::local::Error<network::DriverError>) -> Self {
        use crate::local::Error as E;
        match error {
            E::InvalidOptions(_) => Self::InvalidOptions,
            E::Identity(prns_identities::owned::Error::Driver(error)) => Self::from(error),
            E::Identity(error) => Self::Identity {
                error: error.api_error(network::DriverError::service_failure),
            },
            E::File(error) => error.into(),
            E::Spawn(error) => Self::Io {
                code: error.raw_os_error(),
            },
            E::WorkerStopped => Self::LifecyclePanicked,
            E::Stores(error) => Self::Storage {
                reason: match store_failure(error) {
                    Ok(reason) => reason,
                    Err(never) => match *never {},
                },
                detail: error.to_string(),
            },
        }
    }
}

impl From<&CloseError<crate::local::Error<network::DriverError>>> for Error {
    fn from(error: &CloseError<crate::local::Error<network::DriverError>>) -> Self {
        match error {
            CloseError::Failed(error) => Self::from(error.as_ref()),
            CloseError::Panicked => Self::LifecyclePanicked,
            CloseError::Reentrant => Self::Reentrant,
        }
    }
}

fn file_failure(error: &files::Error) -> StoreFailure {
    match error {
        files::Error::Io(error) => StoreFailure::Io {
            code: error.raw_os_error(),
        },
        files::Error::UnsafePath => StoreFailure::UnsafePath,
        files::Error::UnsafePermissions => StoreFailure::UnsafePermissions,
        files::Error::UnsupportedPlatform => StoreFailure::UnsupportedPlatform,
    }
}
fn binding_failure(error: &prns_identities::scoped_store::Error) -> StoreFailure {
    use prns_identities::scoped_store::Error as E;
    match error {
        E::Io(error) => StoreFailure::Io {
            code: error.raw_os_error(),
        },
        E::Database(_) => StoreFailure::Database,
        E::InvalidScope => StoreFailure::InvalidScope,
        E::InvalidOptions => StoreFailure::InvalidOptions,
        E::MissingBinding => StoreFailure::MissingBinding,
        E::WrongBinding => StoreFailure::WrongBinding,
        E::UnsafePath => StoreFailure::UnsafePath,
        E::UnsupportedPlatform => StoreFailure::UnsupportedPlatform,
    }
}
// The physical owner keeps domain failures typed; its caller classifies them.
fn store_failure<E>(error: &crate::stores::Error<E>) -> Result<StoreFailure, &E> {
    use crate::stores::Error as S;
    use prns_storage::database::Error as Physical;
    Ok(match error {
        S::InvalidOptions => StoreFailure::InvalidOptions,
        S::Allocation => StoreFailure::Allocation,
        S::Capacity => StoreFailure::Capacity,
        S::File(error) => file_failure(error),
        S::Binding(error) => binding_failure(error),
        S::Physical(error) => match error {
            Physical::InvalidOptions => StoreFailure::InvalidOptions,
            Physical::File(error) => file_failure(error),
            Physical::Database(_) => StoreFailure::Database,
            Physical::Domain(error) => return Err(error),
        },
        S::Domain(error) => return Err(error),
    })
}
