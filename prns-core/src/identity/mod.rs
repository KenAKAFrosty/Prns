//! Nothing here generates randomness: the 64 bytes ARE the two private keys, used
//! verbatim (no stretching) — their quality is the key's quality.

pub mod held;
pub mod vault;

use crate::crypto::{
    hkdf_sha256, sha256, token_is_authentic, token_open, token_open_in_place, token_seal,
    x25519_diffie_hellman, x25519_seal_scalars, CryptoError, Ed25519PublicKey, Ed25519Signature,
    TokenKey, X25519PublicKey, X25519SecretKey, X25519SharedSecret,
};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

pub use zeroize::Zeroizing;

/// A 32-byte X25519 secret ‖ a 32-byte Ed25519 secret — RNS's persisted layout
/// (`prv_bytes ‖ sig_prv_bytes`); these bytes *are* the keys.
pub const IDENTITY_SECRET_KEY_LEN: usize = 64;

/// The public mirror: a 32-byte X25519 encryption key ‖ a 32-byte Ed25519 signing key, RNS's `Identity.get_public_key()` layout.
pub const IDENTITY_PUBLIC_KEY_LEN: usize = 64;

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

/// Deliberately the *operation* surface, not the *secret* one — no accessor for
/// either private key; the same seam later admits an enclave-backed signer.
pub trait IdentitySigner {
    fn encryption_public_key(&self) -> IdentityEncryptionPublicKey;
    fn signing_public_key(&self) -> IdentitySigningPublicKey;

    fn identity_hash(&self) -> IdentityHash {
        derive_identity_hash(&self.encryption_public_key(), &self.signing_public_key())
    }

    /// The wire form RNS calls `Identity.get_public_key()`: encryption key ‖ signing key.
    fn public_key_bytes(&self) -> [u8; IDENTITY_PUBLIC_KEY_LEN] {
        let mut bytes = [0u8; IDENTITY_PUBLIC_KEY_LEN];
        bytes[..32].copy_from_slice(self.encryption_public_key().as_bytes());
        bytes[32..].copy_from_slice(self.signing_public_key().as_bytes());
        bytes
    }

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

/// RNS 1.3.5 `Identity.DERIVED_KEY_LENGTH`: packet keys are 64 bytes, selecting
/// the token's AES-256 mode.
const DERIVED_PACKET_KEY_LEN: usize = 64;

pub const ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN: usize = 32;

pub const ENCRYPTION_IV_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptError {
    BufferTooShort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecryptError {
    TokenTooShort,
    InvalidToken,
    BufferTooShort,
}

struct DerivedPacketKey(Zeroizing<[u8; DERIVED_PACKET_KEY_LEN]>);

impl DerivedPacketKey {
    fn derive(shared: &X25519SharedSecret, recipient_identity_hash: &IdentityHash) -> Self {
        Self(Zeroizing::new(hkdf_sha256::<DERIVED_PACKET_KEY_LEN>(
            shared.as_bytes(),
            recipient_identity_hash.as_bytes(),
            &[],
        )))
    }

    #[allow(clippy::expect_used)]
    fn token_key(&self) -> TokenKey<'_> {
        TokenKey::from_derived(self.0.as_slice())
            .expect("a 64-byte derived key is always a valid token key")
    }
}

fn decrypt_token_in_place<'t>(
    encryption_secret: &X25519SecretKey,
    recipient_identity_hash: &IdentityHash,
    ciphertext_token: &'t mut [u8],
) -> Result<&'t [u8], DecryptError> {
    decrypt_token_in_place_with_ratchets(
        &[],
        encryption_secret,
        recipient_identity_hash,
        ciphertext_token,
    )
}

