//! Nothing here generates randomness: the 64 bytes ARE the two private keys, used verbatim (no stretching). Their quality is the key's quality.

pub mod destination_identity;
pub mod held;
mod material;
pub mod vault;

pub use destination_identity::{
    DestinationIdentityRetentionState, MarkDestinationUsedOutcome, ReleaseDestinationOutcome,
    RetainDestinationOutcome, RetainIdentityOutcome,
};
pub use material::{IdentityMaterialLengthError, PrivateIdentityMaterial, PublicIdentityMaterial};

use crate::crypto::ratchets::RatchetId;
use crate::crypto::{
    hkdf_sha256, sha256, token_is_authentic, token_open, token_open_in_place, token_seal,
    x25519_diffie_hellman, x25519_keys_for_seal, Ed25519PublicKey, Ed25519SecretKey,
    Ed25519Signature, TokenKey, TokenOpenError, X25519PublicKey, X25519SecretKey,
    X25519SharedSecret,
};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

pub use zeroize::Zeroizing;

/// An X25519 secret ‖ an Ed25519 secret. RNS's persisted layout (`prv_bytes ‖ sig_prv_bytes`); these bytes *are* the keys.
pub const IDENTITY_SECRET_KEY_LEN: usize = X25519SecretKey::LEN + Ed25519SecretKey::LEN;

/// The public mirror: an X25519 encryption key ‖ an Ed25519 signing key, RNS's `Identity.get_public_key()` layout.
pub const IDENTITY_PUBLIC_KEY_LEN: usize = X25519PublicKey::LEN + Ed25519PublicKey::LEN;

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

/// X25519 public keys are used outside of identity work as well. This newtype is used as a brand to avoid accidental crossing.
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

/// Ed25519 public keys are used outside of identity work as well. This newtype is used as a brand to avoid accidental crossing.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityPublicKeys {
    pub encryption: IdentityEncryptionPublicKey,
    pub signing: IdentitySigningPublicKey,
}

impl IdentityPublicKeys {
    pub fn identity_hash(&self) -> IdentityHash {
        derive_identity_hash(&self.encryption, &self.signing)
    }
}

/// Deliberately the *operation* surface, not the *secret* one. No accessor for either private key.
pub trait IdentitySigner {
    fn encryption_public_key(&self) -> IdentityEncryptionPublicKey;
    fn signing_public_key(&self) -> IdentitySigningPublicKey;

    fn identity_hash(&self) -> IdentityHash {
        derive_identity_hash(&self.encryption_public_key(), &self.signing_public_key())
    }

    /// The wire form RNS calls `Identity.get_public_key()`: encryption key ‖ signing key.
    fn public_key_bytes(&self) -> [u8; IDENTITY_PUBLIC_KEY_LEN] {
        concat_public_keys(&self.encryption_public_key(), &self.signing_public_key())
    }

    fn sign(&self, message: &[u8]) -> Ed25519Signature;
}

/// The one place the `Identity.get_public_key()` layout is spelled: encryption key ‖ signing key.
fn concat_public_keys(
    encryption_public: &IdentityEncryptionPublicKey,
    signing_public: &IdentitySigningPublicKey,
) -> [u8; IDENTITY_PUBLIC_KEY_LEN] {
    let mut bytes = [0u8; IDENTITY_PUBLIC_KEY_LEN];
    bytes[..X25519PublicKey::LEN].copy_from_slice(encryption_public.as_bytes());
    bytes[X25519PublicKey::LEN..].copy_from_slice(signing_public.as_bytes());
    bytes
}

pub(crate) fn derive_identity_hash(
    encryption_public: &IdentityEncryptionPublicKey,
    signing_public: &IdentitySigningPublicKey,
) -> IdentityHash {
    let full = sha256(&concat_public_keys(encryption_public, signing_public));
    let mut truncated = [0u8; TRUNCATED_HASH_BYTE_LEN];
    truncated.copy_from_slice(&full[..TRUNCATED_HASH_BYTE_LEN]);
    IdentityHash(truncated)
}

/// RNS 1.3.5 `Identity.DERIVED_KEY_LENGTH`
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
    RatchetRequired,
}

/// RNS 1.3.5 `Identity.decrypt(..., enforce_ratchets=…)`: whether the identity key may open a token that no retained ratchet authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityKeyFallback {
    Permitted,
    Refused,
}

