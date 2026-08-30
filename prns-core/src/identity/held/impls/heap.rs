use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::crypto::{Ed25519SecretKey, X25519SecretKey};
use crate::identity::held::{HeldIdentityTable, HoldIdentityError};
use crate::identity::{IdentityEncryptionPublicKey, IdentityHash, IdentitySigningPublicKey};

struct HeldSecrets {
    encryption: X25519SecretKey,
    signing: Ed25519SecretKey,
}

#[derive(Default)]
pub struct HeapHeldIdentityTable {
    hashes: Vec<IdentityHash>,
    encryption_publics: Vec<IdentityEncryptionPublicKey>,
    signing_publics: Vec<IdentitySigningPublicKey>,

    /// `Vec` regrowth memcpys raw bytes and frees the old buffer without dropping the moved elements.
    /// `ZeroizeOnDrop` doesn't fire in that situation, which would leave a byte-perfect key copy in freed memory.
    /// So we box it, and the secret sits at one stable address and the only copy of the secrets is the one whose drop zeroizes them.
    #[allow(clippy::vec_box)]
    secrets: Vec<Box<HeldSecrets>>,
}

impl HeldIdentityTable for HeapHeldIdentityTable {
    fn capacity(&self) -> usize {
        usize::MAX
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
        self.secrets.get(index).map(|secrets| &secrets.encryption)
    }
    fn signing_secret_at(&self, index: usize) -> Option<&Ed25519SecretKey> {
        self.secrets.get(index).map(|secrets| &secrets.signing)
    }

    fn push(
        &mut self,
        hash: IdentityHash,
        encryption_secret: X25519SecretKey,
        signing_secret: Ed25519SecretKey,
        encryption_public: IdentityEncryptionPublicKey,
        signing_public: IdentitySigningPublicKey,
    ) -> Result<usize, HoldIdentityError> {
        let i = self.hashes.len();
        self.hashes.push(hash);
        self.secrets.push(Box::new(HeldSecrets {
            encryption: encryption_secret,
            signing: signing_secret,
        }));
        self.encryption_publics.push(encryption_public);
        self.signing_publics.push(signing_public);
        Ok(i)
    }

    fn pop(&mut self) {
        let _hash = self.hashes.pop();
        let _secrets = self.secrets.pop();
        let _encryption_public = self.encryption_publics.pop();
        let _signing_public = self.signing_publics.pop();
    }

    fn swap_remove(&mut self, index: usize) {
        let _hash = self.hashes.swap_remove(index);
        let _secrets = self.secrets.swap_remove(index);
        let _encryption_public = self.encryption_publics.swap_remove(index);
        let _signing_public = self.signing_publics.swap_remove(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_past_any_fixed_ceiling() {
        let mut table = HeapHeldIdentityTable::default();
        assert_eq!(table.capacity(), usize::MAX);

        for n in 0..100u8 {
            let pushed = table.push(
                IdentityHash::new([n; 16]),
                X25519SecretKey::new([n; 32]),
                Ed25519SecretKey::new([n; 32]),
                IdentityEncryptionPublicKey::new(crate::crypto::X25519PublicKey([n; 32])),
                IdentitySigningPublicKey::new(crate::crypto::Ed25519PublicKey([n; 32])),
            );
            assert_eq!(pushed, Ok(n as usize));
        }
        assert_eq!(table.len(), 100);
        assert_eq!(table.hashes().len(), 100);
        assert!(table.encryption_secret_at(99).is_some());
        assert!(table.signing_secret_at(99).is_some());
        assert!(table.encryption_secret_at(100).is_none());
    }

    #[test]
    fn swap_remove_keeps_every_heap_column_aligned() {
        let mut table = HeapHeldIdentityTable::default();
        for byte in [1, 2, 3] {
            table
                .push(
                    IdentityHash::new([byte; 16]),
                    X25519SecretKey::new([byte; 32]),
                    Ed25519SecretKey::new([byte; 32]),
                    IdentityEncryptionPublicKey::new(crate::crypto::X25519PublicKey([byte; 32])),
                    IdentitySigningPublicKey::new(crate::crypto::Ed25519PublicKey([byte; 32])),
                )
                .unwrap();
        }

        table.swap_remove(1);

        assert_eq!(
            table.hashes(),
            &[IdentityHash::new([1; 16]), IdentityHash::new([3; 16])],
        );
        assert_eq!(
            table.encryption_publics(),
            &[
                IdentityEncryptionPublicKey::new(crate::crypto::X25519PublicKey([1; 32])),
                IdentityEncryptionPublicKey::new(crate::crypto::X25519PublicKey([3; 32])),
            ],
        );
        assert_eq!(
            table.signing_publics(),
            &[
                IdentitySigningPublicKey::new(crate::crypto::Ed25519PublicKey([1; 32])),
                IdentitySigningPublicKey::new(crate::crypto::Ed25519PublicKey([3; 32])),
            ],
        );
        assert!(table.encryption_secret_at(1).is_some());
        assert!(table.signing_secret_at(1).is_some());
        assert!(table.encryption_secret_at(2).is_none());
        assert!(table.signing_secret_at(2).is_none());
    }
}
