mod impls;

pub use impls::*;

use crate::crypto::{ed25519_sign, Ed25519SecretKey, Ed25519Signature, X25519SecretKey};
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{
    DecryptError, IdentityEncryptionPublicKey, IdentityHash, IdentitySigner,
    IdentitySigningPublicKey, IDENTITY_SECRET_KEY_LEN,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldIdentityError {
    StoreFull,
}

pub trait HeldIdentityColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn hashes(&self) -> &[IdentityHash];
    fn encryption_publics(&self) -> &[IdentityEncryptionPublicKey];
    fn signing_publics(&self) -> &[IdentitySigningPublicKey];
    fn encryption_secret_at(&self, index: usize) -> Option<&X25519SecretKey>;
    fn signing_secret_at(&self, index: usize) -> Option<&Ed25519SecretKey>;

    fn push(
        &mut self,
        hash: IdentityHash,
        encryption_secret: X25519SecretKey,
        signing_secret: Ed25519SecretKey,
        encryption_public: IdentityEncryptionPublicKey,
        signing_public: IdentitySigningPublicKey,
    ) -> Result<usize, HoldIdentityError>;
}

#[derive(Default)]
pub struct HeldIdentities<C: HeldIdentityColumns> {
    columns: C,
}

impl<C: HeldIdentityColumns> HeldIdentities<C> {
    pub fn hold(
        &mut self,
        secret_key_bytes: &[u8; IDENTITY_SECRET_KEY_LEN],
    ) -> Result<IdentityHash, HoldIdentityError> {
        let parts = InMemoryNodeIdentity::from_secret_key_bytes(secret_key_bytes).into_parts();
        if self.columns.hashes().contains(&parts.hash) {
            return Ok(parts.hash);
        }
        self.columns.push(
            parts.hash,
            parts.encryption_secret,
            parts.signing_secret,
            parts.encryption_public,
            parts.signing_public,
        )?;
        Ok(parts.hash)
    }

    pub fn contains(&self, hash: &IdentityHash) -> bool {
        self.columns.hashes().contains(hash)
    }

    pub fn get(&self, hash: &IdentityHash) -> Option<HeldIdentityRef<'_>> {
        let index = self
            .columns
            .hashes()
            .iter()
            .position(|candidate| candidate == hash)?;
        Some(HeldIdentityRef {
            encryption_secret: self.columns.encryption_secret_at(index)?,
            signing_secret: self.columns.signing_secret_at(index)?,
            encryption_public: *self.columns.encryption_publics().get(index)?,
            signing_public: *self.columns.signing_publics().get(index)?,
            hash: *hash,
        })
    }

    pub fn hashes(&self) -> &[IdentityHash] {
        self.columns.hashes()
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

impl<C: HeldIdentityColumns> core::fmt::Debug for HeldIdentities<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HeldIdentities")
            .field("hashes", &self.columns.hashes())
            .finish()
    }
}

pub struct HeldIdentityRef<'a> {
    encryption_secret: &'a X25519SecretKey,
    signing_secret: &'a Ed25519SecretKey,
    encryption_public: IdentityEncryptionPublicKey,
    signing_public: IdentitySigningPublicKey,
    hash: IdentityHash,
}

impl HeldIdentityRef<'_> {
    pub fn decrypt_in_place<'t>(
        &self,
        ciphertext_token: &'t mut [u8],
    ) -> Result<&'t [u8], DecryptError> {
        super::decrypt_token_in_place(self.encryption_secret, &self.hash, ciphertext_token)
    }

    pub fn decrypt(&self, ciphertext_token: &[u8], out: &mut [u8]) -> Result<usize, DecryptError> {
        super::decrypt_token(self.encryption_secret, &self.hash, ciphertext_token, out)
    }
}

