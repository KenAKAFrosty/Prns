use prns_groups_mls::{LocalIdentity, Policy};
use prns_groups_reticulum::Peer;
#[cfg(feature = "text")]
use prns_groups_service::RecordId;
use prns_groups_service::{ControlReference, Endpoint, GroupId};
use prns_groups_store::StoreIncarnation;
use prns_groups_store_redb::{RedbError, RedbLimits, StoreNamespace};
use prns_host::IdentityHash;

/// Authenticated group provisioning. Private key material stays on the native
/// storage lane and never crosses the application bindings.
#[derive(Clone)]
pub struct Provisioned {
    /// None for fixed provisioning without a managed enrollment owner.
    pub owner_identity: Option<IdentityHash>,
    /// Exact current managed cleanup obligation; None for fixed provisioning.
    /// The captured control head binds this reserve to the admitted roster.
    pub cleanup_reserve: Option<prns_groups_store_redb::RedbReserve>,
    pub lifecycle: GroupLifecycle,
    pub history_generation: u32,
    /// Caller-supplied past-author mappings for fixed provisioning. Managed
    /// memberships resolve retained authors by exact namespace key on demand.
    pub history_peers: Vec<Peer>,
    pub group: GroupId,
    pub control: ControlReference,
    pub readers: Vec<Endpoint>,
    pub identity: LocalIdentity,
    pub policy: Policy,
    pub namespace: StoreNamespace,
    pub incarnation: StoreIncarnation,
    pub store_limits: RedbLimits,
    pub mls_limits: prns_groups_mls::Limits,
    pub service_limits: prns_groups_service::Limits,
    pub title: String,
    pub peers: Vec<Peer>,
    pub local_reticulum_identity: IdentityHash,
}

#[derive(Clone, Debug)]
pub struct NetworkConfig {
    pub control: ControlReference,
    pub group: GroupId,
    pub local: Endpoint,
    pub readers: Vec<Endpoint>,
    pub peers: Vec<Peer>,
    pub incarnation: StoreIncarnation,
    pub local_reticulum_identity: IdentityHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupConversationMember"))]
pub struct ConversationMember {
    pub endpoint: Endpoint,
    pub identity: IdentityHash,
    pub is_local: bool,
    pub is_owner: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupLifecycle"))]
pub enum GroupLifecycle {
    #[default]
    Active,
    Leaving,
    Removed,
    Closed,
}

/// Domain capabilities at the observed membership state. Every mutation still
/// revalidates its own admission and scope; these are not durable permissions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(
    feature = "uniffi-bindings",
    uniffi(name = "GroupConversationCapabilities")
)]
pub struct ConversationCapabilities {
    pub can_send: bool,
    pub can_invite: bool,
    pub can_remove_members: bool,
    pub can_leave: bool,
    pub can_close: bool,
    pub can_delete_local: bool,
}
impl ConversationCapabilities {
    pub(crate) fn for_state(
        owner: Option<IdentityHash>,
        local: IdentityHash,
        lifecycle: GroupLifecycle,
        ready: bool,
        member_count: u64,
    ) -> Self {
        let managed = owner.is_some();
        let is_owner = owner == Some(local);
        let active = lifecycle == GroupLifecycle::Active && ready;
        Self {
            can_send: active && member_count > 1,
            can_invite: active && is_owner,
            can_remove_members: active && is_owner && member_count > 1,
            can_leave: active && managed && !is_owner,
            can_close: active && is_owner,
            can_delete_local: managed && (!is_owner || lifecycle == GroupLifecycle::Closed),
        }
    }
}

/// Catalog facts without a loaded roster or message history. Exact group detail
/// is available independently of which summary page the caller currently holds.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupConversationSummary"))]
pub struct ConversationSummary {
    pub group: GroupId,
    pub title: String,
    pub owner_identity: Option<IdentityHash>,
    pub lifecycle: GroupLifecycle,
    pub ready: bool,
    pub member_count: u64,
    /// None while managed enrollment has no authenticated current membership.
    pub control: Option<ControlReference>,
    pub capabilities: ConversationCapabilities,
}

