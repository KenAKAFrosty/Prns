//! A node's identity. [`IdentitySigner`] is the capability the rest of the crate
//! signs and addresses through: the two public keys (which name the node on the
//! wire), the derived [`IdentityHash`], and the Ed25519 signing operation —
//! deliberately the *operation* surface, never the secret keys. The concrete
//! secret-holding implementation lives in [`in_memory`] and is built transiently
//! from key material, so reach for [`IdentitySigner`] as the canonical surface and
//! treat [`in_memory::InMemoryNodeIdentity`] as one implementation of it. This
//! module currently holds only *our own* identity (we hold the secrets);
//! recall/remember and the storage of *other* nodes' identities will land here
//! too as that work arrives — mirroring RNS's `Identity` module.
//!
//! **Sans-storage, by construction.** Per `crypto/`'s invariant, nothing here
//! generates randomness — [`in_memory::InMemoryNodeIdentity::from_secret_key_bytes`]
//! takes the secret keys *themselves* as input: the 64 bytes ARE the two private
//! keys, used verbatim (no stretching), so their quality is the key's quality. An
//! ephemeral per-run identity is just fresh CSPRNG bytes; a persisted one is those
//! same bytes saved and reloaded (the RNS on-disk layout); a fixed array gives a
//! deterministic test/spike identity. Persistence is a later, separate concern.

