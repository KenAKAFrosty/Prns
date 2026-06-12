use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use zeroize::ZeroizeOnDrop;

use super::CryptoError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ed25519PublicKey(pub [u8; 32]);

/// Drop zeroizes the seed through `SigningKey`'s own `Drop` (dalek's `zeroize`
/// feature); the marker below asserts what the derive used to.
pub struct Ed25519SecretKey(SigningKey);

impl ZeroizeOnDrop for Ed25519SecretKey {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ed25519Signature(pub [u8; 64]);

impl Ed25519SecretKey {
    /// Expands the seed once — `SigningKey` carries its derived verifying key, so the
    /// per-sign cost from here on is one basepoint multiplication, not two. Held
    /// identities live as long as the engine; paying expansion per signature was
    /// a measured ~25µs of every proof.
    pub fn new(seed: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&seed))
    }
}

pub fn ed25519_verify(
    key: &Ed25519PublicKey,
    message: &[u8],
    signature: &Ed25519Signature,
) -> Result<(), CryptoError> {
    let verifying_key =
        VerifyingKey::from_bytes(&key.0).map_err(|_| CryptoError::InvalidSignature)?;
    let signature = Signature::from_bytes(&signature.0);
    verifying_key
        .verify(message, &signature)
        .map_err(|_| CryptoError::InvalidSignature)
}

/// (deterministic, RFC 8032).
pub fn ed25519_sign(secret: &Ed25519SecretKey, message: &[u8]) -> Ed25519Signature {
    Ed25519Signature(secret.0.sign(message).to_bytes())
}

pub fn ed25519_public_key(secret: &Ed25519SecretKey) -> Ed25519PublicKey {
    Ed25519PublicKey(secret.0.verifying_key().to_bytes())
}
