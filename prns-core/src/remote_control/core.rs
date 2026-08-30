use crate::identity::in_memory::{IdentityParts, InMemoryNodeIdentity};
use crate::identity::vault::IdentitySecretKey;
use crate::identity::{IdentityHash, IdentityPublicKeys, IDENTITY_SECRET_KEY_LEN};

use super::pairing::RemoteControlPairingPermissions;
use super::{RemoteControlRequestKind, RemoteControlRequestSet};

pub const REMOTE_CONTROL_NODE_IDENTITY_COUNT: usize = 2;
pub struct RemoteControlStorageRequirements {
    held_identities: usize,
    upstream_app_destinations: usize,
    request_handlers: usize,
}

impl RemoteControlStorageRequirements {
    pub const AVAILABLE: Self = Self {
        held_identities: 3,
        upstream_app_destinations: 3,
        request_handlers: 1,
    };

    #[must_use]
    pub const fn held_identities(&self) -> usize {
        self.held_identities
    }

    #[must_use]
    pub const fn upstream_app_destinations(&self) -> usize {
        self.upstream_app_destinations
    }

    #[must_use]
    pub const fn request_handlers(&self) -> usize {
        self.request_handlers
    }
}

pub const REMOTE_CONTROL_REQUIRED_HELD_IDENTITY_CAPACITY: usize =
    RemoteControlStorageRequirements::AVAILABLE.held_identities();
pub const REMOTE_CONTROL_REQUIRED_UPSTREAM_APP_DESTINATION_CAPACITY: usize =
    RemoteControlStorageRequirements::AVAILABLE.upstream_app_destinations();

pub struct RemoteControlControllerIdentitySecret {
    parts: IdentityParts,
}

impl From<IdentitySecretKey> for RemoteControlControllerIdentitySecret {
    fn from(secret: IdentitySecretKey) -> Self {
        Self {
            parts: InMemoryNodeIdentity::from_secret_key_bytes(&secret).into_parts(),
        }
    }
}

pub struct RemoteControlTargetIdentitySecret {
    parts: IdentityParts,
}

impl From<IdentitySecretKey> for RemoteControlTargetIdentitySecret {
    fn from(secret: IdentitySecretKey) -> Self {
        Self {
            parts: InMemoryNodeIdentity::from_secret_key_bytes(&secret).into_parts(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlNodeIdentitySecretsError {
    ControllerAndTargetAreSameIdentity,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlNodeIdentityGenerationError<EntropyError> {
    ControllerEntropy(EntropyError),
    TargetEntropy(EntropyError),
    InvalidPair(RemoteControlNodeIdentitySecretsError),
}

impl<EntropyError> core::fmt::Display for RemoteControlNodeIdentityGenerationError<EntropyError>
where
    EntropyError: core::fmt::Display,
{
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ControllerEntropy(error) => {
                write!(
                    formatter,
                    "controller identity could not be generated: {error}"
                )
            }
            Self::TargetEntropy(error) => {
                write!(formatter, "target identity could not be generated: {error}")
            }
            Self::InvalidPair(_) => {
                formatter.write_str("controller and target resolve to the same identity")
            }
        }
    }
}

impl<EntropyError> core::error::Error for RemoteControlNodeIdentityGenerationError<EntropyError>
where
    EntropyError: core::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::ControllerEntropy(error) | Self::TargetEntropy(error) => Some(error),
            Self::InvalidPair(_) => None,
        }
    }
}

pub struct RemoteControlNodeIdentitySecrets {
    controller: RemoteControlControllerIdentitySecret,
    target: RemoteControlTargetIdentitySecret,
}

impl RemoteControlNodeIdentitySecrets {
    pub fn generate<EntropyError>(
        mut fill_entropy: impl FnMut(&mut [u8]) -> Result<(), EntropyError>,
    ) -> Result<Self, RemoteControlNodeIdentityGenerationError<EntropyError>> {
        let mut controller = IdentitySecretKey::new([0; IDENTITY_SECRET_KEY_LEN]);
        fill_entropy(&mut controller[..])
            .map_err(RemoteControlNodeIdentityGenerationError::ControllerEntropy)?;
        let mut target = IdentitySecretKey::new([0; IDENTITY_SECRET_KEY_LEN]);
        fill_entropy(&mut target[..])
            .map_err(RemoteControlNodeIdentityGenerationError::TargetEntropy)?;
        Self::new(
            RemoteControlControllerIdentitySecret::from(controller),
            RemoteControlTargetIdentitySecret::from(target),
        )
        .map_err(RemoteControlNodeIdentityGenerationError::InvalidPair)
    }

