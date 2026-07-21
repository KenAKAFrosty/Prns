use crate::identity::IdentityHash;
use crate::interfaces::AttachedInterfaces;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::blackhole::BlackholeTable;
use crate::routing::{
    BlackholeIdentityOutcome, BlackholeInsertFailure, BlackholedIdentity, RemovedRoute,
    UnblackholeIdentityOutcome,
};
use crate::storage::{DirtyInterfaceSet, StorageLayout};

use super::{EngineState, WakeSchedules};

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct BlackholeIdentityEffect {
    pub outcome: Result<BlackholeIdentityOutcome, BlackholeInsertFailure>,
    pub wake_schedules: WakeSchedules,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct UnblackholeIdentityEffect {
    pub outcome: UnblackholeIdentityOutcome,
    pub wake_schedules: WakeSchedules,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BlackholeSeedReport {
    pub seeded_count: u32,
    pub refused_count: u32,
    pub dropped_count: u32,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct BlackholeSeedEffect {
    pub report: BlackholeSeedReport,
    pub wake_schedules: WakeSchedules,
}

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
        interfaces: AttachedInterfaces<'_>,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> BlackholeIdentityEffect {
        let identity = entry.identity;
        let outcome = self
            .identity_blackholes
            .blackhole_identity(entry)
            .map_err(<S::Blackholes as BlackholeTable>::classify_insert_error);
        if outcome != Ok(BlackholeIdentityOutcome::Added) {
            return BlackholeIdentityEffect {
                outcome,
                wake_schedules: WakeSchedules::UNCHANGED,
            };
        }
        let dirty = &mut self.dirty_interfaces;
        let scheduled_announces = &mut self.scheduled_announces;
        let dropped_routes =
            self.routing_table
                .drop_routes_for_identity(&identity, &mut |removed| {
                    let _ = scheduled_announces.cancel(&removed.destination);
                    dirty.mark(removed.receiving_interface);
                    on_removed(removed);
                });
        let mut wake_schedules = self.blackhole_mutation_wake(interfaces);
        if dropped_routes == 0 {
            wake_schedules.scheduled_announces = crate::engine::WakeSchedule::Unchanged;
        }
        BlackholeIdentityEffect {
            outcome,
            wake_schedules,
        }
    }

    pub fn unblackhole_identity(&mut self, identity: &IdentityHash) -> UnblackholeIdentityEffect {
        let outcome = self.identity_blackholes.unblackhole_identity(identity);
        let wake_schedules = if outcome == UnblackholeIdentityOutcome::Removed {
            WakeSchedules {
                expired_blackholes: self.blackhole_expiry_wake(),
                ..WakeSchedules::UNCHANGED
            }
        } else {
            WakeSchedules::UNCHANGED
        };
        UnblackholeIdentityEffect {
            outcome,
            wake_schedules,
        }
    }

    pub fn seed_blackholed_identities<Reason: AsRef<str>>(
        &mut self,
        entries: impl IntoIterator<Item = BlackholedIdentity<Reason>>,
        now: crate::units::InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> BlackholeSeedEffect {
        let mut report = BlackholeSeedReport::default();
        let mut wake_schedules = WakeSchedules::UNCHANGED;
        for entry in entries {
            if entry.expiry.is_expired_at(now) {
                report.refused_count += 1;
                continue;
            }
            let effect = self.blackhole_identity(
                BlackholedIdentity {
                    identity: entry.identity,
                    source: entry.source,
                    expiry: entry.expiry,
                    reason: entry.reason.as_ref().map(AsRef::as_ref),
                },
                interfaces,
                on_removed,
            );
            wake_schedules.merge(effect.wake_schedules);
            match effect.outcome {
                Ok(BlackholeIdentityOutcome::Added) => report.seeded_count += 1,
                Ok(BlackholeIdentityOutcome::AlreadyPresent)
                | Err(
                    BlackholeInsertFailure::CapacityExhausted
                    | BlackholeInsertFailure::ReasonTooLong,
                ) => report.dropped_count += 1,
            }
        }
        BlackholeSeedEffect {
            report,
            wake_schedules,
        }
    }

    pub fn cull_expired_blackholes(&mut self, now: crate::units::InstantMillis) -> WakeSchedules {
        self.identity_blackholes.cull_expired(now);
        WakeSchedules {
            expired_blackholes: self.blackhole_expiry_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    fn blackhole_mutation_wake(&self, interfaces: AttachedInterfaces<'_>) -> WakeSchedules {
        let mut wake_schedules = self.route_removal_wake_schedules(interfaces);
        wake_schedules.expired_blackholes = self.blackhole_expiry_wake();
        wake_schedules
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{
        bytes_from_hex, test_fill_entropy, tick_capture, transporting_interfaces,
        transporting_node, TestStorageLayout, RNS_1_3_5_ANNOUNCE,
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

    fn entry(identity_byte: u8, reason: Option<&'static str>) -> BlackholedIdentity<&'static str> {
        BlackholedIdentity {
            identity: IdentityHash::new([identity_byte; 16]),
            source: IdentityHash::new([0x41; 16]),
            expiry: BlackholeExpiry::At(InstantMillis(1_500)),
            reason,
        }
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
        let effect = engine.blackhole_identity(
            BlackholedIdentity {
                identity,
                source,
                expiry: BlackholeExpiry::Indefinite,
                reason: Some("operator blocked"),
            },
            AttachedInterfaces::new(&interfaces),
            &mut |route| removed.push(route),
        );
        assert_eq!(effect.outcome, Ok(BlackholeIdentityOutcome::Added));
        assert_eq!(effect.wake_schedules.expired_routes, WakeSchedule::Idle);
        assert_eq!(
            effect.wake_schedules.scheduled_announces,
            WakeSchedule::Idle
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
        assert_eq!(engine.scheduled_announce_count(), 0);
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
            engine.unblackhole_identity(&identity).outcome,
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
            engine
                .blackhole_identity(
                    BlackholedIdentity {
                        identity,
                        source: IdentityHash::new([0xC2; 16]),
                        expiry: BlackholeExpiry::Indefinite,
                        reason: None,
                    },
                    AttachedInterfaces::new(&interfaces),
                    &mut |_| {},
                )
                .outcome,
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
                engine
                    .blackhole_identity(
                        BlackholedIdentity {
                            identity: IdentityHash::new([byte; 16]),
                            source: IdentityHash::new([9; 16]),
                            expiry,
                            reason: None,
                        },
                        AttachedInterfaces::new(&[]),
                        &mut |_| {},
                    )
                    .outcome,
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

    #[test]
    fn mutations_return_their_outcome_with_the_complete_wake_delta() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        assert_eq!(
            engine.blackhole_identity(
                entry(0x31, Some("operator")),
                AttachedInterfaces::new(&[]),
                &mut |_| {},
            ),
            BlackholeIdentityEffect {
                outcome: Ok(BlackholeIdentityOutcome::Added),
                wake_schedules: WakeSchedules {
                    expired_routes: WakeSchedule::Idle,
                    expired_destination_identities: WakeSchedule::Idle,
                    expired_blackholes: WakeSchedule::At(InstantMillis(1_501)),
                    ..WakeSchedules::UNCHANGED
                },
            }
        );
        assert_eq!(
            engine.unblackhole_identity(&IdentityHash::new([0x31; 16])),
            UnblackholeIdentityEffect {
                outcome: UnblackholeIdentityOutcome::Removed,
                wake_schedules: WakeSchedules {
                    expired_blackholes: WakeSchedule::Idle,
                    ..WakeSchedules::UNCHANGED
                },
            }
        );
        assert_eq!(
            engine.unblackhole_identity(&IdentityHash::new([0x31; 16])),
            UnblackholeIdentityEffect {
                outcome: UnblackholeIdentityOutcome::NotFound,
                wake_schedules: WakeSchedules::UNCHANGED,
            }
        );
    }

    #[test]
    fn storage_rejections_are_typed_and_leave_schedules_unchanged() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        assert_eq!(
            engine.blackhole_identity(
                entry(
                    0x31,
                    Some("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
                ),
                AttachedInterfaces::new(&[]),
                &mut |_| {},
            ),
            BlackholeIdentityEffect {
                outcome: Err(BlackholeInsertFailure::ReasonTooLong),
                wake_schedules: WakeSchedules::UNCHANGED,
            }
        );

        for identity_byte in 0..8 {
            assert_eq!(
                engine
                    .blackhole_identity(
                        entry(identity_byte, None),
                        AttachedInterfaces::new(&[]),
                        &mut |_| {},
                    )
                    .outcome,
                Ok(BlackholeIdentityOutcome::Added)
            );
        }
        assert_eq!(
            engine
                .blackhole_identity(entry(0x80, None), AttachedInterfaces::new(&[]), &mut |_| {},),
            BlackholeIdentityEffect {
                outcome: Err(BlackholeInsertFailure::CapacityExhausted),
                wake_schedules: WakeSchedules::UNCHANGED,
            }
        );
    }

    #[test]
    fn seeding_classifies_entries_and_returns_the_final_wake_delta() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let effect = engine.seed_blackholed_identities(
            [
                entry(0x31, Some("active")),
                entry(0x31, Some("duplicate")),
                BlackholedIdentity {
                    expiry: BlackholeExpiry::At(InstantMillis(999)),
                    ..entry(0x32, Some("expired"))
                },
                entry(
                    0x33,
                    Some("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
                ),
            ],
            InstantMillis(1_000),
            AttachedInterfaces::new(&[]),
            &mut |_| {},
        );

        assert_eq!(
            effect,
            BlackholeSeedEffect {
                report: BlackholeSeedReport {
                    seeded_count: 1,
                    refused_count: 1,
                    dropped_count: 2,
                },
                wake_schedules: WakeSchedules {
                    expired_routes: WakeSchedule::Idle,
                    expired_destination_identities: WakeSchedule::Idle,
                    expired_blackholes: WakeSchedule::At(InstantMillis(1_501)),
                    ..WakeSchedules::UNCHANGED
                },
            }
        );
        assert_eq!(engine.blackholed_identity_count(), 1);
    }
}
