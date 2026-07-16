use crate::identity::IdentityHash;
use crate::routing::blackhole::BlackholeTable;
use crate::routing::{
    BlackholeIdentityOutcome, BlackholedIdentity, RemovedRoute, UnblackholeIdentityOutcome,
};
use crate::storage::{DirtyInterfaceSet, StorageLayout};

use super::{EngineState, WakeSchedules};

impl<S: StorageLayout> EngineState<S> {
    pub fn blackholed_identity_count(&self) -> usize {
        self.identity_blackholes.len()
    }

    pub fn is_identity_blackholed(&self, identity: &IdentityHash) -> bool {
        self.identity_blackholes.is_blackholed(identity)
    }

    pub fn blackholed_identities(&self) -> impl Iterator<Item = BlackholedIdentity<&str>> + '_ {
        self.identity_blackholes.entries()
    }

    pub fn blackhole_identity(
        &mut self,
        entry: BlackholedIdentity<&str>,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> Result<BlackholeIdentityOutcome, <S::Blackholes as BlackholeTable>::InsertError> {
        let identity = entry.identity;
        let outcome = self.identity_blackholes.blackhole_identity(entry)?;
        if outcome == BlackholeIdentityOutcome::Added {
            let dirty = &mut self.dirty_interfaces;
            self.routing_table
                .drop_routes_for_identity(&identity, &mut |removed| {
                    dirty.mark(removed.receiving_interface);
                    on_removed(removed);
                });
        }
        Ok(outcome)
    }

    pub fn unblackhole_identity(&mut self, identity: &IdentityHash) -> UnblackholeIdentityOutcome {
        self.identity_blackholes.unblackhole_identity(identity)
    }

    pub fn cull_expired_blackholes(&mut self, now: crate::units::InstantMillis) -> WakeSchedules {
        self.identity_blackholes.cull_expired(now);
        WakeSchedules {
            expired_blackholes: self.blackhole_expiry_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{
        bytes_from_hex, test_fill_entropy, tick_capture, transporting_interfaces,
        transporting_node, RNS_1_3_5_ANNOUNCE,
    };
    use crate::engine::{AnnounceIngest, DeferredCrypto, IngestPacketOutcome, WakeSchedule};
    use crate::interfaces::{AttachedInterfaces, InboundPacket, InterfaceId};
    use crate::routing::ingress::Ingress;
    use crate::routing::{BlackholeExpiry, RouteRemovalCause};
    use crate::units::InstantMillis;

    fn identity_hash_from_announce(bytes: &mut [u8], source: InterfaceId) -> IdentityHash {
        let Ingress::Announce { identity_hash, .. } = Ingress::classify(InboundPacket {
            arrived_at: InstantMillis(1_000),
            source_interface: source,
            bytes,
        }) else {
            panic!("the reference announce classifies");
        };
        identity_hash
    }

    #[test]
    fn blackholing_an_identity_drops_its_routes_and_blocks_new_announces_before_crypto() {
        let interfaces = transporting_interfaces();
        let source_interface = interfaces[0].id;
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let identity = identity_hash_from_announce(&mut raw, source_interface);
        let source = IdentityHash::new([0xC1; 16]);
        let mut engine = transporting_node();

        let IngestPacketOutcome::Announce(AnnounceIngest::Accepted(accepted)) = engine
            .ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface,
                    bytes: &mut raw,
                },
                &mut test_fill_entropy,
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            )
        else {
            panic!("the reference announce is accepted before its identity is blackholed");
        };
        assert_eq!(engine.route_count(), 1);
        assert_eq!(engine.scheduled_announce_count(), 1);
        let _ = engine.take_dirty_interfaces();

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            engine.blackhole_identity(
                BlackholedIdentity {
                    identity,
                    source,
                    expiry: BlackholeExpiry::Indefinite,
                    reason: Some("operator blocked"),
                },
                &mut |route| removed.push(route),
            ),
            Ok(BlackholeIdentityOutcome::Added),
        );
        assert_eq!(engine.blackholed_identity_count(), 1);
        assert!(engine.is_identity_blackholed(&identity));
        assert_eq!(
            engine.blackholed_identities().collect::<std::vec::Vec<_>>(),
            std::vec![BlackholedIdentity {
                identity,
                source,
                expiry: BlackholeExpiry::Indefinite,
                reason: Some("operator blocked"),
            }],
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: accepted.destination,
                receiving_interface: source_interface,
                cause: RouteRemovalCause::Dropped,
            }],
        );
        assert_eq!(engine.route_count(), 0);
        assert!(engine.take_dirty_interfaces().contains(&source_interface));
        assert!(tick_capture(
            &mut engine,
            InstantMillis(1_000_000),
            AttachedInterfaces::new(&interfaces),
        )
        .is_empty());

        let Some(signed_app_data_byte) = raw.last_mut() else {
            panic!("the reference announce carries signed app data");
        };
        *signed_app_data_byte ^= 1;
        let mut deferred = DeferredCrypto::default();
        assert_eq!(
            engine.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(2_000),
                    source_interface,
                    bytes: &mut raw,
                },
                &mut test_fill_entropy,
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                Some(&mut deferred),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Blackholed),
        );
        assert!(matches!(deferred, DeferredCrypto::Empty));
        assert_eq!(engine.route_count(), 0);
        assert_eq!(
            engine.unblackhole_identity(&identity),
            UnblackholeIdentityOutcome::Removed,
        );
        assert!(!engine.is_identity_blackholed(&identity));
    }

    #[test]
    fn a_blackhole_added_during_deferred_verification_wins_at_acceptance() {
        let interfaces = transporting_interfaces();
        let source_interface = interfaces[0].id;
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let identity = identity_hash_from_announce(&mut raw, source_interface);
        let mut engine = transporting_node();
        let mut deferred = DeferredCrypto::default();

        assert_eq!(
            engine.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface,
                    bytes: &mut raw,
                },
                &mut test_fill_entropy,
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                Some(&mut deferred),
            ),
            IngestPacketOutcome::OwesAnnounceVerify,
        );
        assert_eq!(
            engine.blackhole_identity(
                BlackholedIdentity {
                    identity,
                    source: IdentityHash::new([0xC2; 16]),
                    expiry: BlackholeExpiry::Indefinite,
                    reason: None,
                },
                &mut |_| {},
            ),
            Ok(BlackholeIdentityOutcome::Added),
        );
        let DeferredCrypto::AnnounceVerify(owed) = deferred else {
            panic!("the announce verification was deferred");
        };

        engine.resume_announce(
            owed,
            AttachedInterfaces::new(&interfaces),
            &mut test_fill_entropy,
            &mut |_| {},
        );
        assert_eq!(engine.route_count(), 0);
        assert_eq!(engine.scheduled_announce_count(), 0);
    }

    #[test]
    fn expiring_blackholes_wake_after_the_deadline_and_rearm_for_the_next_entry() {
        let mut engine = transporting_node();
        for (byte, expiry) in [
            (1, BlackholeExpiry::At(InstantMillis(100))),
            (2, BlackholeExpiry::At(InstantMillis(200))),
            (3, BlackholeExpiry::Indefinite),
        ] {
            assert_eq!(
                engine.blackhole_identity(
                    BlackholedIdentity {
                        identity: IdentityHash::new([byte; 16]),
                        source: IdentityHash::new([9; 16]),
                        expiry,
                        reason: None,
                    },
                    &mut |_| {},
                ),
                Ok(BlackholeIdentityOutcome::Added),
            );
        }

        assert_eq!(
            engine.blackhole_expiry_wake(),
            WakeSchedule::At(InstantMillis(101))
        );
        let unchanged = engine.cull_expired_blackholes(InstantMillis(100));
        assert_eq!(engine.blackholed_identity_count(), 3);
        assert_eq!(
            unchanged.expired_blackholes,
            WakeSchedule::At(InstantMillis(101))
        );

        let rearmed = engine.cull_expired_blackholes(InstantMillis(101));
        assert_eq!(engine.blackholed_identity_count(), 2);
        assert_eq!(
            rearmed.expired_blackholes,
            WakeSchedule::At(InstantMillis(201))
        );

        let cleared = engine.cull_expired_blackholes(InstantMillis(201));
        assert_eq!(engine.blackholed_identity_count(), 1);
        assert_eq!(cleared.expired_blackholes, WakeSchedule::Idle);
    }
}
