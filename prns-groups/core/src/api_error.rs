//! Stable, allocation-free outcomes at the public application API boundary.
//! Backend diagnostics remain in their owning implementation. Unavailable does
//! not establish rejection: preserve the request and recover its exact result.
use crate::retry::RetryError;

/// A definite rejected action. This never classifies an ambiguous storage or
/// transport failure, which must retain its original request for recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupRejectionReason"))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RejectionReason {
    /// Selected local storage refused a proven new, uncommitted operation.
    StorageCapacity,
    InvalidInput,
    NoRecipients,
    RequestConflict,
    InvitationExpired,
    NotFound,
    MembershipConflict,
    NotOwner,
    NotJoined,
    AuthenticationFailed,
    AlreadyJoined,
    GroupClosed,
    OwnerMustClose,
    MemberNotFound,
    StaleMembership,
    UnsupportedVersion,
    UnsupportedProfile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Error))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupApiError"))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ApiError {
    InvalidInput,
    Rejected { reason: RejectionReason },
    Retry { reason: RetryError },
    NotProvisioned,
    Busy,
    StorageFull,
    Unavailable,
    Configuration,
    Corrupt,
    Clock,
}
impl core::fmt::Display for ApiError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "group API: {self:?}")
    }
}
impl core::error::Error for ApiError {}
impl ApiError {
    pub const fn rejection_reason(&self) -> Option<RejectionReason> {
        match self {
            Self::InvalidInput => Some(RejectionReason::InvalidInput),
            Self::Rejected { reason } => Some(*reason),
            _ => None,
        }
    }
    pub const fn is_retry_expired(&self) -> bool {
        matches!(
            self,
            Self::Retry {
                reason: RetryError::Expired | RetryError::InvalidToken
            }
        )
    }
    pub const fn is_storage_full(&self) -> bool {
        matches!(
            self,
            Self::StorageFull
                | Self::Rejected {
                    reason: RejectionReason::StorageCapacity
                }
        )
    }
}
