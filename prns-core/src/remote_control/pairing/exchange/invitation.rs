use core::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::crypto::{hmac_sha256_chunks, sha256_chunks, HmacSha256Stream};
use crate::remote_control::{RemoteControlControllerIdentity, RemoteControlPairingEndpoint};

const REMOTE_CONTROL_PAIRING_INVITATION_CODE_LEN: usize = 4;
const REMOTE_CONTROL_PAIRING_INVITATION_PROOF_LEN: usize = 32;
const REMOTE_CONTROL_PAIRING_INVITATION_KEY_DOMAIN: &[u8] =
    b"reticulum.remote.control.pairing.invitation.key.v1";
const REMOTE_CONTROL_PAIRING_INVITATION_PROOF_DOMAIN: &[u8] =
    b"reticulum.remote.control.pairing.invitation.proof.v1";

#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct RemoteControlPairingInvitationCode([u8; REMOTE_CONTROL_PAIRING_INVITATION_CODE_LEN]);

impl RemoteControlPairingInvitationCode {
    pub const ENTROPY_LEN: usize = REMOTE_CONTROL_PAIRING_INVITATION_CODE_LEN;

    #[must_use]
    pub const fn from_entropy(entropy: [u8; REMOTE_CONTROL_PAIRING_INVITATION_CODE_LEN]) -> Self {
        Self(entropy)
    }

    #[must_use]
    pub const fn from_value(value: u32) -> Self {
        Self(value.to_le_bytes())
    }

    #[must_use]
    pub fn value(&self) -> u32 {
        u32::from_le_bytes(self.0)
    }

    #[must_use]
    pub fn verifier(&self) -> RemoteControlPairingInvitationVerifier {
        RemoteControlPairingInvitationVerifier(sha256_chunks(&[
            REMOTE_CONTROL_PAIRING_INVITATION_KEY_DOMAIN,
            &self.0,
        ]))
    }

    #[must_use]
    pub fn into_proof(
        self,
        endpoint: RemoteControlPairingEndpoint,
        controller: &RemoteControlControllerIdentity,
    ) -> RemoteControlPairingInvitationProof {
        self.verifier().proof(endpoint, controller)
    }
}

impl fmt::Debug for RemoteControlPairingInvitationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RemoteControlPairingInvitationCode")
            .field(&"REDACTED")
            .finish()
    }
}

impl fmt::Display for RemoteControlPairingInvitationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:08X}", self.value())
    }
}

#[derive(PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct RemoteControlPairingInvitationVerifier(
    [u8; REMOTE_CONTROL_PAIRING_INVITATION_PROOF_LEN],
);

impl RemoteControlPairingInvitationVerifier {
    fn proof(
        &self,
        endpoint: RemoteControlPairingEndpoint,
        controller: &RemoteControlControllerIdentity,
    ) -> RemoteControlPairingInvitationProof {
        let destination = endpoint.destination_hash();
        let controller = controller.public_keys().public_key_bytes();
        RemoteControlPairingInvitationProof(hmac_sha256_chunks(
            &self.0,
            &[
                REMOTE_CONTROL_PAIRING_INVITATION_PROOF_DOMAIN,
                destination.as_bytes(),
                &controller,
            ],
        ))
    }

    pub fn verify(
        &self,
        endpoint: RemoteControlPairingEndpoint,
        controller: &RemoteControlControllerIdentity,
        proof: &RemoteControlPairingInvitationProof,
    ) -> Result<(), RemoteControlPairingInvitationProofInvalid> {
        let destination = endpoint.destination_hash();
        let controller = controller.public_keys().public_key_bytes();
        let mut verifier = HmacSha256Stream::keyed(&self.0);
        verifier.update(REMOTE_CONTROL_PAIRING_INVITATION_PROOF_DOMAIN);
        verifier.update(destination.as_bytes());
        verifier.update(&controller);
        verifier
            .verify(&proof.0)
            .map_err(|_| RemoteControlPairingInvitationProofInvalid)
    }
}

impl fmt::Debug for RemoteControlPairingInvitationVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RemoteControlPairingInvitationVerifier")
            .field(&"REDACTED")
            .finish()
    }
}

#[derive(PartialEq, Eq)]
pub struct RemoteControlPairingInvitationProof([u8; REMOTE_CONTROL_PAIRING_INVITATION_PROOF_LEN]);

impl RemoteControlPairingInvitationProof {
    pub(super) const fn from_wire(
        bytes: [u8; REMOTE_CONTROL_PAIRING_INVITATION_PROOF_LEN],
    ) -> Self {
        Self(bytes)
    }

    pub(super) const fn as_wire(&self) -> &[u8; REMOTE_CONTROL_PAIRING_INVITATION_PROOF_LEN] {
        &self.0
    }
}

impl fmt::Debug for RemoteControlPairingInvitationProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RemoteControlPairingInvitationProof")
            .field(&"REDACTED")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPairingInvitationProofInvalid;

pub(super) const PAIRING_INVITATION_PROOF_ENCODED_LEN: usize =
    REMOTE_CONTROL_PAIRING_INVITATION_PROOF_LEN;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::vault::IdentitySecretKey;
    use crate::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
    use crate::remote_control::RemoteControlPairingIdentity;

    fn controller(fill: u8) -> RemoteControlControllerIdentity {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
            [fill; crate::identity::IDENTITY_SECRET_KEY_LEN],
        ));
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: signer.encryption_public_key(),
            signing: signer.signing_public_key(),
        })
    }

    fn endpoint(fill: u8) -> RemoteControlPairingEndpoint {
        RemoteControlPairingIdentity::new(IdentityHash::new([fill; 16])).endpoint()
    }

    #[test]
    fn invitation_proofs_are_bound_to_code_endpoint_and_controller() {
        let code = RemoteControlPairingInvitationCode::from_value(0x1234_ABCD);
        let verifier = code.verifier();
        let proof = code.into_proof(endpoint(0x41), &controller(0x51));

        assert_eq!(
            verifier.verify(endpoint(0x41), &controller(0x51), &proof),
            Ok(()),
        );
        assert_eq!(
            verifier.verify(endpoint(0x42), &controller(0x51), &proof),
            Err(RemoteControlPairingInvitationProofInvalid),
        );
        assert_eq!(
            verifier.verify(endpoint(0x41), &controller(0x52), &proof),
            Err(RemoteControlPairingInvitationProofInvalid),
        );
        assert_eq!(
            RemoteControlPairingInvitationCode::from_value(0x1234_ABCE)
                .verifier()
                .verify(endpoint(0x41), &controller(0x51), &proof),
            Err(RemoteControlPairingInvitationProofInvalid),
        );
    }

    #[test]
    fn invitation_codes_are_fixed_width_and_debug_redacted() {
        let code = RemoteControlPairingInvitationCode::from_value(0x12AB);
        assert_eq!(code.to_string(), "000012AB");
        assert_eq!(
            format!("{code:?}"),
            "RemoteControlPairingInvitationCode(\"REDACTED\")",
        );
    }
}
