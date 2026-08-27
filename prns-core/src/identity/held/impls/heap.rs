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
}
