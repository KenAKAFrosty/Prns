use core::fmt;

use super::in_memory::InMemoryNodeIdentity;
use super::vault::IdentitySecretKey;
use super::{
    DecryptError, EncryptError, IdentityEncryptionPublicKey, IdentityHash, IdentityPublicKeys,
    IdentitySigner, IdentitySigningPublicKey, RemoteIdentity, ENCRYPTION_IV_LEN,
    IDENTITY_PUBLIC_KEY_LEN, IDENTITY_SECRET_KEY_LEN,
};
use crate::crypto::{
    ed25519_verify, Ed25519PublicKey, Ed25519Signature, InvalidSignature, X25519PublicKey,
    X25519SecretKey,
};

pub struct PrivateIdentityMaterial(IdentitySecretKey);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicIdentityMaterial([u8; IDENTITY_PUBLIC_KEY_LEN]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityMaterialLengthError {
    pub expected: usize,
    pub found: usize,
}

impl PrivateIdentityMaterial {
    pub fn from_bytes(bytes: [u8; IDENTITY_SECRET_KEY_LEN]) -> Self {
        Self(IdentitySecretKey::new(bytes))
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, IdentityMaterialLengthError> {
        let bytes: [u8; IDENTITY_SECRET_KEY_LEN] =
            bytes.try_into().map_err(|_| IdentityMaterialLengthError {
                expected: IDENTITY_SECRET_KEY_LEN,
                found: bytes.len(),
            })?;
        Ok(Self::from_bytes(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; IDENTITY_SECRET_KEY_LEN] {
        &self.0
    }

    pub fn public(&self) -> PublicIdentityMaterial {
        PublicIdentityMaterial::from_bytes(self.identity().public_key_bytes())
    }

    pub fn identity_hash(&self) -> IdentityHash {
        self.identity().identity_hash()
    }

    pub fn sign(&self, message: &[u8]) -> Ed25519Signature {
        self.identity().sign(message)
    }

    pub fn decrypt(&self, ciphertext: &[u8], out: &mut [u8]) -> Result<usize, DecryptError> {
        self.identity().decrypt(ciphertext, out)
    }

    fn identity(&self) -> InMemoryNodeIdentity {
        InMemoryNodeIdentity::from_secret_key_bytes(&self.0)
    }
}

impl From<IdentitySecretKey> for PrivateIdentityMaterial {
    fn from(secret: IdentitySecretKey) -> Self {
        Self(secret)
    }
}

impl PublicIdentityMaterial {
    pub const fn from_bytes(bytes: [u8; IDENTITY_PUBLIC_KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, IdentityMaterialLengthError> {
        let bytes: [u8; IDENTITY_PUBLIC_KEY_LEN] =
            bytes.try_into().map_err(|_| IdentityMaterialLengthError {
                expected: IDENTITY_PUBLIC_KEY_LEN,
                found: bytes.len(),
            })?;
        Ok(Self::from_bytes(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; IDENTITY_PUBLIC_KEY_LEN] {
        &self.0
    }

    pub fn public_keys(&self) -> IdentityPublicKeys {
        let mut encryption = [0u8; X25519PublicKey::LEN];
        encryption.copy_from_slice(&self.0[..X25519PublicKey::LEN]);
        let mut signing = [0u8; Ed25519PublicKey::LEN];
        signing.copy_from_slice(&self.0[X25519PublicKey::LEN..]);
        IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey(encryption)),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey(signing)),
        }
    }

    pub fn identity_hash(&self) -> IdentityHash {
        self.public_keys().identity_hash()
    }

    pub fn verify(
        &self,
        message: &[u8],
        signature: &Ed25519Signature,
    ) -> Result<(), InvalidSignature> {
        ed25519_verify(self.public_keys().signing.as_ed25519(), message, signature)
    }

    pub fn encrypt(
        &self,
        ephemeral_secret: &X25519SecretKey,
        iv: &[u8; ENCRYPTION_IV_LEN],
        plaintext: &[u8],
        out: &mut [u8],
    ) -> Result<usize, EncryptError> {
        let keys = self.public_keys();
        RemoteIdentity::from_public_keys(keys.encryption, keys.signing).encrypt(
            ephemeral_secret,
            iv,
            plaintext,
            out,
        )
    }
}

impl fmt::Display for IdentityMaterialLengthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "identity material holds {} bytes, expected {}",
            self.found, self.expected
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for IdentityMaterialLengthError {}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_PUBLIC: [u8; IDENTITY_PUBLIC_KEY_LEN] = hex_literal();

    const fn hex_literal() -> [u8; IDENTITY_PUBLIC_KEY_LEN] {
        [
            0x0f, 0xaa, 0x68, 0x4e, 0xd2, 0x88, 0x67, 0xb9, 0x7f, 0x4a, 0x6a, 0x2d, 0xee, 0x5d,
            0xf8, 0xce, 0x97, 0x4e, 0x76, 0xb7, 0x01, 0x8e, 0x3f, 0x22, 0xa1, 0xc4, 0xcf, 0x26,
            0x78, 0x57, 0x0f, 0x20, 0xd0, 0x4a, 0xb2, 0x32, 0x74, 0x2b, 0xb4, 0xab, 0x3a, 0x13,
            0x68, 0xbd, 0x46, 0x15, 0xe4, 0xe6, 0xd0, 0x22, 0x4a, 0xb7, 0x1a, 0x01, 0x6b, 0xaf,
            0x85, 0x20, 0xa3, 0x32, 0xc9, 0x77, 0x87, 0x37,
        ]
    }

    fn fixed_private() -> PrivateIdentityMaterial {
        let mut bytes = [0u8; IDENTITY_SECRET_KEY_LEN];
        bytes[..32].fill(0x22);
        bytes[32..].fill(0x11);
        PrivateIdentityMaterial::from_bytes(bytes)
    }

    #[test]
    fn private_material_derives_the_rns_1_4_0_public_identity_and_hash() {
        let private = fixed_private();
        assert_eq!(private.public().as_bytes(), &EXPECTED_PUBLIC);
        assert_eq!(
            private.identity_hash(),
            IdentityHash::new([
                0x4c, 0xd0, 0xcc, 0x45, 0xa7, 0x40, 0x5d, 0xbd, 0x5c, 0xf9, 0xb5, 0xbe, 0x1e, 0xf9,
                0x2f, 0x10,
            ])
        );
    }

    #[test]
    fn raw_signature_matches_rns_1_4_0_and_verifies_with_public_material() {
        let private = fixed_private();
        let signature = private.sign(b"local-id-oracle");
        assert_eq!(
            signature.0,
            [
                0x78, 0x58, 0xaf, 0x1c, 0xa0, 0x88, 0x77, 0x31, 0x35, 0xf3, 0xcf, 0xc4, 0x8c, 0x86,
                0x81, 0x69, 0x9c, 0x3b, 0xd4, 0x74, 0x4b, 0xb0, 0xf0, 0xfb, 0xdc, 0x72, 0xc2, 0x76,
                0x07, 0x48, 0x76, 0x36, 0x8d, 0xf6, 0x09, 0x11, 0xaa, 0x18, 0x90, 0xb5, 0x20, 0xb9,
                0xfd, 0x14, 0x16, 0xaa, 0x47, 0xc0, 0x10, 0xa1, 0xc7, 0x41, 0x61, 0xf1, 0xab, 0x37,
                0x8a, 0xb7, 0x58, 0x8f, 0xad, 0x18, 0xc6, 0x0d,
            ]
        );
        assert_eq!(
            private.public().verify(b"local-id-oracle", &signature),
            Ok(())
        );
    }

    #[test]
    fn public_encrypt_and_private_decrypt_round_trip() {
        let private = fixed_private();
        let mut ciphertext = [0u8; 128];
        let written = private
            .public()
            .encrypt(
                &X25519SecretKey::new([0x33; 32]),
                &[0x44; ENCRYPTION_IV_LEN],
                b"local identity",
                &mut ciphertext,
            )
            .unwrap();
        let mut plaintext = [0u8; 64];
        let opened = private
            .decrypt(&ciphertext[..written], &mut plaintext)
            .unwrap();
        assert_eq!(&plaintext[..opened], b"local identity");
    }
}
