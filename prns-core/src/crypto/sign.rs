use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use zeroize::ZeroizeOnDrop;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidSignature;

/// The bytes are not a decompressible Edwards point, so no signature under this key can verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPublicKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ed25519PublicKey(pub [u8; 32]);

pub struct Ed25519SecretKey(SigningKey);
impl ZeroizeOnDrop for Ed25519SecretKey {}
const _: () = {
    const fn dalek_wipes_the_seed_on_drop<T: ZeroizeOnDrop>() {}
    dalek_wipes_the_seed_on_drop::<SigningKey>()
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ed25519Signature(pub [u8; 64]);

impl Ed25519SecretKey {
    /// Expands the seed once: per-sign is one basepoint mult, not two.
    /// Expansion per signature was a measured ~25µs of every proof.
    pub fn new(seed: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&seed))
    }

    pub(crate) fn cloned(&self) -> Self {
        Self(self.0.clone())
    }
}

/// Decompresses the Edwards point once, for performance reasons.
/// `VerifyingKey::from_bytes` per proof was a measured ~8% of a firehose initiator's CPU.
#[derive(Debug, Clone)]
pub struct Ed25519Verifier {
    public: Ed25519PublicKey,
    key: VerifyingKey,
}

impl Ed25519Verifier {
    pub fn new(public: &Ed25519PublicKey) -> Result<Self, InvalidPublicKey> {
        let key = VerifyingKey::from_bytes(&public.0).map_err(|_| InvalidPublicKey)?;
        Ok(Self {
            public: *public,
            key,
        })
    }

    pub fn public_key(&self) -> &Ed25519PublicKey {
        &self.public
    }

    pub fn verify(
        &self,
        message: &[u8],
        signature: &Ed25519Signature,
    ) -> Result<(), InvalidSignature> {
        let signature = Signature::from_bytes(&signature.0);
        self.key
            .verify(message, &signature)
            .map_err(|_| InvalidSignature)
    }
}

pub fn ed25519_verify(
    key: &Ed25519PublicKey,
    message: &[u8],
    signature: &Ed25519Signature,
) -> Result<(), InvalidSignature> {
    Ed25519Verifier::new(key)
        .map_err(|InvalidPublicKey| InvalidSignature)?
        .verify(message, signature)
}

/// (deterministic, RFC 8032).
pub fn ed25519_sign(secret: &Ed25519SecretKey, message: &[u8]) -> Ed25519Signature {
    Ed25519Signature(secret.0.sign(message).to_bytes())
}

pub fn ed25519_public_key(secret: &Ed25519SecretKey) -> Ed25519PublicKey {
    Ed25519PublicKey(secret.0.verifying_key().to_bytes())
}
