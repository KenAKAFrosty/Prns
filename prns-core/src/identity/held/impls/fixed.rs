use crate::crypto::{Ed25519SecretKey, X25519SecretKey};
use crate::identity::held::{HeldIdentityTable, HoldIdentityError};
use crate::identity::{IdentityEncryptionPublicKey, IdentityHash, IdentitySigningPublicKey};
use heapless::Vec as HeaplessVec;

pub struct FixedHeldIdentityTable<const MAX_HELD_IDENTITIES: usize> {
    hashes: HeaplessVec<IdentityHash, MAX_HELD_IDENTITIES>,
    encryption_secrets: HeaplessVec<X25519SecretKey, MAX_HELD_IDENTITIES>,
    signing_secrets: HeaplessVec<Ed25519SecretKey, MAX_HELD_IDENTITIES>,
    encryption_publics: HeaplessVec<IdentityEncryptionPublicKey, MAX_HELD_IDENTITIES>,
    signing_publics: HeaplessVec<IdentitySigningPublicKey, MAX_HELD_IDENTITIES>,
}

impl<const MAX_HELD_IDENTITIES: usize> Default for FixedHeldIdentityTable<MAX_HELD_IDENTITIES> {
    fn default() -> Self {
        Self {
            hashes: HeaplessVec::new(),
            encryption_secrets: HeaplessVec::new(),
            signing_secrets: HeaplessVec::new(),
            encryption_publics: HeaplessVec::new(),
            signing_publics: HeaplessVec::new(),
        }
    }
}

impl<const MAX_HELD_IDENTITIES: usize> HeldIdentityTable
    for FixedHeldIdentityTable<MAX_HELD_IDENTITIES>
{
    fn capacity(&self) -> usize {
        MAX_HELD_IDENTITIES
    }
    fn len(&self) -> usize {
        self.hashes.len()
    }

    fn hashes(&self) -> &[IdentityHash] {
        &self.hashes
    }
    fn encryption_publics(&self) -> &[IdentityEncryptionPublicKey] {
        &self.encryption_publics
    }
    fn signing_publics(&self) -> &[IdentitySigningPublicKey] {
        &self.signing_publics
    }
    fn encryption_secret_at(&self, index: usize) -> Option<&X25519SecretKey> {
        self.encryption_secrets.get(index)
    }
    fn signing_secret_at(&self, index: usize) -> Option<&Ed25519SecretKey> {
        self.signing_secrets.get(index)
    }

    fn push(
        &mut self,
        hash: IdentityHash,
        encryption_secret: X25519SecretKey,
        signing_secret: Ed25519SecretKey,
        encryption_public: IdentityEncryptionPublicKey,
        signing_public: IdentitySigningPublicKey,
    ) -> Result<usize, HoldIdentityError> {
        if self.hashes.is_full() {
            return Err(HoldIdentityError::StoreFull);
        }
        let i = self.hashes.len();
        let _ = self.hashes.push(hash);
        let _ = self.encryption_secrets.push(encryption_secret);
        let _ = self.signing_secrets.push(signing_secret);
        let _ = self.encryption_publics.push(encryption_public);
        let _ = self.signing_publics.push(signing_public);
        Ok(i)
    }

    fn pop(&mut self) {
        let _hash = self.hashes.pop();
        let _encryption_secret = self.encryption_secrets.pop();
        let _signing_secret = self.signing_secrets.pop();
        let _encryption_public = self.encryption_publics.pop();
        let _signing_public = self.signing_publics.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_inputs(
        byte: u8,
    ) -> (
        IdentityHash,
        X25519SecretKey,
        Ed25519SecretKey,
        IdentityEncryptionPublicKey,
        IdentitySigningPublicKey,
    ) {
        (
            IdentityHash::new([byte; 16]),
            X25519SecretKey::new([byte; 32]),
            Ed25519SecretKey::new([byte; 32]),
            IdentityEncryptionPublicKey::new(crate::crypto::X25519PublicKey([byte; 32])),
            IdentitySigningPublicKey::new(crate::crypto::Ed25519PublicKey([byte; 32])),
        )
    }

    #[test]
    fn exposes_only_pushed_rows_and_reports_a_full_store() {
        let mut table = FixedHeldIdentityTable::<2>::default();
        assert_eq!(table.capacity(), 2);
        assert!(table.is_empty());
        assert!(table.hashes().is_empty());
        assert!(table.encryption_secret_at(0).is_none());

        let (hash_a, enc_a, sig_a, enc_pub_a, sig_pub_a) = row_inputs(1);
        let (hash_b, enc_b, sig_b, enc_pub_b, sig_pub_b) = row_inputs(2);
        let (hash_c, enc_c, sig_c, enc_pub_c, sig_pub_c) = row_inputs(3);
        assert_eq!(
            table.push(hash_a, enc_a, sig_a, enc_pub_a, sig_pub_a),
            Ok(0)
        );
        assert_eq!(
            table.push(hash_b, enc_b, sig_b, enc_pub_b, sig_pub_b),
            Ok(1)
        );
        assert_eq!(
            table.push(hash_c, enc_c, sig_c, enc_pub_c, sig_pub_c),
            Err(HoldIdentityError::StoreFull),
        );

        assert_eq!(table.len(), 2);
        assert_eq!(table.hashes(), &[hash_a, hash_b]);
        assert_eq!(table.encryption_publics(), &[enc_pub_a, enc_pub_b]);
        assert_eq!(table.signing_publics(), &[sig_pub_a, sig_pub_b]);
        assert!(table.encryption_secret_at(1).is_some());
        assert!(table.signing_secret_at(1).is_some());
        assert!(table.encryption_secret_at(2).is_none());
    }
}
