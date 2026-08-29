use crate::routing::announce::{derive_destination_hash, DottedNameHash};
use crate::wire::DestinationHash;

use super::RemoteControlTargetIdentity;

pub const REMOTE_CONTROL_APPLICATION_NAME: &str = "reticulum";
pub(crate) const REMOTE_CONTROL_NAMESPACE_ASPECT: &str = "remote";
pub(crate) const REMOTE_CONTROL_SERVICE_ASPECT: &str = "control";
pub const REMOTE_CONTROL_APPLICATION_ASPECTS: &[&str] = &[
    REMOTE_CONTROL_NAMESPACE_ASPECT,
    REMOTE_CONTROL_SERVICE_ASPECT,
];

/// Pre-computed and saved here statically to avoid unnecessary hashing at runtime for what is a stable hash on these well-known app name & aspects
const REMOTE_CONTROL_DOTTED_NAME_HASH: DottedNameHash =
    DottedNameHash::new([0xfc, 0xce, 0x4e, 0xf8, 0x4b, 0x57, 0xc8, 0xe9, 0xb2, 0xe3]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlEndpoint {
    destination_hash: DestinationHash,
}

impl RemoteControlEndpoint {
    #[must_use]
    pub const fn destination_hash(&self) -> DestinationHash {
        self.destination_hash
    }
}

impl From<&RemoteControlTargetIdentity> for RemoteControlEndpoint {
    fn from(target_identity: &RemoteControlTargetIdentity) -> Self {
        Self {
            destination_hash: derive_destination_hash(
                &target_identity.identity_hash(),
                &REMOTE_CONTROL_DOTTED_NAME_HASH,
            ),
        }
    }
}

impl From<RemoteControlEndpoint> for DestinationHash {
    fn from(endpoint: RemoteControlEndpoint) -> Self {
        endpoint.destination_hash
    }
}

impl RemoteControlTargetIdentity {
    #[must_use]
    pub fn endpoint(&self) -> RemoteControlEndpoint {
        self.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
    };
    use crate::routing::announce::expand_name;

    #[test]
    fn pinned_remote_control_dotted_name_hash_matches_sha256_of_the_canonical_name() {
        assert_eq!(
            expand_name(
                REMOTE_CONTROL_APPLICATION_NAME,
                REMOTE_CONTROL_APPLICATION_ASPECTS,
            ),
            Ok(REMOTE_CONTROL_DOTTED_NAME_HASH),
        );
    }

    #[test]
    fn target_identity_derives_its_remote_control_endpoint() {
        let target_identity = RemoteControlTargetIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey(
                [0x41; X25519PublicKey::LEN],
            )),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0x42; Ed25519PublicKey::LEN])),
        });

        assert_eq!(
            target_identity.endpoint().destination_hash(),
            DestinationHash::new([
                0xb3, 0x6f, 0x62, 0x54, 0x71, 0x3a, 0xf4, 0x56, 0x2e, 0x66, 0x42, 0xb3, 0x90, 0x20,
                0xf5, 0xb0,
            ]),
        );
    }
}
