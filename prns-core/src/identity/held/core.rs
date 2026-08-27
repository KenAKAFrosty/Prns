use crate::crypto::{ed25519_sign, Ed25519SecretKey, Ed25519Signature, X25519SecretKey};
use crate::identity::in_memory::IdentityParts;
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{
    DecryptError, IdentityEncryptionPublicKey, IdentityHash, IdentityKeyFallback, IdentitySigner,
    IdentitySigningPublicKey, OpenedToken, Zeroizing, IDENTITY_SECRET_KEY_LEN,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldIdentityError {
    StoreFull,
}

pub trait HeldIdentityTable {
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

    fn pop(&mut self);
}

#[derive(Default)]
pub struct HeldIdentities<C: HeldIdentityTable> {
    table: C,
}

impl<C: HeldIdentityTable> HeldIdentities<C> {
    pub fn hold(
        &mut self,
        secret_key_bytes: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    ) -> Result<IdentityHash, HoldIdentityError> {
        let parts = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key_bytes).into_parts();
        let identity = parts.hash;
        if self.contains(&identity) {
            return Ok(identity);
        }
        self.push_parts(parts)?;
        Ok(identity)
    }

    pub fn hold_pair(
        &mut self,
        first: IdentityParts,
        second: IdentityParts,
    ) -> Result<(), HoldIdentityError> {
        let first_is_new = !self.contains(&first.hash);
        let identities_are_distinct = first.hash != second.hash;
        let second_is_new = identities_are_distinct && !self.contains(&second.hash);
        let required_capacity =
            usize::from(first_is_new).saturating_add(usize::from(second_is_new));
        let available_capacity = self.table.capacity().saturating_sub(self.table.len());
        if required_capacity > available_capacity {
            return Err(HoldIdentityError::StoreFull);
        }

        if first_is_new {
            self.push_parts(first)?;
        }
        if !second_is_new {
            return Ok(());
        }
        if let Err(error) = self.push_parts(second) {
            if first_is_new {
                self.table.pop();
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn contains(&self, hash: &IdentityHash) -> bool {
        self.table.hashes().contains(hash)
    }

    pub fn get(&self, hash: &IdentityHash) -> Option<HeldIdentityRef<'_>> {
        let index = self
            .table
            .hashes()
            .iter()
            .position(|candidate| candidate == hash)?;

        Some(HeldIdentityRef {
            encryption_secret: self.table.encryption_secret_at(index)?,
            signing_secret: self.table.signing_secret_at(index)?,
            encryption_public: *self.table.encryption_publics().get(index)?,
            signing_public: *self.table.signing_publics().get(index)?,
            hash: *hash,
        })
    }

    pub fn hashes(&self) -> &[IdentityHash] {
        self.table.hashes()
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    fn push_parts(&mut self, parts: IdentityParts) -> Result<(), HoldIdentityError> {
        self.table.push(
            parts.hash,
            parts.encryption_secret,
            parts.signing_secret,
            parts.encryption_public,
            parts.signing_public,
        )?;
        Ok(())
    }
}

impl<C: HeldIdentityTable> core::fmt::Debug for HeldIdentities<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HeldIdentities")
            .field("hashes", &self.table.hashes())
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
        crate::identity::decrypt_token_in_place(
            self.encryption_secret,
            &self.hash,
            ciphertext_token,
        )
    }

    pub fn decrypt_in_place_with_ratchets<'t>(
        &self,
        ratchet_secrets: &[X25519SecretKey],
        fallback: IdentityKeyFallback,
        ciphertext_token: &'t mut [u8],
    ) -> Result<OpenedToken<'t>, DecryptError> {
        crate::identity::decrypt_token_in_place_with_ratchets(
            ratchet_secrets,
            self.encryption_secret,
            &self.hash,
            fallback,
            ciphertext_token,
        )
    }

    pub fn decrypt(&self, ciphertext_token: &[u8], out: &mut [u8]) -> Result<usize, DecryptError> {
        crate::identity::decrypt_token(self.encryption_secret, &self.hash, ciphertext_token, out)
    }

    pub fn signing_secret_clone(&self) -> Ed25519SecretKey {
        self.signing_secret.cloned()
    }

    pub fn encryption_secret_clone(&self) -> X25519SecretKey {
        self.encryption_secret.cloned()
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
    use super::super::*;
    use super::*;
    use crate::crypto::ratchets::RatchetId;
    use crate::crypto::{Ed25519SecretKey, X25519SecretKey};
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentitySigningPublicKey, OpenedBy, RemoteIdentity,
        ENCRYPTION_IV_LEN,
    };

    type TestIdentities = HeldIdentities<FixedHeldIdentityTable<2>>;

    #[derive(Default)]
    struct RejectSecondIdentityTable {
        inner: FixedHeldIdentityTable<2>,
    }

    impl HeldIdentityTable for RejectSecondIdentityTable {
        fn capacity(&self) -> usize {
            self.inner.capacity()
        }

        fn len(&self) -> usize {
            self.inner.len()
        }

        fn hashes(&self) -> &[IdentityHash] {
            self.inner.hashes()
        }

        fn encryption_publics(&self) -> &[IdentityEncryptionPublicKey] {
            self.inner.encryption_publics()
        }

        fn signing_publics(&self) -> &[IdentitySigningPublicKey] {
            self.inner.signing_publics()
        }

        fn encryption_secret_at(&self, index: usize) -> Option<&X25519SecretKey> {
            self.inner.encryption_secret_at(index)
        }

        fn signing_secret_at(&self, index: usize) -> Option<&Ed25519SecretKey> {
            self.inner.signing_secret_at(index)
        }

        fn push(
            &mut self,
            hash: IdentityHash,
            encryption_secret: X25519SecretKey,
            signing_secret: Ed25519SecretKey,
            encryption_public: IdentityEncryptionPublicKey,
            signing_public: IdentitySigningPublicKey,
        ) -> Result<usize, HoldIdentityError> {
            if self.inner.len() == 1 {
                return Err(HoldIdentityError::StoreFull);
            }
            self.inner.push(
                hash,
                encryption_secret,
                signing_secret,
                encryption_public,
                signing_public,
            )
        }

        fn pop(&mut self) {
            self.inner.pop();
        }
    }

    fn secret_key_bytes(fill: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
        let mut bytes = [0u8; IDENTITY_SECRET_KEY_LEN];
        bytes[..32].fill(fill);
        bytes[32..].fill(fill.wrapping_add(1));
        Zeroizing::new(bytes)
    }

    fn identity_parts(fill: u8) -> IdentityParts {
        InMemoryNodeIdentity::from_secret_key_bytes(&secret_key_bytes(fill)).into_parts()
    }

    #[test]
    fn hold_derives_the_same_hash_as_the_freestanding_identity() {
        let bytes = secret_key_bytes(0x22);
        let freestanding = InMemoryNodeIdentity::from_secret_key_bytes(&bytes);
        let mut identities = TestIdentities::default();
        assert_eq!(
            identities.hold(bytes.clone()),
            Ok(freestanding.identity_hash())
        );
        assert_eq!(identities.len(), 1);
    }

    #[test]
    fn re_holding_the_same_key_is_idempotent() {
        let bytes = secret_key_bytes(0x22);
        let mut identities = TestIdentities::default();
        let first = identities.hold(bytes.clone()).unwrap();
        let second = identities.hold(bytes.clone()).unwrap();
        assert_eq!(first, second);
        assert_eq!(identities.len(), 1);
    }

    #[test]
    fn a_full_store_reports_itself() {
        let mut identities = TestIdentities::default();
        assert!(identities.hold(secret_key_bytes(0x11)).is_ok());
        assert!(identities.hold(secret_key_bytes(0x33)).is_ok());
        assert_eq!(
            identities.hold(secret_key_bytes(0x55)),
            Err(HoldIdentityError::StoreFull),
        );
        assert_eq!(identities.len(), 2);
    }

    #[test]
    fn a_pair_preflights_capacity_without_partially_holding_either_identity() {
        let mut identities: HeldIdentities<FixedHeldIdentityTable<1>> = HeldIdentities::default();

        assert_eq!(
            identities.hold_pair(identity_parts(0x11), identity_parts(0x33)),
            Err(HoldIdentityError::StoreFull),
        );
        assert!(identities.is_empty());
    }

    #[test]
    fn a_pair_holds_both_distinct_identities_as_one_operation() {
        let first = identity_parts(0x11);
        let second = identity_parts(0x33);
        let expected = [first.hash, second.hash];
        let mut identities = TestIdentities::default();

        assert_eq!(identities.hold_pair(first, second), Ok(()));
        assert_eq!(identities.hashes(), &expected);
    }

    #[test]
    fn a_pair_rolls_back_when_the_second_insertion_is_rejected() {
        let mut identities: HeldIdentities<RejectSecondIdentityTable> = HeldIdentities::default();

        assert_eq!(
            identities.hold_pair(identity_parts(0x11), identity_parts(0x33)),
            Err(HoldIdentityError::StoreFull),
        );
        assert!(identities.is_empty());
    }

    #[test]
    fn a_pair_only_spends_capacity_for_identities_not_already_held() {
        let first_secret = secret_key_bytes(0x11);
        let first = InMemoryNodeIdentity::from_secret_key_bytes(&first_secret).into_parts();
        let second = identity_parts(0x33);
        let first_hash = first.hash;
        let expected = [first.hash, second.hash];
        let mut identities = TestIdentities::default();

        assert_eq!(identities.hold(first_secret), Ok(first_hash));
        assert_eq!(identities.hold_pair(first, second), Ok(()));
        assert_eq!(identities.hashes(), &expected);
    }

    #[test]
    fn get_misses_an_unheld_hash() {
        let mut identities = TestIdentities::default();
        identities.hold(secret_key_bytes(0x11)).unwrap();
        let unheld = IdentityHash::new([0x99; 16]);
        assert!(!identities.contains(&unheld));
        assert!(identities.get(&unheld).is_none());
    }

    #[test]
    fn a_held_ref_signs_byte_identically_to_the_freestanding_identity() {
        let bytes = secret_key_bytes(0x42);
        let freestanding = InMemoryNodeIdentity::from_secret_key_bytes(&bytes);
        let mut identities = TestIdentities::default();
        let hash = identities.hold(bytes.clone()).unwrap();
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
        let hash = identities.hold(bytes.clone()).unwrap();
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

    fn seal_to_key(
        target: &crate::crypto::X25519PublicKey,
        salt: &IdentityHash,
        plaintext: &[u8],
    ) -> [u8; 96] {
        assert!(plaintext.len() < 16, "one-block tokens keep the size fixed");
        let ephemeral = X25519SecretKey::new([0x33; 32]);
        let shared = crate::crypto::x25519_diffie_hellman(&ephemeral, target);
        let key = crate::identity::DerivedPacketKey::derive(&shared, salt);
        let mut out = [0u8; 96];
        out[..32].copy_from_slice(&crate::crypto::x25519_public_key(&ephemeral).0);
        let n = crate::crypto::token_seal(
            &key.token_key(),
            &[0x44; ENCRYPTION_IV_LEN],
            plaintext,
            &mut out[32..],
        )
        .unwrap();
        assert_eq!(32 + n, out.len());
        out
    }

    #[test]
    fn ratchet_trials_pick_the_owning_key_and_fall_back_to_the_identity() {
        let mut identities = TestIdentities::default();
        let hash = identities.hold(secret_key_bytes(0x42)).unwrap();
        let held = identities.get(&hash).unwrap();

        let trials = [
            X25519SecretKey::new([0x66; 32]),
            X25519SecretKey::new([0x55; 32]),
        ];
        let ratchet_pub = crate::crypto::x25519_public_key(&X25519SecretKey::new([0x55; 32]));

        let mut sealed_to_ratchet = seal_to_key(&ratchet_pub, &hash, b"to-the-ratchet");
        assert_eq!(
            held.decrypt_in_place_with_ratchets(
                &trials,
                IdentityKeyFallback::Permitted,
                &mut sealed_to_ratchet,
            ),
            Ok(OpenedToken {
                opened_by: OpenedBy::Ratchet(RatchetId::of_public_key(&ratchet_pub)),
                plaintext: b"to-the-ratchet",
            }),
        );

        let mut sealed_to_identity = seal_to_key(
            held.encryption_public_key().as_x25519(),
            &hash,
            b"to-the-identity",
        );
        assert_eq!(
            held.decrypt_in_place_with_ratchets(
                &trials,
                IdentityKeyFallback::Permitted,
                &mut sealed_to_identity,
            ),
            Ok(OpenedToken {
                opened_by: OpenedBy::IdentityKey,
                plaintext: b"to-the-identity",
            }),
        );

        let stranger = crate::crypto::x25519_public_key(&X25519SecretKey::new([0x99; 32]));
        let mut sealed_to_stranger = seal_to_key(&stranger, &hash, b"to-a-stranger");
        assert_eq!(
            held.decrypt_in_place_with_ratchets(
                &trials,
                IdentityKeyFallback::Permitted,
                &mut sealed_to_stranger,
            ),
            Err(DecryptError::InvalidToken),
        );
    }

    #[test]
    fn a_required_ratchet_refuses_the_identity_key_fallback_by_name() {
        let mut identities = TestIdentities::default();
        let hash = identities.hold(secret_key_bytes(0x42)).unwrap();
        let held = identities.get(&hash).unwrap();
        let trials = [X25519SecretKey::new([0x55; 32])];

        let mut sealed_to_identity = seal_to_key(
            held.encryption_public_key().as_x25519(),
            &hash,
            b"to-the-identity",
        );
        assert_eq!(
            held.decrypt_in_place_with_ratchets(
                &trials,
                IdentityKeyFallback::Refused,
                &mut sealed_to_identity,
            ),
            Err(DecryptError::RatchetRequired),
        );

        let ratchet_pub = crate::crypto::x25519_public_key(&X25519SecretKey::new([0x55; 32]));
        let mut sealed_to_ratchet = seal_to_key(&ratchet_pub, &hash, b"to-the-ratchet");
        assert_eq!(
            held.decrypt_in_place_with_ratchets(
                &trials,
                IdentityKeyFallback::Refused,
                &mut sealed_to_ratchet,
            ),
            Ok(OpenedToken {
                opened_by: OpenedBy::Ratchet(RatchetId::of_public_key(&ratchet_pub)),
                plaintext: b"to-the-ratchet",
            }),
        );
    }

    #[test]
    fn a_remote_seal_to_our_announced_ratchet_round_trips() {
        let mut identities = TestIdentities::default();
        let hash = identities.hold(secret_key_bytes(0x42)).unwrap();
        let held = identities.get(&hash).unwrap();
        let remote = RemoteIdentity::from_public_keys(
            held.encryption_public_key(),
            held.signing_public_key(),
        );

        let ratchet_secret = X25519SecretKey::new([0x55; 32]);
        let ratchet_public = crate::crypto::x25519_public_key(&ratchet_secret);
        let mut sealed = [0u8; 256];
        let sealed_len = remote
            .encrypt_to_ratchet(
                &ratchet_public,
                &X25519SecretKey::new([0x77; 32]),
                &[0x0B; ENCRYPTION_IV_LEN],
                b"over-the-air",
                &mut sealed,
            )
            .unwrap();

        assert_eq!(
            held.decrypt_in_place_with_ratchets(
                ::core::slice::from_ref(&ratchet_secret),
                IdentityKeyFallback::Permitted,
                &mut sealed[..sealed_len],
            ),
            Ok(OpenedToken {
                opened_by: OpenedBy::Ratchet(RatchetId::of_secret(&ratchet_secret)),
                plaintext: b"over-the-air",
            }),
        );
    }

    #[test]
    fn the_wrong_identity_cannot_open_a_sealed_token() {
        let mut identities = TestIdentities::default();
        let right = identities.hold(secret_key_bytes(0x42)).unwrap();
        let wrong = identities.hold(secret_key_bytes(0x77)).unwrap();

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