/// RNS 1.3.5 `Identity.decrypt(ciphertext, ratchets=…)`: ratchets newest-first, then
/// the identity key. The HKDF salt stays the *identity* hash even when a ratchet did
/// the exchange (reference `get_salt` is `self.hash` unconditionally); candidates are
/// probed by MAC so the buffer decrypts in place exactly once.
pub fn decrypt_token_in_place_with_ratchets<'t>(
    ratchet_secrets: &[X25519SecretKey],
    encryption_secret: &X25519SecretKey,
    recipient_identity_hash: &IdentityHash,
    ciphertext_token: &'t mut [u8],
) -> Result<&'t [u8], DecryptError> {
    if ciphertext_token.len() <= ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN {
        return Err(DecryptError::TokenTooShort);
    }
    let (ephemeral, token) = ciphertext_token.split_at_mut(ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN);
    let mut ephemeral_public_bytes = [0u8; ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN];
    ephemeral_public_bytes.copy_from_slice(ephemeral);
    let ephemeral_public = X25519PublicKey(ephemeral_public_bytes);

    let derive_for = |secret: &X25519SecretKey| {
        let shared = x25519_diffie_hellman(secret, &ephemeral_public);
        DerivedPacketKey::derive(&shared, recipient_identity_hash)
    };
    let key = ratchet_secrets
        .iter()
        .map(derive_for)
        .find(|candidate| token_is_authentic(&candidate.token_key(), token))
        .unwrap_or_else(|| derive_for(encryption_secret));

    token_open_in_place(&key.token_key(), token).map_err(|error| match error {
        CryptoError::InvalidSignature
        | CryptoError::InvalidMac
        | CryptoError::InvalidPadding
        | CryptoError::MalformedToken
        | CryptoError::BadKeyLength
        | CryptoError::BufferTooShort => DecryptError::InvalidToken,
    })
}

pub(crate) fn decrypt_finish_in_place<'t>(
    shared: &X25519SharedSecret,
    recipient_identity_hash: &IdentityHash,
    token: &'t mut [u8],
) -> Result<&'t [u8], DecryptError> {
    let key = DerivedPacketKey::derive(shared, recipient_identity_hash);
    token_open_in_place(&key.token_key(), token).map_err(|error| match error {
        CryptoError::InvalidSignature
        | CryptoError::InvalidMac
        | CryptoError::InvalidPadding
        | CryptoError::MalformedToken
        | CryptoError::BadKeyLength
        | CryptoError::BufferTooShort => DecryptError::InvalidToken,
    })
}

fn decrypt_token(
    encryption_secret: &X25519SecretKey,
    recipient_identity_hash: &IdentityHash,
    ciphertext_token: &[u8],
    out: &mut [u8],
) -> Result<usize, DecryptError> {
    if ciphertext_token.len() <= ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN {
        return Err(DecryptError::TokenTooShort);
    }
    let mut ephemeral_public_bytes = [0u8; ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN];
    ephemeral_public_bytes
        .copy_from_slice(&ciphertext_token[..ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN]);
    let ephemeral_public = X25519PublicKey(ephemeral_public_bytes);

    let shared = x25519_diffie_hellman(encryption_secret, &ephemeral_public);
    let key = DerivedPacketKey::derive(&shared, recipient_identity_hash);

    token_open(
        &key.token_key(),
        &ciphertext_token[ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN..],
        out,
    )
    .map_err(|error| match error {
        CryptoError::BufferTooShort => DecryptError::BufferTooShort,
        CryptoError::InvalidSignature
        | CryptoError::InvalidMac
        | CryptoError::InvalidPadding
        | CryptoError::MalformedToken
        | CryptoError::BadKeyLength => DecryptError::InvalidToken,
    })
}

/// The encrypting side of RNS 1.3.5 `Identity.encrypt` — no private material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteIdentity {
    encryption_public: IdentityEncryptionPublicKey,
    hash: IdentityHash,
}

impl RemoteIdentity {
    pub fn from_public_keys(
        encryption_public: IdentityEncryptionPublicKey,
        signing_public: IdentitySigningPublicKey,
    ) -> Self {
        Self {
            encryption_public,
            hash: derive_identity_hash(&encryption_public, &signing_public),
        }
    }

