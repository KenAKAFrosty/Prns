//! The fixed-capacity, heap-backed twin of [`FixedHeldAnnounceColumns`]: the held announces
//! live in a caller-chosen heap region (PSRAM on the S3) via `A`. The dest lookup stays a
//! linear scan: held announces are touched only during a burst, bounded by the queue's own capacity.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
use crate::interfaces::InterfaceId;
use crate::routing::announce::held::{HeldAnnounce, HeldAnnounceColumns};
use crate::routing::announce::retained::RetainedAnnounceEntry;
use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys};
use crate::routing::NextHop;
use crate::wire::DestinationHash;

fn vacant() -> HeldAnnounce {
    HeldAnnounce {
        destination: DestinationHash::new([0u8; 16]),
        hops: 0,
        receiving_interface: InterfaceId::new([0u8; 8]),
        next_hop: NextHop::Direct,
        is_path_response: false,
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

pub struct FixedHeapHeldAnnounceColumns<const MAX_HELD: usize, A: Allocator = Global> {
    len: usize,
    rows: Box<[HeldAnnounce], A>,
}

impl<const MAX_HELD: usize, A: Allocator + Default> Default
    for FixedHeapHeldAnnounceColumns<MAX_HELD, A>
{
    fn default() -> Self {
        let mut rows = Vec::with_capacity_in(MAX_HELD, A::default());
        rows.resize(MAX_HELD, vacant());
        Self {
            len: 0,
            rows: rows.into_boxed_slice(),
        }
    }
}

impl<const MAX_HELD: usize, A: Allocator> HeldAnnounceColumns
    for FixedHeapHeldAnnounceColumns<MAX_HELD, A>
{
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
