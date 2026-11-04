use super::*;

impl From<&crate::Error> for ApiError {
    fn from(error: &crate::Error) -> Self {
        use crate::Error as E;
        use StorageFailure as S;
        let reason = match error {
            E::Authority(reason) => return Self::Authority { reason: *reason },
            E::Codec(prns_identities_codec::Error::InvalidCatalog(reason)) => {
                return Self::Authority { reason: *reason };
            }
            E::Io(error) => S::Io {
                code: error.raw_os_error(),
            },
            E::Entropy(_) => S::Entropy,
            E::Busy => S::Busy,
            E::Bounds | E::Codec(prns_identities_codec::Error::BufferTooSmall) => S::Bounds,
            E::Corrupt
            | E::Codec(
                prns_identities_codec::Error::Truncated
                | prns_identities_codec::Error::InvalidEncoding
                | prns_identities_codec::Error::TrailingBytes,
            ) => S::Corrupt,
            E::MissingCatalog => S::MissingCatalog,
            E::MissingKey => S::MissingKey,
            E::KeyMismatch => S::KeyMismatch,
            E::ImportRequired => S::ImportRequired,
            E::UnexpectedImport => S::UnexpectedImport,
            E::WrongAction => S::WrongAction,
            E::UnsafePath => S::UnsafePath,
            E::UnsafePermissions => S::UnsafePermissions,
            E::UnsupportedPlatform => S::UnsupportedPlatform,
            E::UnsupportedCatalogFormat
            | E::Codec(
                prns_identities_codec::Error::UnsupportedVersion
                | prns_identities_codec::Error::CapacityMismatch,
            ) => S::UnsupportedFormat,
            E::RevisionMismatch { expected, found } => S::RevisionMismatch {
                expected: *expected,
                found: *found,
            },
            E::ForeignCatalog => S::ForeignCatalog,
        };
        Self::Storage { reason }
    }
}
impl From<&crate::creation::CreateError> for ApiError {
    fn from(error: &crate::creation::CreateError) -> Self {
        use crate::creation::CreateError as E;
        let reason = match error {
            E::Storage(error) => return Self::from(error),
            E::Authority(reason) => return Self::Authority { reason: *reason },
            E::Conflict => CreationFailure::Conflict,
            E::Expired => CreationFailure::Expired,
            E::FutureScope => CreationFailure::FutureScope,
            E::ForeignCatalog => CreationFailure::ForeignCatalog,
            E::Capacity => CreationFailure::Capacity,
            E::UnknownRequest => CreationFailure::UnknownRequest,
            E::Pending => CreationFailure::Pending,
            E::PruningBlocked => CreationFailure::PruningBlocked,
        };
        Self::Creation { reason }
    }
}
pub(crate) fn runtime_error(error: prns_identities_runtime::Fault) -> ApiError {
    use prns_identities_runtime::Fault as E;
    use RuntimeFailure as R;
    let reason = match error {
        E::Authority(reason) => return ApiError::Authority { reason },
        E::InvalidBudget | E::InvalidIncarnation | E::InvalidPlan => R::InvalidConfiguration,
        E::UnsupportedService => R::UnsupportedService,
        E::CapacityExceeded => R::Capacity,
        E::Exhausted => R::Exhausted,
        E::UnknownIdentity => R::UnknownIdentity,
        E::UnknownService => R::UnknownService,
        E::WrongIdentity => R::WrongIdentity,
        E::Busy => R::Busy,
        E::WrongPhase => R::WrongPhase,
        E::OwnerStillActive => R::OwnerStillActive,
        E::StaleOwner | E::StaleEffect | E::StaleEvidence | E::StaleLease => R::StaleAuthority,
        E::InvalidEvidence => R::InvalidEvidence,
        E::MutationsPending => R::MutationsPending,
        E::DuplicateMutation => R::DuplicateMutation,
        E::InvalidAdmission => R::InvalidAdmission,
        E::Terminating => R::Terminating,
        E::StorageNotReady => R::StorageNotReady,
        E::AuthorityUnavailable => R::AuthorityUnavailable,
    };
    ApiError::Runtime { reason }
}

pub(crate) fn owner_error<E>(
    error: &crate::owned::Error<E>,
    classify: impl FnOnce(&E) -> ServiceFailure,
) -> ApiError {
    use crate::owned::Error;
    match error {
        Error::InvalidOptions => ApiError::InvalidOptions,
        Error::Api(error) => *error,
        Error::Store(error) => ApiError::from(error),
        Error::Creation(error) => ApiError::from(error),
        Error::Authority(reason) => ApiError::Authority { reason: *reason },
        Error::Runtime(reason) => runtime_error(*reason),
        Error::Driver(error) => ApiError::Driver {
            reason: classify(error),
        },
        Error::Closing => ApiError::Closing,
        Error::WorkerStopped => ApiError::WorkerStopped,
        Error::Spawn(error) => ApiError::Storage {
            reason: StorageFailure::Io {
                code: error.raw_os_error(),
            },
        },
    }
}
