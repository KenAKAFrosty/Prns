//! An [`Identity`] is an X25519 (encryption) keypair plus an Ed25519 (signing)
//! keypair; its [`IdentityHash`] is the truncated SHA-256 of the two public
//! keys and is how the network names the node. This module currently holds only
//! *our own* identity (we hold the secrets); recall/remember and the storage of
//! *other* nodes' identities will land here too as that work arrives — Identity
//! is one distinct concern, mirroring RNS's `Identity` module.
//!
//! **Sans-storage, by construction.** Per `crypto/`'s invariant, nothing here
//! generates randomness — [`Identity::from_entropy`] takes the key material as
//! an *input*. An ephemeral per-run identity is simply that fed from the host's
//! [`fill_entropy`](crate::engine::EngineDriver::fill_entropy); a fixed seed
//! gives a deterministic identity for tests and early spikes. Persistence is a
//! later, separate concern.

use crate::crypto::{
    ed25519_public_key, ed25519_sign, sha256, x25519_diffie_hellman, x25519_public_key,
    Ed25519PublicKey, Ed25519SecretKey, Ed25519Signature, X25519PublicKey, X25519SecretKey,
    X25519SharedSecret,
};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

/// Bytes of entropy [`Identity::from_entropy`] consumes: a 32-byte X25519
/// (encryption) secret followed by a 32-byte Ed25519 (signing) secret —
/// matching RNS's private-key byte layout (`prv_bytes ‖ sig_prv_bytes`).
pub const IDENTITY_SEED_LEN: usize = 64;

/// The truncated SHA-256 of an identity's two public keys
/// (`sha256(encryption_pub ‖ signing_pub)[..16]`) — how the network names a
/// node. Distinct from a [`DestinationHash`](crate::wire::DestinationHash),
/// which names a *destination* (an identity bound to an app name / app aspect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityHash([u8; TRUNCATED_HASH_BYTE_LEN]);

impl IdentityHash {
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }
}

/// A node's own identity: the encryption + signing keypairs, the derived public
/// keys, and the identity hash. Holds the secret keys, so it is neither `Clone`
/// nor `Copy` and never logged.
pub struct Identity {
    encryption_secret: X25519SecretKey,
    signing_secret: Ed25519SecretKey,
    encryption_public: X25519PublicKey,
    signing_public: Ed25519PublicKey,
    hash: IdentityHash,
}

impl Identity {
    /// Build an identity from [`IDENTITY_SEED_LEN`] bytes of key material: the
    /// first 32 become the X25519 (encryption) secret, the next 32 the Ed25519
    /// (signing) secret. Feed CSPRNG bytes for an ephemeral per-run identity - or
    /// a fixed array for a deterministic test/spike one - the math is identical.
    pub fn from_entropy(seed: &[u8; IDENTITY_SEED_LEN]) -> Self {
        let mut encryption_seed = [0u8; 32];
        encryption_seed.copy_from_slice(&seed[..32]);
        let mut signing_seed = [0u8; 32];
        signing_seed.copy_from_slice(&seed[32..]);

        let encryption_secret = X25519SecretKey::new(encryption_seed);
        let signing_secret = Ed25519SecretKey::new(signing_seed);
        let encryption_public = x25519_public_key(&encryption_secret);
        let signing_public = ed25519_public_key(&signing_secret);
        let hash = derive_identity_hash(&encryption_public, &signing_public);

        Self {
            encryption_secret,
            signing_secret,
            encryption_public,
            signing_public,
            hash,
        }
    }

    pub const fn hash(&self) -> IdentityHash {
        self.hash
    }

    pub const fn encryption_public_key(&self) -> X25519PublicKey {
        self.encryption_public
    }

    pub const fn signing_public_key(&self) -> Ed25519PublicKey {
        self.signing_public
    }

    /// Sign `message` with this identity's Ed25519 secret (deterministic, RFC 8032)
    pub fn sign(&self, message: &[u8]) -> Ed25519Signature {
        ed25519_sign(&self.signing_secret, message)
    }