    pub fn new(
        controller: RemoteControlControllerIdentitySecret,
        target: RemoteControlTargetIdentitySecret,
    ) -> Result<Self, RemoteControlNodeIdentitySecretsError> {
        Self::validate_identity_hashes(&controller.parts.hash, &target.parts.hash)?;
        Ok(Self { controller, target })
    }

    pub(crate) fn validate_secret_keys(
        controller: &IdentitySecretKey,
        target: &IdentitySecretKey,
    ) -> Result<(), RemoteControlNodeIdentitySecretsError> {
        let controller = InMemoryNodeIdentity::from_secret_key_bytes(controller);
        let target = InMemoryNodeIdentity::from_secret_key_bytes(target);
        Self::validate_identity_hashes(&controller.into_parts().hash, &target.into_parts().hash)
    }

    fn validate_identity_hashes(
        controller: &IdentityHash,
        target: &IdentityHash,
    ) -> Result<(), RemoteControlNodeIdentitySecretsError> {
        if controller == target {
            return Err(RemoteControlNodeIdentitySecretsError::ControllerAndTargetAreSameIdentity);
        }
        Ok(())
    }

    #[must_use]
    pub fn identities(&self) -> RemoteControlNodeIdentities {
        RemoteControlNodeIdentities {
            controller: RemoteControlControllerIdentity::new(IdentityPublicKeys {
                encryption: self.controller.parts.encryption_public,
                signing: self.controller.parts.signing_public,
            }),
            target: RemoteControlTargetIdentity::new(IdentityPublicKeys {
                encryption: self.target.parts.encryption_public,
                signing: self.target.parts.signing_public,
            }),
        }
    }