/// One live catalog traversal page. The next exclusive ID continues this view;
/// additions before that ID appear on refresh, not by restarting every page.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupConversationPage"))]
pub struct ConversationPage {
    pub items: Vec<ConversationSummary>,
    pub next: Option<GroupId>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupInvitationPage"))]
pub struct InvitationPage {
    pub items: Vec<crate::enrollment::Invitation>,
    /// Continue until None, including when filtering leaves an empty page.
    pub next: Option<crate::InvitationId>,
}

/// Exact selected-group detail. Fixed provisioning has no authenticated managed
/// profile; member bindings are queried separately with their own page budget.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupConversation"))]
pub struct Conversation {
    pub owner_identity: Option<IdentityHash>,
    pub lifecycle: GroupLifecycle,
    pub capabilities: ConversationCapabilities,
    pub group: GroupId,
    pub title: String,
    pub revision: u64,
    /// Membership provenance, separate from the record/change revision.
    pub control: ControlReference,
    pub member_count: u64,
    pub profile: Option<crate::GroupProfile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupMemberPage"))]
pub struct MemberPage {
    pub members: Vec<ConversationMember>,
    pub next: Option<Endpoint>,
    /// A changed control between pages requires a fresh roster traversal.
    pub control: ControlReference,
}
#[cfg(feature = "text")]
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupTextMessage"))]
pub struct Message {
    /// Delivery evidence stays truthful after an authorization barrier.
    pub membership_changed: bool,
    pub id: RecordId,
    pub client_request_id: Option<crate::RequestId>,
    /// The generic record's insertion position, including membership provenance.
    pub position: crate::ChangeToken,
    pub author: IdentityHash,
    pub text: String,
    pub outgoing: bool,
    pub received_count: u64,
    pub recipient_count: u64,
    pub sent_at_millis: u64,
}
#[cfg(feature = "text")]
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupTextPage"))]
pub struct MessagePage {
    pub messages: Vec<Message>,
    pub visited_heads: Vec<crate::ChangeToken>,
    pub current_incarnation: StoreIncarnation,
    /// Continue even when messages is empty if this cursor is present.
    pub next_before: Option<crate::RecordCursor>,
}
pub use prns_groups_core::{ApiError, RejectionReason};

#[derive(Debug)]
pub enum Error {
    /// Canonical public error propagated through a native composition boundary.
    Api(ApiError),
    NotProvisioned,
    NoRecipients,
    Enrollment(crate::enrollment::EnrollmentError),
    InvalidInput,
    IdempotencyConflict,
    Retry(prns_groups_core::retry::RetryError),
    StorageFull,
    Busy,
    Unavailable,
    Configuration,
    Corrupt,
    Clock,
    Storage(RedbError),
    Service(prns_groups_service::Error<RedbError>),
    Transport(prns_groups_reticulum::Error<RedbError>),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native group runner: {self:?}")
    }
}
impl std::error::Error for Error {}
impl Error {
    pub fn rejection_reason(&self) -> Option<RejectionReason> {
        if let Self::Api(error) = self {
            return error.rejection_reason();
        }
        use crate::enrollment::EnrollmentError;
        use prns_groups_mls::Error as MlsError;
        use prns_groups_service::Error as ServiceError;
        use prns_groups_store::{OperationRejection, StoreError};
        Some(match self {
            Self::Service(ServiceError::Store(StoreError::OperationRejected(
                OperationRejection::StorageCapacity,
            )))
            | Self::Service(ServiceError::Security(MlsError::Store(
                StoreError::OperationRejected(OperationRejection::StorageCapacity),
            ))) => RejectionReason::StorageCapacity,
            Self::InvalidInput => RejectionReason::InvalidInput,
            Self::NoRecipients => RejectionReason::NoRecipients,
            Self::IdempotencyConflict
            | Self::Service(prns_groups_service::Error::Fault(
                prns_groups_service::Fault::IdempotencyConflict,
            )) => RejectionReason::RequestConflict,
            Self::Enrollment(error) => match error {
                EnrollmentError::Invalid => RejectionReason::InvalidInput,
                EnrollmentError::Authentication => RejectionReason::AuthenticationFailed,
                EnrollmentError::Expired => RejectionReason::InvitationExpired,
                EnrollmentError::Missing => RejectionReason::NotFound,
                EnrollmentError::Conflict => RejectionReason::MembershipConflict,
                EnrollmentError::RequestConflict => RejectionReason::RequestConflict,
                EnrollmentError::NotOwner => RejectionReason::NotOwner,
                EnrollmentError::NotJoined => RejectionReason::NotJoined,
                EnrollmentError::AlreadyJoined => RejectionReason::AlreadyJoined,
                EnrollmentError::GroupClosed => RejectionReason::GroupClosed,
                EnrollmentError::OwnerMustClose => RejectionReason::OwnerMustClose,
                EnrollmentError::MemberNotFound => RejectionReason::MemberNotFound,
                EnrollmentError::StaleMembership => RejectionReason::StaleMembership,
                EnrollmentError::UnsupportedVersion => RejectionReason::UnsupportedVersion,
                EnrollmentError::UnsupportedProfile => RejectionReason::UnsupportedProfile,
                _ => return None,
            },
            _ => return None,
        })
    }

