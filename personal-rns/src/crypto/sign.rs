use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::CryptoError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ed25519PublicKey(pub [u8; 32]);

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Ed25519SecretKey([u8; 32]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ed25519Signature(pub [u8; 64]);

impl Ed25519SecretKey {
    pub const fn new(seed: [u8; 32]) -> Self {
        Self(seed)
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
    let signing_key = SigningKey::from_bytes(&secret.0);
    Ed25519Signature(signing_key.sign(message).to_bytes())
}

pub fn ed25519_public_key(secret: &Ed25519SecretKey) -> Ed25519PublicKey {
    Ed25519PublicKey(SigningKey::from_bytes(&secret.0).verifying_key().to_bytes())
}