    pub(crate) fn into_parts(self) -> (IdentityParts, IdentityParts) {
        (self.controller.parts, self.target.parts)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlNodeIdentities {
    controller: RemoteControlControllerIdentity,
    target: RemoteControlTargetIdentity,
}

impl RemoteControlNodeIdentities {
    #[must_use]
    pub const fn controller(&self) -> &RemoteControlControllerIdentity {
        &self.controller
    }

    #[must_use]
    pub const fn target(&self) -> &RemoteControlTargetIdentity {
        &self.target
    }

    #[must_use]
    pub fn into_parts(self) -> (RemoteControlControllerIdentity, RemoteControlTargetIdentity) {
        (self.controller, self.target)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerIdentity {
    public_keys: IdentityPublicKeys,
}

impl RemoteControlControllerIdentity {
    #[must_use]
    pub const fn new(public_keys: IdentityPublicKeys) -> Self {
        Self { public_keys }
    }

    #[must_use]
    pub const fn public_keys(&self) -> &IdentityPublicKeys {
        &self.public_keys
    }

    #[must_use]
    pub fn identity_hash(&self) -> IdentityHash {
        self.public_keys.identity_hash()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerGrantError {
    NoPermittedRequests,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerGrant {
    controller: RemoteControlControllerIdentity,
    permitted_requests: RemoteControlRequestSet,
}

impl RemoteControlControllerGrant {
    pub fn new(
        controller: RemoteControlControllerIdentity,
        permitted_requests: RemoteControlRequestSet,
    ) -> Result<Self, RemoteControlControllerGrantError> {
        if permitted_requests.is_empty() {
            return Err(RemoteControlControllerGrantError::NoPermittedRequests);
        }
        Ok(Self {
            controller,
            permitted_requests,
        })
    }

    #[must_use]
    pub const fn controller(&self) -> &RemoteControlControllerIdentity {
        &self.controller
    }

    #[must_use]
    pub const fn permitted_requests(&self) -> &RemoteControlRequestSet {
        &self.permitted_requests
    }

    #[must_use]
    pub fn permits(&self, request: RemoteControlRequestKind) -> bool {
        self.permitted_requests.supports(request)
    }
}

impl
    From<(
        &RemoteControlControllerIdentity,
        &RemoteControlPairingPermissions,
    )> for RemoteControlControllerGrant
{
    fn from(
        (controller, permissions): (
            &RemoteControlControllerIdentity,
            &RemoteControlPairingPermissions,
        ),
    ) -> Self {
        Self {
            controller: *controller,
            permitted_requests: *permissions.permitted_requests(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlTargetIdentity {
    public_keys: IdentityPublicKeys,
}

impl RemoteControlTargetIdentity {
    #[must_use]
    pub const fn new(public_keys: IdentityPublicKeys) -> Self {
        Self { public_keys }
    }

    #[must_use]
    pub const fn public_keys(&self) -> &IdentityPublicKeys {
        &self.public_keys
    }

    #[must_use]
    pub fn identity_hash(&self) -> IdentityHash {
        self.public_keys.identity_hash()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetAccessError {
    NoPermittedRequests,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlTargetAccess {
    target: RemoteControlTargetIdentity,
    permitted_requests: RemoteControlRequestSet,
}

impl RemoteControlTargetAccess {
    pub fn new(
        target: RemoteControlTargetIdentity,
        permitted_requests: RemoteControlRequestSet,
    ) -> Result<Self, RemoteControlTargetAccessError> {
        if permitted_requests.is_empty() {
            return Err(RemoteControlTargetAccessError::NoPermittedRequests);
        }
        Ok(Self {
            target,
            permitted_requests,
        })
    }

    #[must_use]
    pub const fn target(&self) -> &RemoteControlTargetIdentity {
        &self.target
    }

    #[must_use]
    pub fn endpoint(&self) -> super::RemoteControlEndpoint {
        self.target.endpoint()
    }

    #[must_use]
    pub const fn permitted_requests(&self) -> &RemoteControlRequestSet {
        &self.permitted_requests
    }

    #[must_use]
    pub fn permits(&self, request: RemoteControlRequestKind) -> bool {
        self.permitted_requests.supports(request)
    }
}

impl From<(RemoteControlTargetIdentity, RemoteControlPairingPermissions)>
    for RemoteControlTargetAccess
{
    fn from(
        (target, permissions): (RemoteControlTargetIdentity, RemoteControlPairingPermissions),
    ) -> Self {
        Self {
            target,
            permitted_requests: permissions.into_permitted_requests(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRemoteControlControllerGrantOutcome {
    Added,
    Unchanged,
    Updated {
        previous: RemoteControlControllerGrant,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRemoteControlControllerGrantError {
    CapacityExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeRemoteControlControllerOutcome {
    Revoked { grant: RemoteControlControllerGrant },
    NotFound,
}

pub trait RemoteControlControllerGrantTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;
    fn grants_in_identity_hash_order(&self) -> &[RemoteControlControllerGrant];
    fn set_controller_grant(
        &mut self,
        grant: RemoteControlControllerGrant,
    ) -> Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantError>;
    fn revoke_controller(
        &mut self,
        controller: &RemoteControlControllerIdentity,
    ) -> RevokeRemoteControlControllerOutcome;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn grant_for(&self, identity: &IdentityHash) -> Option<&RemoteControlControllerGrant> {
        self.grants_in_identity_hash_order()
            .iter()
            .find(|grant| grant.controller().identity_hash() == *identity)
    }

    fn contains_controller(&self, identity: &IdentityHash) -> bool {
        self.grant_for(identity).is_some()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SetRemoteControlTargetAccessOutcome {
    Added,
    Unchanged,
    Updated { previous: RemoteControlTargetAccess },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRemoteControlTargetAccessError {
    CapacityExhausted,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ForgetRemoteControlTargetOutcome {
    Forgotten { access: RemoteControlTargetAccess },
    NotFound,
}

pub trait RemoteControlTargetAccessTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;
    fn accesses_in_identity_hash_order(&self) -> &[RemoteControlTargetAccess];
    fn set_target_access(
        &mut self,
        access: RemoteControlTargetAccess,
    ) -> Result<SetRemoteControlTargetAccessOutcome, SetRemoteControlTargetAccessError>;
    fn forget_target(
        &mut self,
        target: &RemoteControlTargetIdentity,
    ) -> ForgetRemoteControlTargetOutcome;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn access_for(&self, identity: &IdentityHash) -> Option<&RemoteControlTargetAccess> {
        self.accesses_in_identity_hash_order()
            .iter()
            .find(|access| access.target().identity_hash() == *identity)
    }

    fn contains_target(&self, identity: &IdentityHash) -> bool {
        self.access_for(identity).is_some()
    }
}
