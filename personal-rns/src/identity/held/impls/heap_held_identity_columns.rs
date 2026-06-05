use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::crypto::{Ed25519SecretKey, X25519SecretKey};
use crate::identity::held::{HeldIdentityColumns, HoldIdentityError};
use crate::identity::{IdentityEncryptionPublicKey, IdentityHash, IdentitySigningPublicKey};

struct HeldSecrets {
    encryption: X25519SecretKey,
    signing: Ed25519SecretKey,
}

#[derive(Default)]
pub struct HeapHeldIdentityColumns {
    hashes: Vec<IdentityHash>,
    encryption_publics: Vec<IdentityEncryptionPublicKey>,
    signing_publics: Vec<IdentitySigningPublicKey>,

    /// A `Vec` stores its elements' bytes inline in its buffer, and growing past
    /// capacity allocates a bigger buffer, memcpys the raw bytes across, and frees
    /// the old buffer *without dropping the moved elements*. `ZeroizeOnDrop`
    /// never fires, and an unboxed secret would leave a byte-perfect copy of the
    /// key in freed memory on every regrowth. When Boxed, the `Vec` holds only
    /// pointers; the secret bytes sit at one stable heap address for their whole
    /// life, regrowth copies pointers, and the only copy of the key material is
    /// the one whose drop zeroizes it.
    secrets: Vec<Box<HeldSecrets>>,
}

impl HeldIdentityColumns for HeapHeldIdentityColumns {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_past_any_fixed_ceiling() {
        let mut columns = HeapHeldIdentityColumns::default();
        assert_eq!(columns.capacity(), usize::MAX);

        for n in 0..100u8 {
            let pushed = columns.push(
                IdentityHash::new([n; 16]),
                X25519SecretKey::new([n; 32]),
                Ed25519SecretKey::new([n; 32]),
                IdentityEncryptionPublicKey::new(crate::crypto::X25519PublicKey([n; 32])),
                IdentitySigningPublicKey::new(crate::crypto::Ed25519PublicKey([n; 32])),
            );
            assert_eq!(pushed, Ok(n as usize));
        }
        assert_eq!(columns.len(), 100);
        assert_eq!(columns.hashes().len(), 100);
        assert!(columns.encryption_secret_at(99).is_some());
        assert!(columns.signing_secret_at(99).is_some());
        assert!(columns.encryption_secret_at(100).is_none());
    }
}
