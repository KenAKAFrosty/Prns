//! Optional adapter for the existing core Reticulum identity capability.

use crate::{CardFields, ContactCardSigner, EncodeError, SERVICE_NAME_HASH_BYTES, SIGNATURE_BYTES};
use core::convert::Infallible;
use prns_core::identity::PrivateIdentityMaterial;

impl ContactCardSigner for PrivateIdentityMaterial {
    type Error = Infallible;

    fn sign(&self, message: &[u8]) -> Result<[u8; SIGNATURE_BYTES], Self::Error> {
        Ok(PrivateIdentityMaterial::sign(self, message).0)
    }
}

/// Sign a service claim using the core identity's own public keys.
///
/// The caller retains the identity capability for this synchronous operation.
/// No key bytes, identity owner, catalog, host or runtime are created or retained.
/// A native key owner can pass its lease's borrowed `material()` here. The same
/// adapter is available to allocation-free Rust consumers with a core identity.
pub fn encode_identity(
    identity: &PrivateIdentityMaterial,
    service_name_hash: &[u8; SERVICE_NAME_HASH_BYTES],
    display_name: &str,
    output: &mut [u8],
) -> Result<usize, EncodeError<Infallible>> {
    crate::encode_signed(
        CardFields {
            public_identity: identity.public().as_bytes(),
            service_name_hash,
            display_name,
        },
        identity,
        output,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_identity_service_claims_bind_its_own_public_keys() {
        let identity = PrivateIdentityMaterial::from_bytes([91; 64]);
        for service in [
            "prns.groups.enrollment",
            "example.telemetry",
            "lxmf.delivery",
        ] {
            let service_hash = crate::service_name_hash(service);
            let mut wire = [0; crate::MAX_CARD_BYTES];
            let count = encode_identity(&identity, &service_hash, "Sensor α", &mut wire).unwrap();
            let card = crate::verify(&wire[..count]).unwrap();
            assert_eq!(card.identity_hash(), identity.identity_hash().as_bytes());
            assert_eq!(card.public_identity(), identity.public().as_bytes());
            assert_eq!(card.service_name_hash(), &service_hash);
            assert_eq!(card.display_name(), "Sensor α");
            let mut token = [0; crate::MAX_TOKEN_BYTES];
            let token_len = crate::encode_token(card.wire(), &mut token).unwrap();
            let mut decoded = [0; crate::MAX_CARD_BYTES];
            let decoded_len = crate::decode_token(
                core::str::from_utf8(&token[..token_len]).unwrap(),
                &mut decoded,
            )
            .unwrap();
            assert_eq!(
                crate::verify(&decoded[..decoded_len]).unwrap().wire(),
                card.wire()
            );
        }
    }
}