use crate::crypto::{sha256, Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

/// Re-exported so callers handing secret key bytes to the engine's
/// identity-bearing constructors can build the expected zeroizing buffer
/// without taking their own (version-coupled) `zeroize` dependency.
pub use zeroize::Zeroizing;

/// Length of an identity's secret key material: a 32-byte X25519 (encryption)
/// secret followed by a 32-byte Ed25519 (signing) secret — the two private keys
/// concatenated, matching RNS's persisted layout (`prv_bytes ‖ sig_prv_bytes`).
/// These bytes *are* the keys; persist them to persist the identity.
pub const IDENTITY_SECRET_KEY_LEN: usize = 64;

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

/// An identity's static X25519 public key, in its *encryption* role. A bare
/// [`X25519PublicKey`] is used for other roles too — notably a forward-secrecy
/// ratchet key — so wrapping it keeps the compiler from letting one stand in for
/// the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityEncryptionPublicKey(X25519PublicKey);

impl IdentityEncryptionPublicKey {
    pub const fn new(key: X25519PublicKey) -> Self {
        Self(key)
    }

    pub const fn as_x25519(&self) -> &X25519PublicKey {
        &self.0
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0 .0
    }
}

/// An identity's Ed25519 public key, in its *signing* role — distinct from any
/// other Ed25519 key the type system might otherwise let through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentitySigningPublicKey(Ed25519PublicKey);

impl IdentitySigningPublicKey {
    pub const fn new(key: Ed25519PublicKey) -> Self {
        Self(key)
    }

    pub const fn as_ed25519(&self) -> &Ed25519PublicKey {
        &self.0
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0 .0
    }
}

/// The capability the engine needs to author signed material on a node's behalf:
/// the two public keys (which travel on the wire and name the node), the derived
/// [`IdentityHash`], and the Ed25519 signing operation. This is deliberately the
/// *operation* surface, not the *secret* one — there is no accessor for either
/// private key, so a signer can expose exactly this without exposing key
/// material. Today the only implementor is [`in_memory::InMemoryNodeIdentity`]
/// (we hold the secrets); a trait because the same seam later admits an
/// enclave-backed external signer that signs without the key ever leaving.
/// Packet key-agreement (`agree`) is a separate capability that joins when
/// encryption lands — the announce path never needs it.
pub trait IdentitySigner {
    fn encryption_public_key(&self) -> IdentityEncryptionPublicKey;
    fn signing_public_key(&self) -> IdentitySigningPublicKey;

    /// `sha256(encryption_pub ‖ signing_pub)[..16]`. Defaulted from the two
    /// public keys, so an implementor's hash is always consistent with the keys
    /// it reports; an implementor holding a precomputed hash may override.
    fn identity_hash(&self) -> IdentityHash {
        derive_identity_hash(&self.encryption_public_key(), &self.signing_public_key())
    }

    /// Sign `message` with the node's Ed25519 secret (deterministic, RFC 8032).
    fn sign(&self, message: &[u8]) -> Ed25519Signature;
}

fn derive_identity_hash(
    encryption_public: &IdentityEncryptionPublicKey,
    signing_public: &IdentitySigningPublicKey,
) -> IdentityHash {
    let mut material = [0u8; 64];
    material[..32].copy_from_slice(encryption_public.as_bytes());
    material[32..].copy_from_slice(signing_public.as_bytes());

    let full = sha256(&material);
    let mut truncated = [0u8; TRUNCATED_HASH_BYTE_LEN];
    truncated.copy_from_slice(&full[..TRUNCATED_HASH_BYTE_LEN]);
    IdentityHash(truncated)
}

pub mod in_memory {
    //! The in-memory implementation of [`IdentitySigner`](super::IdentitySigner): the
    //! one that actually holds the secret keys. Built transiently from key
    //! material (host-custodied bytes, or a fixed seed for tests/spikes), it signs
    //! and agrees with the secrets in RAM. Reach for the
    //! [`IdentitySigner`](super::IdentitySigner) capability everywhere else; this is
    //! just where the secret lives when *we* are the signer, not "the" identity.

    use super::{
        derive_identity_hash, IdentityEncryptionPublicKey, IdentityHash, IdentitySigner,
        IdentitySigningPublicKey, IDENTITY_SECRET_KEY_LEN,
    };
    use crate::crypto::{
        ed25519_public_key, ed25519_sign, x25519_diffie_hellman, x25519_public_key,
        Ed25519SecretKey, Ed25519Signature, X25519PublicKey, X25519SecretKey, X25519SharedSecret,
    };

    /// A node identity whose secret keys live in memory: the X25519 (encryption)
    /// and Ed25519 (signing) keypairs, the derived public keys, and the identity
    /// hash. Holds the secret keys, so it is neither `Clone` nor `Copy` and never
    /// logged.
    pub struct InMemoryNodeIdentity {
        encryption_secret: X25519SecretKey,
        signing_secret: Ed25519SecretKey,
        encryption_public: IdentityEncryptionPublicKey,
        signing_public: IdentitySigningPublicKey,
        hash: IdentityHash,
    }

    impl InMemoryNodeIdentity {
        /// Build an identity from its [`IDENTITY_SECRET_KEY_LEN`](super::IDENTITY_SECRET_KEY_LEN)
        /// bytes of secret key material: the first 32 *are* the X25519 (encryption)
        /// private key, the next 32 *are* the Ed25519 (signing) private key — used
        /// verbatim, not stretched, so the bytes' quality is the keys' quality.
        /// CSPRNG bytes give a fresh identity; the same bytes reloaded from storage
        /// give the persisted one; a fixed array gives a deterministic test/spike.
        pub fn from_secret_key_bytes(bytes: &[u8; IDENTITY_SECRET_KEY_LEN]) -> Self {
            let mut encryption_secret_bytes = [0u8; 32];
            encryption_secret_bytes.copy_from_slice(&bytes[..32]);
            let mut signing_secret_bytes = [0u8; 32];
            signing_secret_bytes.copy_from_slice(&bytes[32..]);

            let encryption_secret = X25519SecretKey::new(encryption_secret_bytes);
            let signing_secret = Ed25519SecretKey::new(signing_secret_bytes);
            let encryption_public =
                IdentityEncryptionPublicKey::new(x25519_public_key(&encryption_secret));
            let signing_public = IdentitySigningPublicKey::new(ed25519_public_key(&signing_secret));
            let hash = derive_identity_hash(&encryption_public, &signing_public);

            Self {
                encryption_secret,
                signing_secret,
                encryption_public,
                signing_public,
                hash,
            }
        }

        /// The X25519 shared secret between this identity's encryption secret and
        /// a peer's encryption public key (RFC 7748 clamping).
        pub fn agree(&self, peer_encryption_public: &X25519PublicKey) -> X25519SharedSecret {
            x25519_diffie_hellman(&self.encryption_secret, peer_encryption_public)
        }
    }

    impl IdentitySigner for InMemoryNodeIdentity {
        fn encryption_public_key(&self) -> IdentityEncryptionPublicKey {
            self.encryption_public
        }

        fn signing_public_key(&self) -> IdentitySigningPublicKey {
            self.signing_public
        }

        fn identity_hash(&self) -> IdentityHash {
            self.hash
        }

        fn sign(&self, message: &[u8]) -> Ed25519Signature {
            ed25519_sign(&self.signing_secret, message)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Fixed key material matching the seeds the `crypto` vectors already pin
        /// (X25519 secret `[0x22; 32]`, Ed25519 secret `[0x11; 32]`), so the
        /// derived public keys and identity hash can be checked against RNS 1.3.1.
        fn fixed_secret_key_bytes() -> [u8; IDENTITY_SECRET_KEY_LEN] {
            let mut bytes = [0u8; IDENTITY_SECRET_KEY_LEN];
            bytes[..32].fill(0x22);
            bytes[32..].fill(0x11);
            bytes
        }

        fn hx<const N: usize>(s: &str) -> [u8; N] {
            let mut out = [0u8; N];
            for (i, byte) in out.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("valid hex");
            }
            out
        }

        #[test]
        fn from_secret_key_bytes_derives_rns_public_keys_and_hash() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());

            // Public keys match the RNS 1.3.1 vectors (same seeds as crypto tests).
            assert_eq!(
                identity.encryption_public_key().as_bytes(),
                &hx::<32>("0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20"),
            );
            assert_eq!(
                identity.signing_public_key().as_bytes(),
                &hx::<32>("d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737"),
            );

            // Identity hash = sha256(enc_pub ‖ sig_pub)[..16], oracle-confirmed.
            assert_eq!(
                identity.identity_hash(),
                IdentityHash::new(hx::<16>("4cd0cc45a7405dbd5cf9b5be1ef92f10")),
            );
        }

        #[test]
        fn same_seed_is_deterministic() {
            let a = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let b = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            assert_eq!(a.identity_hash(), b.identity_hash());
        }

        #[test]
        fn signatures_verify_under_the_identitys_public_key() {
            use crate::crypto::ed25519_verify;

            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let message = b"announce-ourselves";
            let signature = identity.sign(message);

            assert!(ed25519_verify(
                identity.signing_public_key().as_ed25519(),
                message,
                &signature
            )
            .is_ok());
            assert!(ed25519_verify(
                identity.signing_public_key().as_ed25519(),
                b"tampered",
                &signature
            )
            .is_err());
        }

        #[test]
        fn key_agreement_matches_the_rns_shared_secret() {
            // Same X25519 vector the crypto tests pin: our encryption secret
            // `[0x22; 32]` agreed with the peer public below yields this shared
            // secret per RNS 1.3.1.
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
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
            let a = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let b = InMemoryNodeIdentity::from_secret_key_bytes(&[0x05; IDENTITY_SECRET_KEY_LEN]);
            assert_ne!(a.identity_hash(), b.identity_hash());
        }
    }
}
