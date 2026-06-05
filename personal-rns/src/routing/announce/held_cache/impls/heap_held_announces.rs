use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::held_cache::{
    HeldAnnounce, HeldAnnounces, HoldReason, ParkOutcome, HELD_APP_DATA_LIMIT,
};
use crate::routing::announce::Announce;

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
        let mut best = 0;
        for i in 1..self.entries.len() {
            let cur = (
                self.entries[i].received_hops(),
                self.entries[i].arrived_at().0,
            );
            let best_key = (
                self.entries[best].received_hops(),
                self.entries[best].arrived_at().0,
            );
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

        let held = cache.take_next().unwrap();
        assert_eq!(held.received_hops(), 0);
        assert_eq!(held.announce().destination, dest(0));
    }

    #[test]
    fn app_data_at_the_held_limit_is_accepted_but_one_byte_past_is_rejected() {
        let mut cache = HeapHeldAnnounces::default();
        let any = InterfaceId::new([0u8; 16]);
        let exact = std::vec![0xA5; HELD_APP_DATA_LIMIT];
        let too_large = std::vec![0x5A; HELD_APP_DATA_LIMIT + 1];

        assert_eq!(
            cache.park(
                &announce_for(dest(1), &exact),
                ts(100),
                1,
                HoldReason::RoutingArenaPressure,
                any
            ),
            ParkOutcome::Parked
        );
        assert_eq!(
            cache.park(
                &announce_for(dest(2), &too_large),
                ts(200),
                1,
                HoldReason::RoutingArenaPressure,
                any
            ),
            ParkOutcome::AppDataTooLarge
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn take_next_selects_the_lowest_priority_even_when_not_first() {
        let mut cache = HeapHeldAnnounces::default();
        let any = InterfaceId::new([0u8; 16]);

        cache.park(
            &announce_for(dest(1), b"later"),
            ts(300),
            4,
            HoldReason::RoutingArenaPressure,
            any,
        );
        cache.park(
            &announce_for(dest(2), b"earliest"),
            ts(100),
            4,
            HoldReason::RoutingArenaPressure,
            any,
        );
        cache.park(
            &announce_for(dest(3), b"nearest"),
            ts(200),
            2,
            HoldReason::RoutingArenaPressure,
            any,
        );

        let held = cache.take_next().unwrap();
        assert_eq!(held.received_hops(), 2);
        assert_eq!(held.announce().destination, dest(3));
    }

    #[test]
    fn take_next_keeps_first_inserted_when_priority_is_identical() {
        let mut cache = HeapHeldAnnounces::default();
        let any = InterfaceId::new([0u8; 16]);

        cache.park(
            &announce_for(dest(1), b"first"),
            ts(100),
            3,
            HoldReason::RoutingArenaPressure,
            any,
        );
        cache.park(
            &announce_for(dest(2), b"second"),
            ts(100),
            3,
            HoldReason::RoutingArenaPressure,
            any,
        );

        assert_eq!(cache.take_next().unwrap().announce().destination, dest(1));
    }
}
