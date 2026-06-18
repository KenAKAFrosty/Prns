use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
use crate::interfaces::InterfaceId;
use crate::routing::announce::held::{HeldAnnounce, HeldAnnounceColumns};
use crate::routing::announce::retained::RetainedAnnounceEntry;
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys};
use crate::wire::DestinationHash;

fn vacant() -> HeldAnnounce {
    HeldAnnounce {
        destination: DestinationHash::new([0u8; 16]),
        hops: 0,
        receiving_interface: InterfaceId::new([0u8; 8]),
        announce: RetainedAnnounceEntry {
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            },
            dotted_name_hash: DottedNameHash::new([0u8; 10]),
            retained_announce_id: AnnounceId::from_wire([0u8; 10]),
            signature: Ed25519Signature([0u8; 64]),
            maybe_ratchet: None,
            maybe_app_data_handle: None,
        },
    }
}

#[derive(Debug)]
pub struct FixedHeldAnnounceColumns<const MAX_HELD: usize> {
    len: usize,
    rows: [HeldAnnounce; MAX_HELD],
}

impl<const MAX_HELD: usize> Default for FixedHeldAnnounceColumns<MAX_HELD> {
    fn default() -> Self {
        Self {
            len: 0,
            rows: [vacant(); MAX_HELD],
        }
    }
}

impl<const MAX_HELD: usize> HeldAnnounceColumns for FixedHeldAnnounceColumns<MAX_HELD> {
    fn capacity(&self) -> usize {
        MAX_HELD
    }

    fn rows(&self) -> &[HeldAnnounce] {
        &self.rows[..self.len]
    }

    fn rows_mut(&mut self) -> &mut [HeldAnnounce] {
        &mut self.rows[..self.len]
    }

    fn push(&mut self, row: HeldAnnounce) {
        if self.len >= MAX_HELD {
            return;
        }
        self.rows[self.len] = row;
        self.len += 1;
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        if index != last {
            self.rows[index] = self.rows[last];
        }
        self.len = last;
    }
}