    pub const fn encryption_public_key(&self) -> IdentityEncryptionPublicKey {
        self.encryption_public
    }

    pub const fn identity_hash(&self) -> IdentityHash {
        self.hash
    }

    pub fn encrypt(
        &self,
        ephemeral_secret: &X25519SecretKey,
        iv: &[u8; ENCRYPTION_IV_LEN],
        plaintext: &[u8],
        out: &mut [u8],
    ) -> Result<usize, EncryptError> {
        self.seal_toward(
            self.encryption_public.as_x25519(),
            ephemeral_secret,
            iv,
            plaintext,
            out,
        )
    }

    /// Seal toward the peer's announced ratchet instead of its identity key —
    /// RNS 1.3.5 `Identity.encrypt(ratchet=…)`: only the Diffie-Hellman target
    /// changes; the HKDF salt stays the identity hash.
    pub fn encrypt_to_ratchet(
        &self,
        ratchet_public: &X25519PublicKey,
        ephemeral_secret: &X25519SecretKey,
        iv: &[u8; ENCRYPTION_IV_LEN],
        plaintext: &[u8],
        out: &mut [u8],
    ) -> Result<usize, EncryptError> {
        self.seal_toward(ratchet_public, ephemeral_secret, iv, plaintext, out)
    }

    fn seal_toward(
        &self,
        dh_target: &X25519PublicKey,
        ephemeral_secret: &X25519SecretKey,
        iv: &[u8; ENCRYPTION_IV_LEN],
        plaintext: &[u8],
        out: &mut [u8],
    ) -> Result<usize, EncryptError> {
        let (ephemeral_public, shared) = x25519_seal_scalars(ephemeral_secret, dh_target);
        seal_finish(&self.hash, &ephemeral_public, &shared, iv, plaintext, out)
    }
}

/// The inline and pooled paths both finish through here, so the seal stays
/// byte-identical either way.
pub(crate) fn seal_finish(
    recipient_identity_hash: &IdentityHash,
    ephemeral_public: &X25519PublicKey,
    shared: &X25519SharedSecret,
    iv: &[u8; ENCRYPTION_IV_LEN],
    plaintext: &[u8],
    out: &mut [u8],
) -> Result<usize, EncryptError> {
    if out.len() < ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN {
        return Err(EncryptError::BufferTooShort);
    }
    let key = DerivedPacketKey::derive(shared, recipient_identity_hash);
    out[..ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN].copy_from_slice(&ephemeral_public.0);
    let sealed = token_seal(
        &key.token_key(),
        iv,
        plaintext,
        &mut out[ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN..],
    )
    .map_err(|_| EncryptError::BufferTooShort)?;
    Ok(ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN + sealed)
}

pub mod in_memory {
    use super::{
        derive_identity_hash, DecryptError, IdentityEncryptionPublicKey, IdentityHash,
        IdentitySigner, IdentitySigningPublicKey, IDENTITY_SECRET_KEY_LEN,
    };
    use crate::crypto::{
        ed25519_public_key, ed25519_sign, x25519_diffie_hellman, x25519_public_key,
        Ed25519SecretKey, Ed25519Signature, X25519PublicKey, X25519SecretKey, X25519SharedSecret,
    };

    pub struct InMemoryNodeIdentity {
        encryption_secret: X25519SecretKey,
        signing_secret: Ed25519SecretKey,
        encryption_public: IdentityEncryptionPublicKey,
        signing_public: IdentitySigningPublicKey,
        hash: IdentityHash,
    }

    pub struct IdentityParts {
        pub encryption_secret: X25519SecretKey,
        pub signing_secret: Ed25519SecretKey,
        pub encryption_public: IdentityEncryptionPublicKey,
        pub signing_public: IdentitySigningPublicKey,
        pub hash: IdentityHash,
    }

    impl InMemoryNodeIdentity {
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

