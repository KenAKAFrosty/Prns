use x25519_dalek::{x25519, PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X25519PublicKey(pub [u8; 32]);

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct X25519SecretKey([u8; 32]);

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct X25519SharedSecret([u8; 32]);

impl X25519SecretKey {
    pub const fn new(scalar: [u8; 32]) -> Self {
        Self(scalar)
    }
}

impl X25519SharedSecret {
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

/// `PublicKey::from` rides the precomputed basepoint table where the raw
/// `x25519(secret, X25519_BASEPOINT_BYTES)` form always walks the
/// variable-base Montgomery ladder — same clamping, same point, same bytes,
/// a third of the time. Every single's seal pays this once for its ephemeral.
pub fn x25519_public_key(secret: &X25519SecretKey) -> X25519PublicKey {
    X25519PublicKey(*PublicKey::from(&StaticSecret::from(secret.0)).as_bytes())
}
