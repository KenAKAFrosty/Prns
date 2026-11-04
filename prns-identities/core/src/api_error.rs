//! Stable public application failures. Rich storage/driver diagnostics remain in
//! their native owner. An I/O, unavailable, or stopped-worker error does not prove
//! that an operation had no effect: retain its exact request and recover it.
use crate::{Fault, ServiceFailure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(
    feature = "uniffi-bindings",
    uniffi(name = "IdentitySubmissionFailure")
)]
pub enum SubmissionFailure {
    Full,
    Closed,
    Reentrant,
    Bounds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityStorageFailure"))]
pub enum StorageFailure {
    Io { code: Option<i32> },
    Entropy,
    Busy,
    Bounds,
    Corrupt,
    MissingCatalog,
    MissingKey,
    KeyMismatch,
    ImportRequired,
    UnexpectedImport,
    WrongAction,
    UnsafePath,
    UnsafePermissions,
    UnsupportedPlatform,
    UnsupportedFormat,
    RevisionMismatch { expected: u64, found: u64 },
    ForeignCatalog,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityCreationFailure"))]
pub enum CreationFailure {
    Conflict,
    Expired,
    FutureScope,
    ForeignCatalog,
    Capacity,
    UnknownRequest,
    Pending,
    PruningBlocked,
}

/// Portable public classification, independent of a runtime implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityRuntimeFailure"))]
pub enum RuntimeFailure {
    InvalidConfiguration,
    UnsupportedService,
    Capacity,
    Exhausted,
    UnknownIdentity,
    UnknownService,
    WrongIdentity,
    Busy,
    WrongPhase,
    OwnerStillActive,
    StaleAuthority,
    InvalidEvidence,
    MutationsPending,
    DuplicateMutation,
    InvalidAdmission,
    Terminating,
    StorageNotReady,
    AuthorityUnavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Error))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityApiError"))]
pub enum ApiError {
    InvalidInput,
    InvalidOptions,
    Submission { reason: SubmissionFailure },
    Authority { reason: Fault },
    Storage { reason: StorageFailure },
    Creation { reason: CreationFailure },
    Runtime { reason: RuntimeFailure },
    Driver { reason: ServiceFailure },
    Closing,
    WorkerStopped,
}
impl core::fmt::Display for ApiError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "identity API: {self:?}")
    }
}
impl core::error::Error for ApiError {}
impl From<Fault> for ApiError {
    fn from(reason: Fault) -> Self {
        Self::Authority { reason }
    }
}
impl From<SubmissionFailure> for ApiError {
    fn from(reason: SubmissionFailure) -> Self {
        Self::Submission { reason }
    }
}
