use crate::entropy::{EntropySource, RuntimeEntropy};
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
    /// Peak storage owned by an available service while a pairing window is open.
    ///
    /// The service keeps its controller and target identities, target and pairing-availability
    /// destinations, and target request handler for its entire lifetime. Opening a pairing window
    /// adds one provisional identity, destination, and request handler until the window closes.
    pub const AVAILABLE: Self = Self {
        held_identities: 3,
        upstream_app_destinations: 3,
        // Live `/remote-control` plus the ephemeral pairing-session handler.
        request_handlers: 2,
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

impl core::fmt::Display for RemoteControlNodeIdentitySecretsError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("controller and target resolve to the same identity")
    }
}

impl core::error::Error for RemoteControlNodeIdentitySecretsError {}

pub struct RemoteControlNodeIdentitySecrets {
    controller: RemoteControlControllerIdentitySecret,
    target: RemoteControlTargetIdentitySecret,
}

pub struct RemoteControlTargetSealingKey(zeroize::Zeroizing<[u8; 64]>);

impl RemoteControlTargetSealingKey {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

impl RemoteControlNodeIdentitySecrets {
    pub fn generate_with_runtime_entropy<S: EntropySource>(
        entropy: &mut RuntimeEntropy<S>,
    ) -> Result<Self, RemoteControlNodeIdentitySecretsError> {
        let mut controller = IdentitySecretKey::new([0; IDENTITY_SECRET_KEY_LEN]);
        entropy.fill_random(&mut controller[..]);
        let mut target = IdentitySecretKey::new([0; IDENTITY_SECRET_KEY_LEN]);
        entropy.fill_random(&mut target[..]);
        Self::new(
            RemoteControlControllerIdentitySecret::from(controller),
            RemoteControlTargetIdentitySecret::from(target),
        )
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

    /// Derives a purpose-bound local sealing key without exposing target identity key material.
    ///
    /// The target identity hash is the HKDF salt and `domain` is the application context. Callers
    /// must use a stable, unique domain label for each stored-record family.
    pub fn target_sealing_key(&self, domain: &[u8]) -> RemoteControlTargetSealingKey {
        self.target
            .parts
            .encryption_secret
            .with_scalar_bytes(|secret| {
                RemoteControlTargetSealingKey(zeroize::Zeroizing::new(
                    crate::crypto::hkdf_sha256::<64>(
                        secret,
                        self.target.parts.hash.as_bytes(),
                        domain,
                    ),
                ))
            })
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
#[repr(u8)]
pub enum RemoteControlControllerAuthority {
    Operator = 0x01,
    Administrator = 0x02,
}

impl RemoteControlControllerAuthority {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) const fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Operator),
            0x02 => Some(Self::Administrator),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerGrantError {
    NoPermittedRequests,
    AdministratorRequestRequiresAuthority { request: RemoteControlRequestKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerGrant {
    controller: RemoteControlControllerIdentity,
    authority: RemoteControlControllerAuthority,
    permitted_requests: RemoteControlRequestSet,
}

impl RemoteControlControllerGrant {
    pub fn new(
        controller: RemoteControlControllerIdentity,
        authority: RemoteControlControllerAuthority,
        permitted_requests: RemoteControlRequestSet,
    ) -> Result<Self, RemoteControlControllerGrantError> {
        if permitted_requests.is_empty() {
            return Err(RemoteControlControllerGrantError::NoPermittedRequests);
        }
        if authority == RemoteControlControllerAuthority::Operator {
            if let Some(request) = permitted_requests
                .iter()
                .find(|request| request.requires_administrator())
            {
                return Err(
                    RemoteControlControllerGrantError::AdministratorRequestRequiresAuthority {
                        request,
                    },
                );
            }
        }
        Ok(Self {
            controller,
            authority,
            permitted_requests,
        })
    }

    #[must_use]
    pub const fn controller(&self) -> &RemoteControlControllerIdentity {
        &self.controller
    }

    #[must_use]
    pub const fn authority(&self) -> RemoteControlControllerAuthority {
        self.authority
    }

    #[must_use]
    pub const fn permitted_requests(&self) -> &RemoteControlRequestSet {
        &self.permitted_requests
    }

    #[must_use]
    pub fn permits(&self, request: RemoteControlRequestKind) -> bool {
        if request.requires_administrator() {
            self.authority == RemoteControlControllerAuthority::Administrator
        } else {
            self.permitted_requests.supports(request)
        }
    }

    #[must_use]
    pub fn effective_requests(&self) -> RemoteControlRequestSet {
        let mut requests = self.permitted_requests;
        if self.authority == RemoteControlControllerAuthority::Administrator {
            for request in RemoteControlRequestKind::ALL {
                if request.requires_administrator() {
                    let _inserted = requests.insert(request);
                }
            }
        }
        if self.authority == RemoteControlControllerAuthority::Administrator
            || requests.supports(RemoteControlRequestKind::DescribePower)
            || requests.supports(RemoteControlRequestKind::InventoryInterfaces)
        {
            let _inserted = requests.insert(RemoteControlRequestKind::DescribePower);
            let _inserted = requests.insert(RemoteControlRequestKind::FirmwareUpdate);
        }
        requests
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
            authority: permissions.authority(),
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
    AdministratorRequestRequiresAuthority { request: RemoteControlRequestKind },
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlTargetAccess {
    target: RemoteControlTargetIdentity,
    authority: RemoteControlControllerAuthority,
    permitted_requests: RemoteControlRequestSet,
}

impl RemoteControlTargetAccess {
    pub fn new(
        target: RemoteControlTargetIdentity,
        authority: RemoteControlControllerAuthority,
        permitted_requests: RemoteControlRequestSet,
    ) -> Result<Self, RemoteControlTargetAccessError> {
        if permitted_requests.is_empty() {
            return Err(RemoteControlTargetAccessError::NoPermittedRequests);
        }
        if authority == RemoteControlControllerAuthority::Operator {
            if let Some(request) = permitted_requests
                .iter()
                .find(|request| request.requires_administrator())
            {
                return Err(
                    RemoteControlTargetAccessError::AdministratorRequestRequiresAuthority {
                        request,
                    },
                );
            }
        }
        Ok(Self {
            target,
            authority,
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
    pub const fn authority(&self) -> RemoteControlControllerAuthority {
        self.authority
    }

    #[must_use]
    pub const fn permitted_requests(&self) -> &RemoteControlRequestSet {
        &self.permitted_requests
    }

    #[must_use]
    pub fn permits(&self, request: RemoteControlRequestKind) -> bool {
        if request.requires_administrator() {
            self.authority == RemoteControlControllerAuthority::Administrator
        } else {
            self.permitted_requests.supports(request)
        }
    }

    #[must_use]
    pub fn effective_requests(&self) -> RemoteControlRequestSet {
        let mut requests = self.permitted_requests;
        if self.authority == RemoteControlControllerAuthority::Administrator {
            for request in RemoteControlRequestKind::ALL {
                if request.requires_administrator() {
                    let _inserted = requests.insert(request);
                }
            }
        }
        if self.authority == RemoteControlControllerAuthority::Administrator
            || requests.supports(RemoteControlRequestKind::DescribePower)
            || requests.supports(RemoteControlRequestKind::InventoryInterfaces)
        {
            let _inserted = requests.insert(RemoteControlRequestKind::DescribePower);
            let _inserted = requests.insert(RemoteControlRequestKind::FirmwareUpdate);
        }
        requests
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
            authority: permissions.authority(),
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
    fn forget_by_identity_hash(
        &mut self,
        identity: &IdentityHash,
    ) -> ForgetRemoteControlTargetOutcome;

    fn forget_target(
        &mut self,
        target: &RemoteControlTargetIdentity,
    ) -> ForgetRemoteControlTargetOutcome {
        self.forget_by_identity_hash(&target.identity_hash())
    }

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
