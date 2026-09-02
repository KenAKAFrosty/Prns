use x25519_dalek::{x25519, PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X25519PublicKey(pub [u8; 32]);

impl X25519PublicKey {
    pub const LEN: usize = 32;
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct X25519SecretKey([u8; 32]);

// Test assertions need to compare complete typed ingress outcomes, some of which own secret
// material. Keep those conveniences out of production builds and never print the key bytes.
#[cfg(test)]
impl core::fmt::Debug for X25519SecretKey {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("X25519SecretKey([REDACTED])")
    }
}

#[cfg(test)]
impl PartialEq for X25519SecretKey {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

#[cfg(test)]
impl Eq for X25519SecretKey {}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct X25519SharedSecret([u8; 32]);

impl X25519SecretKey {
    pub const LEN: usize = 32;

    pub const fn new(scalar: [u8; 32]) -> Self {
        Self(scalar)
    }

    pub(crate) fn cloned(&self) -> Self {
        Self(self.0)
    }

    pub(crate) fn secret_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn with_scalar_bytes<R>(&self, use_bytes: impl FnOnce(&[u8; 32]) -> R) -> R {
        use_bytes(&self.0)
    }
}

impl X25519SharedSecret {
    pub const LEN: usize = 32;

    pub const fn from_external_diffie_hellman(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// The shared secret of `secret` with `peer` (RFC 7748 clamping applied).
pub fn x25519_diffie_hellman(
    secret: &X25519SecretKey,
    peer: &X25519PublicKey,
) -> X25519SharedSecret {
    X25519SharedSecret(x25519(secret.0, peer.0))
}

pub fn x25519_public_key(secret: &X25519SecretKey) -> X25519PublicKey {
    // `PublicKey::from` rides the precomputed basepoint table: same clamping, same bytes, a third of the variable-base ladder's time.
    // Every single's seal pays this once.
    X25519PublicKey(*PublicKey::from(&StaticSecret::from(secret.0)).as_bytes())
}

pub fn x25519_keys_for_seal(
    ephemeral_secret: &X25519SecretKey,
    dh_target: &X25519PublicKey,
) -> (X25519PublicKey, X25519SharedSecret) {
    (
        x25519_public_key(ephemeral_secret),
        x25519_diffie_hellman(ephemeral_secret, dh_target),
    )
}