    pub fn is_retry_expired(&self) -> bool {
        if let Self::Api(error) = self {
            return error.is_retry_expired();
        }
        use prns_groups_core::retry::RetryError;
        matches!(
            self,
            Self::Retry(RetryError::Expired | RetryError::InvalidToken)
                | Self::Enrollment(crate::enrollment::EnrollmentError::Retry(
                    RetryError::Expired | RetryError::InvalidToken
                ))
        )
    }
    /// Bounded admission pressure, not permission to discard accepted work or
    /// change its request ID. Ambiguous database errors remain Unavailable.
    pub fn is_storage_full(&self) -> bool {
        if let Self::Api(error) = self {
            return error.is_storage_full();
        }
        use prns_groups_mls::Error as MlsError;
        use prns_groups_service::{Error as ServiceError, Fault};
        use prns_groups_store::{OperationRejection, StoreError};
        matches!(
            self,
            Self::StorageFull
                | Self::Storage(RedbError::Capacity)
                | Self::Enrollment(crate::enrollment::EnrollmentError::Capacity)
                | Self::Service(ServiceError::Fault(Fault::Capacity))
                | Self::Service(ServiceError::Store(
                    StoreError::Capacity(_)
                        | StoreError::OperationRejected(
                            OperationRejection::ReservedCapacity
                                | OperationRejection::StorageCapacity
                                | OperationRejection::RecoveryWindowFull
                        )
                ))
                | Self::Service(ServiceError::Security(MlsError::Capacity(_)))
                | Self::Service(ServiceError::Security(MlsError::Provider(
                    prns_groups_mls::ProviderError::Capacity(_)
                )))
                | Self::Service(ServiceError::Security(MlsError::Store(
                    StoreError::Capacity(_)
                        | StoreError::OperationRejected(
                            OperationRejection::RecoveryWindowFull
                                | OperationRejection::ReservedCapacity
                                | OperationRejection::StorageCapacity
                        )
                )))
        )
    }
}
impl From<RedbError> for Error {
    fn from(value: RedbError) -> Self {
        Self::Storage(value)
    }
}
impl From<prns_groups_service::Error<RedbError>> for Error {
    fn from(value: prns_groups_service::Error<RedbError>) -> Self {
        Self::Service(value)
    }
}
impl From<prns_groups_reticulum::Error<RedbError>> for Error {
    fn from(value: prns_groups_reticulum::Error<RedbError>) -> Self {
        Self::Transport(value)
    }
}

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        Self::from(&error)
    }
}
impl From<&Error> for ApiError {
    fn from(error: &Error) -> Self {
        if let Error::Api(error) = error {
            return *error;
        }
        if let Some(reason) = error.rejection_reason() {
            return if reason == RejectionReason::InvalidInput {
                Self::InvalidInput
            } else {
                Self::Rejected { reason }
            };
        }
        if error.is_storage_full() {
            return Self::StorageFull;
        }
        match error {
            Error::NotProvisioned
            | Error::Enrollment(crate::enrollment::EnrollmentError::MissingCatalog) => {
                Self::NotProvisioned
            }
            Error::Retry(reason)
            | Error::Enrollment(crate::enrollment::EnrollmentError::Retry(reason)) => {
                Self::Retry { reason: *reason }
            }
            Error::Busy => Self::Busy,
            Error::Configuration
            | Error::Enrollment(crate::enrollment::EnrollmentError::AlreadyInitialized) => {
                Self::Configuration
            }
            Error::Corrupt | Error::Enrollment(crate::enrollment::EnrollmentError::Corrupt) => {
                Self::Corrupt
            }
            Error::Clock | Error::Enrollment(crate::enrollment::EnrollmentError::Clock) => {
                Self::Clock
            }
            // Rich backend diagnostics remain available on the native manager
            // and Submit paths. Never misclassify uncertain outcomes as rejected.
            _ => Self::Unavailable,
        }
    }
}
impl From<ApiError> for Error {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

#[cfg(test)]
mod api_error_tests {
    use super::*;
    use crate::enrollment::EnrollmentError;
    use prns_groups_core::retry::RetryError;
    #[test]
    fn public_errors_preserve_definite_rejections_and_retry_identity() {
        for (error, reason) in [
            (Error::NoRecipients, RejectionReason::NoRecipients),
            (Error::IdempotencyConflict, RejectionReason::RequestConflict),
            (
                Error::Enrollment(EnrollmentError::RequestConflict),
                RejectionReason::RequestConflict,
            ),
            (
                Error::Enrollment(EnrollmentError::StaleMembership),
                RejectionReason::StaleMembership,
            ),
            (
                Error::Service(prns_groups_service::Fault::IdempotencyConflict.into()),
                RejectionReason::RequestConflict,
            ),
        ] {
            assert_eq!(ApiError::from(error), ApiError::Rejected { reason });
        }
        for reason in [RetryError::Expired, RetryError::InvalidToken] {
            assert_eq!(
                ApiError::from(Error::Retry(reason)),
                ApiError::Retry { reason }
            );
            assert_eq!(
                ApiError::from(Error::Enrollment(EnrollmentError::Retry(reason))),
                ApiError::Retry { reason }
            );
        }
        assert_eq!(
            ApiError::from(Error::Enrollment(EnrollmentError::MissingCatalog)),
            ApiError::NotProvisioned
        );
    }
    #[test]
    fn ambiguous_backend_errors_do_not_become_rejections() {
        for error in [
            Error::Unavailable,
            Error::Enrollment(EnrollmentError::Crypto),
            Error::Enrollment(EnrollmentError::Entropy),
        ] {
            let public = ApiError::from(error);
            assert_eq!(public, ApiError::Unavailable);
            assert_eq!(public.rejection_reason(), None);
        }
        let public = ApiError::Rejected {
            reason: RejectionReason::NoRecipients,
        };
        assert_eq!(ApiError::from(Error::from(public)), public);
        assert_eq!(
            Error::from(public).rejection_reason(),
            public.rejection_reason()
        );
    }
}
