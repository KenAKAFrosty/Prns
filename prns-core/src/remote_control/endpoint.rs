use crate::routing::announce::{derive_destination_hash, DottedNameHash};
use crate::wire::DestinationHash;

use super::RemoteControlTargetIdentity;

pub const REMOTE_CONTROL_APPLICATION_NAME: &str = "reticulum";
pub const REMOTE_CONTROL_APPLICATION_ASPECTS: &[&str] = &["remote", "control"];

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
    use crate::identity::IdentityHash;
    use crate::routing::announce::expand_name;
    use crate::wire::TRUNCATED_HASH_BYTE_LEN;

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
        let target_identity =
            RemoteControlTargetIdentity::new(IdentityHash::new([0x41; TRUNCATED_HASH_BYTE_LEN]));

        assert_eq!(
            target_identity.endpoint().destination_hash(),
            DestinationHash::new([
                0x89, 0x9c, 0x44, 0x9b, 0x33, 0x48, 0x1f, 0x14, 0x5a, 0xc6, 0x20, 0x5e, 0x46, 0x32,
                0x6c, 0xe1,
            ]),
        );
    }
}