    /// The X25519 shared secret between this identity's encryption secret and a
    /// peer's encryption public key (RFC 7748 clamping).
    pub fn agree(&self, peer_encryption_public: &X25519PublicKey) -> X25519SharedSecret {
        x25519_diffie_hellman(&self.encryption_secret, peer_encryption_public)
    }
}

/// `sha256(encryption_pub ‖ signing_pub)` truncated to the standard address
/// length — RNS's `Identity.truncated_hash(get_public_key())`.
fn derive_identity_hash(
    encryption_public: &X25519PublicKey,
    signing_public: &Ed25519PublicKey,
) -> IdentityHash {
    let mut material = [0u8; 64];
    material[..32].copy_from_slice(&encryption_public.0);
    material[32..].copy_from_slice(&signing_public.0);

    let full = sha256(&material);
    let mut truncated = [0u8; TRUNCATED_HASH_BYTE_LEN];
    truncated.copy_from_slice(&full[..TRUNCATED_HASH_BYTE_LEN]);
    IdentityHash(truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed key material matching the seeds the `crypto` vectors already pin
    /// (X25519 secret `[0x22; 32]`, Ed25519 secret `[0x11; 32]`), so the derived
    /// public keys and identity hash can be checked against RNS 1.3.1.
    fn fixed_seed() -> [u8; IDENTITY_SEED_LEN] {
        let mut seed = [0u8; IDENTITY_SEED_LEN];
        seed[..32].fill(0x22);
        seed[32..].fill(0x11);
        seed
    }

    fn hx<const N: usize>(s: &str) -> [u8; N] {
        let mut out = [0u8; N];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("valid hex");
        }
        out
    }

    #[test]
    fn from_entropy_derives_rns_public_keys_and_hash() {
        let identity = Identity::from_entropy(&fixed_seed());

        // Public keys match the RNS 1.3.1 vectors (same seeds as crypto tests).
        assert_eq!(
            identity.encryption_public_key().0,
            hx::<32>("0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20"),
        );
        assert_eq!(
            identity.signing_public_key().0,
            hx::<32>("d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737"),
        );

        // Identity hash = sha256(enc_pub ‖ sig_pub)[..16], oracle-confirmed.
        assert_eq!(
            identity.hash(),
            IdentityHash::new(hx::<16>("4cd0cc45a7405dbd5cf9b5be1ef92f10")),
        );
    }

    #[test]
    fn same_seed_is_deterministic() {
        let a = Identity::from_entropy(&fixed_seed());
        let b = Identity::from_entropy(&fixed_seed());
        assert_eq!(a.hash(), b.hash());
    }

    #[test]
    fn signatures_verify_under_the_identitys_public_key() {
        use crate::crypto::ed25519_verify;

        let identity = Identity::from_entropy(&fixed_seed());
        let message = b"announce-ourselves";
        let signature = identity.sign(message);

        assert!(ed25519_verify(&identity.signing_public_key(), message, &signature).is_ok());
        assert!(ed25519_verify(&identity.signing_public_key(), b"tampered", &signature).is_err());
    }

    #[test]
    fn key_agreement_matches_the_rns_shared_secret() {
        // Same X25519 vector the crypto tests pin: our encryption secret
        // `[0x22; 32]` agreed with the peer public below yields this shared
        // secret per RNS 1.3.1.
        let identity = Identity::from_entropy(&fixed_seed());
        let peer = X25519PublicKey(hx::<32>(
            "7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14",
        ));
        assert_eq!(
            identity.agree(&peer).as_bytes(),
            &hx::<32>("1fdc192faa0212a9aae7bb4f41b580227fd5ad3e5d777faae230dfe973f3e805"),
        );
    }

    #[test]
    fn distinct_seeds_yield_distinct_identities() {
        let a = Identity::from_entropy(&fixed_seed());
        let b = Identity::from_entropy(&[0x05; IDENTITY_SEED_LEN]);
        assert_ne!(a.hash(), b.hash());
    }
}