impl IdentitySigner for HeldIdentityRef<'_> {
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
        ed25519_sign(self.signing_secret, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{RemoteIdentity, ENCRYPTION_IV_LEN};

    type TestIdentities = HeldIdentities<FixedHeldIdentityColumns<2>>;

    fn secret_key_bytes(fill: u8) -> [u8; IDENTITY_SECRET_KEY_LEN] {
        let mut bytes = [0u8; IDENTITY_SECRET_KEY_LEN];
        bytes[..32].fill(fill);
        bytes[32..].fill(fill.wrapping_add(1));
        bytes
    }

    #[test]
    fn hold_derives_the_same_hash_as_the_freestanding_identity() {
        let bytes = secret_key_bytes(0x22);
        let freestanding = InMemoryNodeIdentity::from_secret_key_bytes(&bytes);
        let mut identities = TestIdentities::default();
        assert_eq!(identities.hold(&bytes), Ok(freestanding.identity_hash()));
        assert_eq!(identities.len(), 1);
    }

    #[test]
    fn re_holding_the_same_key_is_idempotent() {
        let bytes = secret_key_bytes(0x22);
        let mut identities = TestIdentities::default();
        let first = identities.hold(&bytes).unwrap();
        let second = identities.hold(&bytes).unwrap();
        assert_eq!(first, second);
        assert_eq!(identities.len(), 1);
    }

    #[test]
    fn a_full_store_reports_itself() {
        let mut identities = TestIdentities::default();
        assert!(identities.hold(&secret_key_bytes(0x11)).is_ok());
        assert!(identities.hold(&secret_key_bytes(0x33)).is_ok());
        assert_eq!(
            identities.hold(&secret_key_bytes(0x55)),
            Err(HoldIdentityError::StoreFull),
        );
        assert_eq!(identities.len(), 2);
    }

    #[test]
    fn get_misses_an_unheld_hash() {
        let mut identities = TestIdentities::default();
        identities.hold(&secret_key_bytes(0x11)).unwrap();
        let unheld = IdentityHash::new([0x99; 16]);
        assert!(!identities.contains(&unheld));
        assert!(identities.get(&unheld).is_none());
    }

    #[test]
    fn a_held_ref_signs_byte_identically_to_the_freestanding_identity() {
        let bytes = secret_key_bytes(0x42);
        let freestanding = InMemoryNodeIdentity::from_secret_key_bytes(&bytes);
        let mut identities = TestIdentities::default();
        let hash = identities.hold(&bytes).unwrap();
        let held = identities.get(&hash).unwrap();

        let message = b"announce body to sign";
        assert_eq!(held.sign(message), freestanding.sign(message));
        assert_eq!(
            held.encryption_public_key(),
            freestanding.encryption_public_key()
        );
        assert_eq!(held.signing_public_key(), freestanding.signing_public_key());
        assert_eq!(held.identity_hash(), freestanding.identity_hash());
    }

    #[test]
    fn a_held_ref_decrypts_what_was_sealed_for_its_identity() {
        let bytes = secret_key_bytes(0x42);
        let mut identities = TestIdentities::default();
        let hash = identities.hold(&bytes).unwrap();
        let held = identities.get(&hash).unwrap();

        let remote = RemoteIdentity::from_public_keys(
            held.encryption_public_key(),
            held.signing_public_key(),
        );
        let ephemeral = X25519SecretKey::new([0x07; 32]);
        let iv = [0x0Au8; ENCRYPTION_IV_LEN];
        let plaintext = b"hello through the store";
        let mut sealed = [0u8; 256];
        let sealed_len = remote
            .encrypt(&ephemeral, &iv, plaintext, &mut sealed)
            .unwrap();

        let mut in_place = [0u8; 256];
        in_place[..sealed_len].copy_from_slice(&sealed[..sealed_len]);
        assert_eq!(
            held.decrypt_in_place(&mut in_place[..sealed_len]),
            Ok(plaintext.as_slice()),
        );
    }

    #[test]
    fn the_wrong_identity_cannot_open_a_sealed_token() {
        let mut identities = TestIdentities::default();
        let right = identities.hold(&secret_key_bytes(0x42)).unwrap();
        let wrong = identities.hold(&secret_key_bytes(0x77)).unwrap();

        let sealed_for = identities.get(&right).unwrap();
        let remote = RemoteIdentity::from_public_keys(
            sealed_for.encryption_public_key(),
            sealed_for.signing_public_key(),
        );
        let ephemeral = X25519SecretKey::new([0x07; 32]);
        let iv = [0x0Au8; ENCRYPTION_IV_LEN];
        let mut sealed = [0u8; 256];
        let sealed_len = remote
            .encrypt(&ephemeral, &iv, b"for right only", &mut sealed)
            .unwrap();

        let opener = identities.get(&wrong).unwrap();
        assert_eq!(
            opener.decrypt_in_place(&mut sealed[..sealed_len]),
            Err(DecryptError::InvalidToken),
        );
    }
}