/// The reference surfaces this as `Destination.latest_ratchet_id` (`None` when the identity key opened it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenedBy {
    Ratchet(RatchetId),
    IdentityKey,
}

#[derive(Debug, PartialEq, Eq)]
pub struct OpenedToken<'t> {
    pub opened_by: OpenedBy,
    pub plaintext: &'t [u8],
}

struct DerivedPacketKey(Zeroizing<[u8; DERIVED_PACKET_KEY_LEN]>);

impl DerivedPacketKey {
    fn derive(shared_secret: &X25519SharedSecret, recipient_identity_hash: &IdentityHash) -> Self {
        Self(Zeroizing::new(hkdf_sha256::<DERIVED_PACKET_KEY_LEN>(
            shared_secret.as_bytes(),
            recipient_identity_hash.as_bytes(),
            &[],
        )))
    }

    fn token_key(&self) -> TokenKey<'_> {
        TokenKey::from_aes256(&self.0)
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
        IdentityKeyFallback::Permitted,
        ciphertext_token,
    )
    .map(|opened| opened.plaintext)
}

/// RNS 1.3.5 `Identity.decrypt(ciphertext, ratchets=…)`: ratchets newest-first, then the identity key.
/// The HKDF salt stays the *identity* hash even when a ratchet did the exchange (reference `get_salt` is `self.hash` unconditionally).
/// Candidates are probed by MAC so the buffer decrypts in place exactly once.
pub fn decrypt_token_in_place_with_ratchets<'t>(
    ratchet_secrets: &[X25519SecretKey],
    encryption_secret: &X25519SecretKey,
    recipient_identity_hash: &IdentityHash,
    fallback: IdentityKeyFallback,
    ciphertext_token: &'t mut [u8],
) -> Result<OpenedToken<'t>, DecryptError> {
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
    let winning_ratchet = ratchet_secrets.iter().find_map(|secret| {
        let candidate = derive_for(secret);
        token_is_authentic(&candidate.token_key(), token).then_some((candidate, secret))
    });
    let (key, opened_by) = match (winning_ratchet, fallback) {
        (Some((key, secret)), _) => (key, OpenedBy::Ratchet(RatchetId::of_secret(secret))),
        (None, IdentityKeyFallback::Permitted) => {
            (derive_for(encryption_secret), OpenedBy::IdentityKey)
        }
        (None, IdentityKeyFallback::Refused) => return Err(DecryptError::RatchetRequired),
    };

    let plaintext = token_open_in_place(&key.token_key(), token).map_err(|error| match error {
        TokenOpenError::Malformed
        | TokenOpenError::InvalidMac
        | TokenOpenError::InvalidPadding
        | TokenOpenError::BufferTooShort => DecryptError::InvalidToken,
    })?;
    Ok(OpenedToken {
        opened_by,
        plaintext,
    })
}

pub(crate) fn decrypt_finish_in_place<'t>(
    shared: &X25519SharedSecret,
    recipient_identity_hash: &IdentityHash,
    token: &'t mut [u8],
) -> Result<&'t [u8], DecryptError> {
    let key = DerivedPacketKey::derive(shared, recipient_identity_hash);
    token_open_in_place(&key.token_key(), token).map_err(|error| match error {
        TokenOpenError::Malformed
        | TokenOpenError::InvalidMac
        | TokenOpenError::InvalidPadding
        | TokenOpenError::BufferTooShort => DecryptError::InvalidToken,
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
        TokenOpenError::BufferTooShort => DecryptError::BufferTooShort,
        TokenOpenError::Malformed | TokenOpenError::InvalidMac | TokenOpenError::InvalidPadding => {
            DecryptError::InvalidToken
        }
    })
}

/// The encrypting side of RNS 1.3.5 `Identity.encrypt`. No private material.
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

    /// RNS 1.3.5 `Identity.encrypt(ratchet=…)`. Only the Diffie-Hellman target changes; the HKDF salt stays the identity hash.
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
        let (ephemeral_public, shared) = x25519_keys_for_seal(ephemeral_secret, dh_target);
        seal_finish(&self.hash, &ephemeral_public, &shared, iv, plaintext, out)
    }
}

/// The inline and pooled-crypto paths both finish through here, so the seal stays byte-identical either way.
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

pub mod in_memory;