        pub fn agree(&self, peer_encryption_public: &X25519PublicKey) -> X25519SharedSecret {
            x25519_diffie_hellman(&self.encryption_secret, peer_encryption_public)
        }

        pub fn decrypt_in_place<'t>(
            &self,
            ciphertext_token: &'t mut [u8],
        ) -> Result<&'t [u8], DecryptError> {
            super::decrypt_token_in_place(&self.encryption_secret, &self.hash, ciphertext_token)
        }

        pub fn decrypt(
            &self,
            ciphertext_token: &[u8],
            out: &mut [u8],
        ) -> Result<usize, DecryptError> {
            super::decrypt_token(&self.encryption_secret, &self.hash, ciphertext_token, out)
        }

        pub fn into_parts(self) -> IdentityParts {
            let Self {
                encryption_secret,
                signing_secret,
                encryption_public,
                signing_public,
                hash,
            } = self;
            IdentityParts {
                encryption_secret,
                signing_secret,
                encryption_public,
                signing_public,
                hash,
            }
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
        use super::super::ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN;
        use super::*;

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

        const RNS_SEALED_TOKEN: &str =
            "81359acd4801e770d203b5b8f8500cd30830045a31616ff3167cac747c3c9072\
             93c11f98685deef2b25d3f6514d10a3c17c1bb903f6531e5499ef38dd7536fad\
             65701dad8b651b60ed993be65e1433a7f49cdb641b314b1ebeac3930058deea3";

        const RUST_SEALED_TOKEN: &str =
            "7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14\
             44444444444444444444444444444444f3fc0f35b7a182440fd9efc1ed35ae58\
             99108742c09abbdf0d496ff0e0461e0b9959bc4e968a39f8934dc9b071066050";

        fn token_hex(s: &str) -> std::vec::Vec<u8> {
            let cleaned: std::string::String = s.split_whitespace().collect();
            (0..cleaned.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16).expect("valid hex"))
                .collect()
        }

        fn remote_for(identity: &InMemoryNodeIdentity) -> super::super::RemoteIdentity {
            super::super::RemoteIdentity::from_public_keys(
                identity.encryption_public_key(),
                identity.signing_public_key(),
            )
        }

        #[test]
        fn decrypt_in_place_opens_the_rns_1_3_5_token_without_a_copy() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let mut token = token_hex(RNS_SEALED_TOKEN);
            let plaintext = identity.decrypt_in_place(&mut token).unwrap();
            assert_eq!(plaintext, b"hello-single");
        }

        #[test]
        fn decrypt_in_place_rejects_tampering_and_short_tokens() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());

            let mut tampered = token_hex(RNS_SEALED_TOKEN);
            tampered[50] ^= 0x01;
            assert_eq!(
                identity.decrypt_in_place(&mut tampered),
                Err(DecryptError::InvalidToken),
            );

            let mut short = [0xAA; ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN];
            assert_eq!(
                identity.decrypt_in_place(&mut short),
                Err(DecryptError::TokenTooShort),
            );
        }

        #[test]
        fn decrypt_opens_a_token_sealed_by_rns_1_3_5() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let token = token_hex(RNS_SEALED_TOKEN);
            let mut out = [0u8; 64];
            let n = identity.decrypt(&token, &mut out).unwrap();
            assert_eq!(&out[..n], b"hello-single");
        }

        #[test]
        fn encrypt_seals_the_token_rns_1_3_5_opens() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let mut out = [0u8; 128];
            let n = remote_for(&identity)
                .encrypt(
                    &X25519SecretKey::new([0x33; 32]),
                    &[0x44; 16],
                    b"hello-single",
                    &mut out,
                )
                .unwrap();
            assert_eq!(out[..n].to_vec(), token_hex(RUST_SEALED_TOKEN));
        }

        const RATCHET_SEALED_TOKEN: &str =
            "7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14\
             44444444444444444444444444444444f0c0d10df07782f3a9a89a271b84960b\
             c9d2525bfcfd385954b4ebda6c6702dd9b82ca630f3b45c1c57457ad70aa14e6";

        #[test]
        fn encrypt_to_ratchet_reproduces_the_rns_1_3_5_minted_token() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let ratchet_public = x25519_public_key(&X25519SecretKey::new([0x55; 32]));
            let mut out = [0u8; 128];
            let n = remote_for(&identity)
                .encrypt_to_ratchet(
                    &ratchet_public,
                    &X25519SecretKey::new([0x33; 32]),
                    &[0x44; 16],
                    b"ratchet-parity",
                    &mut out,
                )
                .unwrap();
            assert_eq!(out[..n].to_vec(), token_hex(RATCHET_SEALED_TOKEN));
        }

        #[test]
        fn a_ratchet_sealed_token_does_not_open_with_the_identity_key_alone() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let mut token = token_hex(RATCHET_SEALED_TOKEN);
            assert_eq!(
                identity.decrypt_in_place(&mut token),
                Err(DecryptError::InvalidToken),
            );
        }

        #[test]
        fn encrypt_round_trips_through_decrypt() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let plaintext = [0xC7; 211];
            let mut sealed = [0u8; 512];
            let n = remote_for(&identity)
                .encrypt(
                    &X25519SecretKey::new([0x55; 32]),
                    &[0x66; 16],
                    &plaintext,
                    &mut sealed,
                )
                .unwrap();

            let mut opened = [0u8; 512];
            let opened_len = identity.decrypt(&sealed[..n], &mut opened).unwrap();
            assert_eq!(&opened[..opened_len], &plaintext);
        }

        #[test]
        fn a_tampered_token_is_rejected() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let mut out = [0u8; 64];

            let mut tampered_ciphertext = token_hex(RNS_SEALED_TOKEN);
            tampered_ciphertext[50] ^= 0x01;
            assert_eq!(
                identity.decrypt(&tampered_ciphertext, &mut out),
                Err(DecryptError::InvalidToken),
            );

            let mut tampered_mac = token_hex(RNS_SEALED_TOKEN);
            let last = tampered_mac.len() - 1;
            tampered_mac[last] ^= 0x01;
            assert_eq!(
                identity.decrypt(&tampered_mac, &mut out),
                Err(DecryptError::InvalidToken),
            );
        }

        #[test]
        fn a_token_no_longer_than_the_ephemeral_key_is_rejected() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let mut out = [0u8; 64];
            assert_eq!(
                identity.decrypt(&[0xAA; ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN], &mut out),
                Err(DecryptError::TokenTooShort),
            );
        }

        #[test]
        fn another_identity_cannot_open_the_token() {
            let other =
                InMemoryNodeIdentity::from_secret_key_bytes(&[0x05; IDENTITY_SECRET_KEY_LEN]);
            let token = token_hex(RNS_SEALED_TOKEN);
            let mut out = [0u8; 64];
            assert_eq!(
                other.decrypt(&token, &mut out),
                Err(DecryptError::InvalidToken),
            );
        }

        #[test]
        fn undersized_buffers_are_reported_not_panicked() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());
            let token = token_hex(RNS_SEALED_TOKEN);
            let mut tiny = [0u8; 4];
            assert_eq!(
                identity.decrypt(&token, &mut tiny),
                Err(DecryptError::BufferTooShort),
            );

            let mut tiny_out = [0u8; 16];
            assert_eq!(
                remote_for(&identity).encrypt(
                    &X25519SecretKey::new([0x33; 32]),
                    &[0x44; 16],
                    b"hello-single",
                    &mut tiny_out,
                ),
                Err(super::super::EncryptError::BufferTooShort),
            );
        }

        #[test]
        fn from_secret_key_bytes_derives_rns_public_keys_and_hash() {
            let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key_bytes());

            assert_eq!(
                identity.encryption_public_key().as_bytes(),
                &hx::<32>("0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20"),
            );
            assert_eq!(
                identity.signing_public_key().as_bytes(),
                &hx::<32>("d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737"),
            );

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
