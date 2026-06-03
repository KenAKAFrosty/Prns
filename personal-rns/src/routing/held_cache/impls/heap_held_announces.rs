//! Growable (std/alloc) held-announce cache — the unbounded [`HeldAnnounces`].
//! Array-of-structs (one `Vec<HeldAnnounce>`) since there's no const-array layout
//! to keep; never returns `CacheFull`.

use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::Announce;
use crate::routing::held_cache::{
    HeldAnnounce, HeldAnnounces, HoldReason, ParkOutcome, HELD_APP_DATA_LIMIT,
};

#[derive(Debug, Default)]
pub struct HeapHeldAnnounces {
    entries: Vec<HeldAnnounce>,
}

impl HeldAnnounces for HeapHeldAnnounces {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn park(
        &mut self,
        announce: &Announce<'_>,
        arrived_at: InstantMillis,
        received_hops: u8,
        reason: HoldReason,
        source_interface: InterfaceId,
    ) -> ParkOutcome {
        if announce.app_data.len() > HELD_APP_DATA_LIMIT {
            return ParkOutcome::AppDataTooLarge;
        }
        let mut app_data_buf = [0u8; HELD_APP_DATA_LIMIT];
        let app_data_len = announce.app_data.len();
        app_data_buf[..app_data_len].copy_from_slice(announce.app_data);
        let held = HeldAnnounce {
            reason,
            destination: announce.destination,
            public_keys: announce.public_keys,
            dotted_name_hash: announce.dotted_name_hash,
            announce_id: announce.announce_id,
            maybe_ratchet: announce.maybe_ratchet,
            signature: announce.signature,
            app_data_buf,
            app_data_len: app_data_len as u16,
            arrived_at,
            received_hops,
            source_interface,
        };
        if let Some(slot) = self
            .entries
            .iter_mut()
            .find(|entry| entry.announce().destination == announce.destination)
        {
            *slot = held;
            ParkOutcome::Overwrote
        } else {
            self.entries.push(held);
            ParkOutcome::Parked
        }
    }

    fn take_next(&mut self) -> Option<HeldAnnounce> {
        if self.entries.is_empty() {
            return None;
        }
        // Lowest received_hops wins; ties break to the oldest arrival.
        let mut best = 0;
        for i in 1..self.entries.len() {
            let cur = (self.entries[i].received_hops(), self.entries[i].arrived_at().0);
            let best_key = (self.entries[best].received_hops(), self.entries[best].arrived_at().0);
            if cur < best_key {
                best = i;
            }
        }
        Some(self.entries.swap_remove(best))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
    use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
    use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys};
    use crate::wire::DestinationHash;

    fn ts(n: u64) -> InstantMillis {
        InstantMillis(n)
    }
    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }
    fn announce_for<'a>(destination: DestinationHash, app_data: &'a [u8]) -> Announce<'a> {
        Announce {
            destination,
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            },
            dotted_name_hash: DottedNameHash::new([0u8; 10]),
            announce_id: AnnounceId::from_wire([0u8; 10]),
            maybe_ratchet: None,
            signature: Ed25519Signature([0u8; 64]),
            app_data,
        }
    }

    #[test]
    fn grows_past_a_fixed_cap_overwrites_and_orders_by_hops() {
        let mut cache = HeapHeldAnnounces::default();
        let any = InterfaceId::new([0u8; 16]);
        // 200 distinct destinations — well past any fixed CAPACITY; never CacheFull.
        for n in 0..200u8 {
            assert_eq!(
                cache.park(
                    &announce_for(dest(n), b"x"),
                    ts(100 + n as u64),
                    n,
                    HoldReason::RoutingArenaPressure,
                    any
                ),
                ParkOutcome::Parked
            );
        }
        assert_eq!(cache.len(), 200);

        // Re-park an existing destination → overwrite, not grow.
        assert_eq!(
            cache.park(
                &announce_for(dest(0), b"y"),
                ts(50),
                0,
                HoldReason::RoutingArenaPressure,
                any
            ),
            ParkOutcome::Overwrote
        );
        assert_eq!(cache.len(), 200);

        // take_next returns the lowest received_hops first — dest(0), hops 0.
        let held = cache.take_next().unwrap();
        assert_eq!(held.received_hops(), 0);
        assert_eq!(held.announce().destination, dest(0));
    }
}
