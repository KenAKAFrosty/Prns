use crate::identity::in_memory::{IdentityParts, InMemoryNodeIdentity};
use crate::identity::vault::IdentitySecretKey;
use crate::identity::{IdentityHash, IdentityPublicKeys, IDENTITY_SECRET_KEY_LEN};
use crate::storage::TablePushError;

use super::{RemoteControlRequestKind, RemoteControlRequestSet};

pub const REMOTE_CONTROL_REQUIRED_HELD_IDENTITY_CAPACITY: usize = 2;

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
            target: RemoteControlTargetIdentity::new(self.target.parts.hash),
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

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlTargetIdentity {
    identity_hash: IdentityHash,
}

impl RemoteControlTargetIdentity {
    #[must_use]
    pub const fn new(identity_hash: IdentityHash) -> Self {
        Self { identity_hash }
    }

    #[must_use]
    pub const fn identity_hash(&self) -> IdentityHash {
        self.identity_hash
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
    Unavailable,
    CapacityExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeRemoteControlControllerError {
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeRemoteControlControllerOutcome {
    Revoked { grant: RemoteControlControllerGrant },
    NotFound,
}

pub trait RemoteControlAccessTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;
    fn grants(&self) -> &[RemoteControlControllerGrant];
    fn upsert(&mut self, grant: RemoteControlControllerGrant) -> Result<(), TablePushError>;
    fn swap_remove(&mut self, index: usize);

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn index_of(&self, identity: &IdentityHash) -> Option<usize> {
        self.grants()
            .iter()
            .position(|grant| grant.controller().identity_hash() == *identity)
    }

    fn grant_for(&self, identity: &IdentityHash) -> Option<&RemoteControlControllerGrant> {
        //It's expected this will be a very short list (single digit members allowed in most cases, etc.)
        //Because the domain fundamentally wants to keep this tight, quick scans like this are not only okay but probably faster
        //If we start getting to like ~32 members or more to support real use cases or something, we'll want to improve this with a proper index.
        self.grants().get(self.index_of(identity)?)
    }

    fn contains(&self, identity: &IdentityHash) -> bool {
        self.index_of(identity).is_some()
    }

    fn set_controller_grant(
        &mut self,
        grant: RemoteControlControllerGrant,
    ) -> Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantError> {
        let identity = grant.controller().identity_hash();
        let previous = self.grant_for(&identity).copied();
        if previous == Some(grant) {
            return Ok(SetRemoteControlControllerGrantOutcome::Unchanged);
        }
        self.upsert(grant).map_err(|TablePushError::TableFull| {
            SetRemoteControlControllerGrantError::CapacityExhausted
        })?;
        Ok(match previous {
            Some(previous) => SetRemoteControlControllerGrantOutcome::Updated { previous },
            None => SetRemoteControlControllerGrantOutcome::Added,
        })
    }

    fn revoke_controller(
        &mut self,
        controller: &RemoteControlControllerIdentity,
    ) -> RevokeRemoteControlControllerOutcome {
        let Some(index) = self.index_of(&controller.identity_hash()) else {
            return RevokeRemoteControlControllerOutcome::NotFound;
        };
        let Some(grant) = self.grants().get(index).copied() else {
            return RevokeRemoteControlControllerOutcome::NotFound;
        };
        self.swap_remove(index);
        RevokeRemoteControlControllerOutcome::Revoked { grant }
    }
}
