use crate::identity::IdentityHash;
use crate::remote_control::{
    FixedRemoteControlTargetAccessTable, ForgetRemoteControlTargetOutcome,
    RemoteControlControllerIdentity, RemoteControlEndpoint, RemoteControlRequestSet,
    RemoteControlTargetAccess, RemoteControlTargetAccessCard, RemoteControlTargetAccessTable,
    SetRemoteControlTargetAccessError, SetRemoteControlTargetAccessOutcome,
    DEFAULT_MAX_REMOTE_CONTROL_TARGET_ACCESSES,
};
use heapless::Vec;

#[derive(Debug, PartialEq, Eq)]
pub struct AuthorizedRemoteControlTarget {
    identity_hash: IdentityHash,
}

impl AuthorizedRemoteControlTarget {
    #[must_use]
    pub const fn identity_hash(&self) -> IdentityHash {
        self.identity_hash
    }
}

impl From<&RemoteControlTargetAccess> for AuthorizedRemoteControlTarget {
    fn from(access: &RemoteControlTargetAccess) -> Self {
        Self {
            identity_hash: access.target().identity_hash(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetInventoryError {
    CapacityInvariantViolation,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlTargetInventory {
    targets: Vec<AuthorizedRemoteControlTarget, DEFAULT_MAX_REMOTE_CONTROL_TARGET_ACCESSES>,
}

impl RemoteControlTargetInventory {
    #[must_use]
    pub fn targets(&self) -> &[AuthorizedRemoteControlTarget] {
        self.targets.as_slice()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

impl TryFrom<&FixedRemoteControlTargetAccessTable<DEFAULT_MAX_REMOTE_CONTROL_TARGET_ACCESSES>>
    for RemoteControlTargetInventory
{
    type Error = RemoteControlTargetInventoryError;

    fn try_from(
        accesses: &FixedRemoteControlTargetAccessTable<DEFAULT_MAX_REMOTE_CONTROL_TARGET_ACCESSES>,
    ) -> Result<Self, Self::Error> {
        let mut targets = Vec::new();
        for access in accesses.accesses_in_identity_hash_order() {
            targets
                .push(access.into())
                .map_err(|_| RemoteControlTargetInventoryError::CapacityInvariantViolation)?;
        }
        Ok(Self { targets })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedRemoteControlTarget {
    target: IdentityHash,
    endpoint: RemoteControlEndpoint,
    controller: RemoteControlControllerIdentity,
    permitted_requests: RemoteControlRequestSet,
}

impl ResolvedRemoteControlTarget {
    #[must_use]
    pub const fn target(&self) -> IdentityHash {
        self.target
    }

    #[must_use]
    pub const fn endpoint(&self) -> RemoteControlEndpoint {
        self.endpoint
    }

    #[must_use]
    pub const fn controller(&self) -> &RemoteControlControllerIdentity {
        &self.controller
    }

    #[must_use]
    pub const fn permitted_requests(&self) -> &RemoteControlRequestSet {
        &self.permitted_requests
    }
}

impl From<(&RemoteControlControllerIdentity, &RemoteControlTargetAccess)>
    for ResolvedRemoteControlTarget
{
    fn from(
        (controller, access): (&RemoteControlControllerIdentity, &RemoteControlTargetAccess),
    ) -> Self {
        Self {
            target: access.target().identity_hash(),
            endpoint: access.endpoint(),
            controller: *controller,
            permitted_requests: access.effective_requests(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveRemoteControlTargetServiceError {
    Unavailable,
    TargetNotAuthorized,
    TransactionInProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetInventoryServiceError {
    Unavailable,
    Inventory(RemoteControlTargetInventoryError),
    TransactionInProgress,
}

impl From<RemoteControlTargetInventoryError> for RemoteControlTargetInventoryServiceError {
    fn from(error: RemoteControlTargetInventoryError) -> Self {
        match error {
            RemoteControlTargetInventoryError::CapacityInvariantViolation => Self::Inventory(error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveRemoteControlTargetControlError {
    NodeStopped,
    Busy,
    Unavailable,
    TargetNotAuthorized,
}

impl From<ResolveRemoteControlTargetServiceError> for ResolveRemoteControlTargetControlError {
    fn from(error: ResolveRemoteControlTargetServiceError) -> Self {
        match error {
            ResolveRemoteControlTargetServiceError::Unavailable => Self::Unavailable,
            ResolveRemoteControlTargetServiceError::TargetNotAuthorized => {
                Self::TargetNotAuthorized
            }
            ResolveRemoteControlTargetServiceError::TransactionInProgress => Self::Busy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetInventoryControlError {
    NodeStopped,
    Busy,
    Unavailable,
    Inventory(RemoteControlTargetInventoryError),
}

impl From<RemoteControlTargetInventoryServiceError> for RemoteControlTargetInventoryControlError {
    fn from(error: RemoteControlTargetInventoryServiceError) -> Self {
        match error {
            RemoteControlTargetInventoryServiceError::Unavailable => Self::Unavailable,
            RemoteControlTargetInventoryServiceError::Inventory(error) => Self::Inventory(error),
            RemoteControlTargetInventoryServiceError::TransactionInProgress => Self::Busy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRemoteControlTargetAccessServiceError {
    Unavailable,
    CapacityExhausted,
    TransactionInProgress,
}

impl From<SetRemoteControlTargetAccessError> for SetRemoteControlTargetAccessServiceError {
    fn from(error: SetRemoteControlTargetAccessError) -> Self {
        match error {
            SetRemoteControlTargetAccessError::CapacityExhausted => Self::CapacityExhausted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgetRemoteControlTargetServiceError {
    Unavailable,
    TransactionInProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRemoteControlTargetAccessControlError {
    NodeStopped,
    Busy,
    Unavailable,
    CapacityExhausted,
}

impl From<SetRemoteControlTargetAccessServiceError> for SetRemoteControlTargetAccessControlError {
    fn from(error: SetRemoteControlTargetAccessServiceError) -> Self {
        match error {
            SetRemoteControlTargetAccessServiceError::Unavailable => Self::Unavailable,
            SetRemoteControlTargetAccessServiceError::CapacityExhausted => Self::CapacityExhausted,
            SetRemoteControlTargetAccessServiceError::TransactionInProgress => Self::Busy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgetRemoteControlTargetControlError {
    NodeStopped,
    Busy,
    Unavailable,
}

impl From<ForgetRemoteControlTargetServiceError> for ForgetRemoteControlTargetControlError {
    fn from(error: ForgetRemoteControlTargetServiceError) -> Self {
        match error {
            ForgetRemoteControlTargetServiceError::Unavailable => Self::Unavailable,
            ForgetRemoteControlTargetServiceError::TransactionInProgress => Self::Busy,
        }
    }
}

pub trait RemoteControlTargetAccessControl {
    fn remote_control_target_inventory(
        &self,
    ) -> impl core::future::Future<
        Output = Result<RemoteControlTargetInventory, RemoteControlTargetInventoryControlError>,
    > + Send;

    fn resolve_remote_control_target(
        &self,
        target: IdentityHash,
    ) -> impl core::future::Future<
        Output = Result<ResolvedRemoteControlTarget, ResolveRemoteControlTargetControlError>,
    > + Send;

    fn set_remote_control_target_access(
        &self,
        access: RemoteControlTargetAccess,
    ) -> impl core::future::Future<
        Output = Result<
            SetRemoteControlTargetAccessOutcome,
            SetRemoteControlTargetAccessControlError,
        >,
    > + Send;

    fn forget_remote_control_target(
        &self,
        target: IdentityHash,
    ) -> impl core::future::Future<
        Output = Result<ForgetRemoteControlTargetOutcome, ForgetRemoteControlTargetControlError>,
    > + Send;
}

/// Failure to durably save controller-side target access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveRemoteControlTargetAccessError {
    Busy,
    /// The node stopped before acknowledgement; a submitted save may already be durable.
    NodeStopped,
    Unavailable,
    CapacityExhausted,
    PersistenceUnavailable,
    PersistenceFailed,
    RecipientMismatch,
    /// A card import would change access already saved for this target.
    Conflict,
}

impl core::fmt::Display for SaveRemoteControlTargetAccessError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Busy => "another target-access operation is pending",
            Self::NodeStopped => {
                "node stopped before save acknowledgement; the outcome may be unknown"
            }
            Self::Unavailable => "the RemoteControl service is unavailable",
            Self::CapacityExhausted => "the saved-target table is full",
            Self::PersistenceUnavailable => "durable authorization storage is unavailable",
            Self::PersistenceFailed => {
                "target access could not be saved; prior access was retained"
            }
            Self::RecipientMismatch => "the card is addressed to a different controller identity",
            Self::Conflict => "the card would replace existing target access",
        })
    }
}

impl core::error::Error for SaveRemoteControlTargetAccessError {}

impl From<SetRemoteControlTargetAccessServiceError> for SaveRemoteControlTargetAccessError {
    fn from(error: SetRemoteControlTargetAccessServiceError) -> Self {
        match error {
            SetRemoteControlTargetAccessServiceError::Unavailable => Self::Unavailable,
            SetRemoteControlTargetAccessServiceError::CapacityExhausted => Self::CapacityExhausted,
            SetRemoteControlTargetAccessServiceError::TransactionInProgress => Self::Busy,
        }
    }
}

/// Optional durable controller-side access capability. Implementations must
/// persist a successful change before activating it. A dropped waiter does not
/// cancel admitted persistence, and a stopped acknowledgement may be uncertain.
/// This capability never falls back to the volatile access-control setter.
pub trait RemoteControlTargetAccessPersistence {
    /// The running controller's public identity, even with an empty target table.
    fn remote_control_controller_identity(
        &self,
    ) -> impl core::future::Future<
        Output = Result<RemoteControlControllerIdentity, RemoteControlTargetInventoryControlError>,
    > + Send;

    /// Explicitly save or replace access after caller review.
    fn save_remote_control_target_access(
        &self,
        access: RemoteControlTargetAccess,
    ) -> impl core::future::Future<
        Output = Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError>,
    > + Send;

    /// Import recipient-bound access without replacing conflicting saved access.
    /// The caller must verify the source separately: the card is public data and
    /// neither a target grant receipt nor proof of authority.
    fn save_remote_control_target_access_card(
        &self,
        card: RemoteControlTargetAccessCard,
    ) -> impl core::future::Future<
        Output = Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError>,
    > + Send;
}
