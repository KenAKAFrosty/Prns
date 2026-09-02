use crate::identity::destination_identity::{
    DestinationIdentity, DestinationIdentitySeed, RememberDestinationIdentityError,
    RememberDestinationIdentityOutcome,
};
use crate::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
    RetainIdentityOutcome,
};
use crate::routing::announce::Announce;
use crate::storage::StorageLayout;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

use super::{EngineState, WakeSchedules};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RememberAnnouncedDestinationIdentityOutcome {
    Remembered,
    PublicKeyChanged,
    CapacityExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationIdentitySeedOutcome {
    Seeded,
    Replaced,
    Expired,
    RefusedPublicKeyChanged,
    CapacityExhausted,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct DestinationIdentityRetentionEffect<Outcome> {
    outcome: Outcome,
    wake_schedules: WakeSchedules,
}

impl<Outcome: Copy> DestinationIdentityRetentionEffect<Outcome> {
    pub fn outcome(&self) -> Outcome {
        self.outcome
    }
}

impl<Outcome> DestinationIdentityRetentionEffect<Outcome> {
    pub fn wake_schedules(&self) -> WakeSchedules {
        self.wake_schedules
    }
}

pub type MarkDestinationUsedEffect = DestinationIdentityRetentionEffect<MarkDestinationUsedOutcome>;
pub type RetainDestinationEffect = DestinationIdentityRetentionEffect<RetainDestinationOutcome>;
pub type ReleaseDestinationEffect = DestinationIdentityRetentionEffect<ReleaseDestinationOutcome>;
pub type RetainIdentityEffect = DestinationIdentityRetentionEffect<RetainIdentityOutcome>;

enum DestinationIdentityRetentionMutation {
    Changed,
    Unchanged,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn destination_identity_count(&self) -> usize {
        self.destination_identities.len()
    }

    pub fn destination_identity(
        &self,
        destination: &DestinationHash,
    ) -> Option<DestinationIdentity<'_>> {
        self.destination_identities.get(destination)
    }

    pub fn destination_identities(&self) -> impl Iterator<Item = DestinationIdentity<'_>> + '_ {
        self.destination_identities.rows()
    }

    pub fn mark_destination_used(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
    ) -> MarkDestinationUsedEffect {
        let outcome = self.destination_identities.mark_used(destination, now);
        let mutation = match outcome {
            MarkDestinationUsedOutcome::Recorded | MarkDestinationUsedOutcome::Refreshed => {
                DestinationIdentityRetentionMutation::Changed
            }
            MarkDestinationUsedOutcome::Retained | MarkDestinationUsedOutcome::NotFound => {
                DestinationIdentityRetentionMutation::Unchanged
            }
        };
        self.destination_identity_retention_effect(outcome, mutation)
    }

    pub fn retain_destination(&mut self, destination: &DestinationHash) -> RetainDestinationEffect {
        let outcome = self.destination_identities.retain(destination);
        let mutation = match outcome {
            RetainDestinationOutcome::Retained => DestinationIdentityRetentionMutation::Changed,
            RetainDestinationOutcome::AlreadyRetained | RetainDestinationOutcome::NotFound => {
                DestinationIdentityRetentionMutation::Unchanged
            }
        };
        self.destination_identity_retention_effect(outcome, mutation)
    }

    pub fn release_destination(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
    ) -> ReleaseDestinationEffect {
        let outcome = self.destination_identities.release(destination, now);
        let mutation = match outcome {
            ReleaseDestinationOutcome::Released
            | ReleaseDestinationOutcome::UseRecorded
            | ReleaseDestinationOutcome::UseRefreshed => {
                DestinationIdentityRetentionMutation::Changed
            }
            ReleaseDestinationOutcome::NotFound => DestinationIdentityRetentionMutation::Unchanged,
        };
        self.destination_identity_retention_effect(outcome, mutation)
    }

    pub fn retain_identity(&mut self, identity: &IdentityHash) -> RetainIdentityEffect {
        let outcome = self.destination_identities.retain_identity(identity);
        let mutation = if outcome.newly_retained_destination_count == 0 {
            DestinationIdentityRetentionMutation::Unchanged
        } else {
            DestinationIdentityRetentionMutation::Changed
        };
        self.destination_identity_retention_effect(outcome, mutation)
    }

    pub fn seed_destination_identity(
        &mut self,
        identity: DestinationIdentitySeed<'_>,
        now: InstantMillis,
    ) -> DestinationIdentitySeedOutcome {
        let outcome = loop {
            match self.destination_identities.restore(
                identity.destination,
                identity.public_keys,
                identity.app_data,
                identity.announced_at,
                identity.retention,
            ) {
                Ok(RememberDestinationIdentityOutcome::Remembered) => {
                    break DestinationIdentitySeedOutcome::Seeded;
                }
                Ok(RememberDestinationIdentityOutcome::Refreshed) => {
                    break DestinationIdentitySeedOutcome::Replaced;
                }
                Err(RememberDestinationIdentityError::PublicKeyChanged) => {
                    return DestinationIdentitySeedOutcome::RefusedPublicKeyChanged;
                }
                Err(
                    RememberDestinationIdentityError::TableFull
                    | RememberDestinationIdentityError::AppDataFull,
                ) => {
                    let routing_table = &self.routing_table;
                    if !self
                        .destination_identities
                        .evict_oldest_unretained_without_path(|destination| {
                            routing_table.has_route(destination)
                        })
                    {
                        return DestinationIdentitySeedOutcome::CapacityExhausted;
                    }
                }
            }
        };
        let routing_table = &self.routing_table;
        let removed = self
            .destination_identities
            .cull_expired(now, |destination| {
                *destination != identity.destination || routing_table.has_route(destination)
            });
        if removed == 0 {
            outcome
        } else {
            DestinationIdentitySeedOutcome::Expired
        }
    }

    pub fn cull_expired_destination_identities(&mut self, now: InstantMillis) -> WakeSchedules {
        let routing_table = &self.routing_table;
        self.destination_identities
            .cull_expired(now, |destination| routing_table.has_route(destination));
        WakeSchedules {
            expired_destination_identities: self.destination_identity_expiry_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    pub(crate) fn remember_announced_destination_identity(
        &mut self,
        announce: &Announce<'_>,
        announced_at: InstantMillis,
    ) -> RememberAnnouncedDestinationIdentityOutcome {
        loop {
            match self.destination_identities.remember(
                announce.destination,
                announce.public_keys,
                announce.app_data,
                announced_at,
            ) {
                Ok(_) => return RememberAnnouncedDestinationIdentityOutcome::Remembered,
                Err(RememberDestinationIdentityError::PublicKeyChanged) => {
                    return RememberAnnouncedDestinationIdentityOutcome::PublicKeyChanged;
                }
                Err(
                    RememberDestinationIdentityError::TableFull
                    | RememberDestinationIdentityError::AppDataFull,
                ) => {
                    let routing_table = &self.routing_table;
                    if !self
                        .destination_identities
                        .evict_oldest_unretained_without_path(|destination| {
                            routing_table.has_route(destination)
                        })
                    {
                        return RememberAnnouncedDestinationIdentityOutcome::CapacityExhausted;
                    }
                }
            }
        }
    }

    pub(crate) fn destination_identity_expiry(
        &self,
        destination: &DestinationHash,
    ) -> Option<InstantMillis> {
        self.destination_identities.expiry_at(destination)
    }

    pub(crate) fn unprotected_destination_identity_expiry(
        &self,
        destination: &DestinationHash,
    ) -> Option<InstantMillis> {
        if self.routing_table.has_route(destination) {
            None
        } else {
            self.destination_identity_expiry(destination)
        }
    }

    fn destination_identity_retention_effect<Outcome>(
        &self,
        outcome: Outcome,
        mutation: DestinationIdentityRetentionMutation,
    ) -> DestinationIdentityRetentionEffect<Outcome> {
        DestinationIdentityRetentionEffect {
            outcome,
            wake_schedules: match mutation {
                DestinationIdentityRetentionMutation::Changed => WakeSchedules {
                    expired_destination_identities: self.destination_identity_expiry_wake(),
                    ..WakeSchedules::UNCHANGED
                },
                DestinationIdentityRetentionMutation::Unchanged => WakeSchedules::UNCHANGED,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{
        bytes_from_hex, transporting_interfaces, transporting_node, RNS_1_4_2_ANNOUNCE,
    };
    use crate::engine::DropRouteOutcome;
    use crate::engine::{AnnounceIngest, IngestPacketOutcome, WakeSchedule};
    use crate::identity::destination_identity::{
        DestinationIdentityRetentionState, UNUSED_DESTINATION_LINGER_MILLIS,
        USED_DESTINATION_LINGER_MILLIS,
    };
    use crate::interfaces::{AttachedInterfaces, InboundPacket, InterfaceId};

    const DESTINATION: DestinationHash = DestinationHash::new([
        0x16, 0xf8, 0xa6, 0xd3, 0xf7, 0xd7, 0xc5, 0xb6, 0xf1, 0x06, 0xd2, 0x93, 0x80, 0x4d, 0x73,
        0x14,
    ]);

    fn hear_reference_announce(
        engine: &mut EngineState<crate::engine::test_support::TestStorageLayout>,
    ) {
        let interfaces = transporting_interfaces();
        let mut wire = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        assert!(matches!(
            engine.ingest_for_test(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0xA1; 8]),
                    bytes: &mut wire,
                },
                AttachedInterfaces::new(&interfaces),
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
        ));
    }

    #[test]
    fn a_verified_announce_populates_identity_memory_independently_of_its_route() {
        let mut engine = transporting_node();
        hear_reference_announce(&mut engine);

        let identity = engine.destination_identity(&DESTINATION).unwrap();
        assert_eq!(identity.destination, DESTINATION);
        assert_eq!(identity.announced_at, InstantMillis(1_000));
        assert_eq!(
            identity.retention,
            DestinationIdentityRetentionState::NeverUsed
        );
        assert_eq!(identity.app_data, b"hello-personal");

        assert_eq!(
            engine
                .drop_route(
                    &DESTINATION,
                    AttachedInterfaces::new(&transporting_interfaces()),
                )
                .outcome(),
            DropRouteOutcome::Dropped,
        );
        assert_eq!(engine.route_count(), 0);
        assert_eq!(engine.destination_identity_count(), 1);
        assert_eq!(
            engine.destination_identity_expiry_wake(),
            WakeSchedule::At(InstantMillis(1_000 + UNUSED_DESTINATION_LINGER_MILLIS + 1)),
        );

        engine.cull_expired_destination_identities(InstantMillis(
            1_000 + UNUSED_DESTINATION_LINGER_MILLIS,
        ));
        assert_eq!(engine.destination_identity_count(), 1);
        engine.cull_expired_destination_identities(InstantMillis(
            1_000 + UNUSED_DESTINATION_LINGER_MILLIS + 1,
        ));
        assert_eq!(engine.destination_identity_count(), 0);
    }

    #[test]
    fn retention_and_release_preserve_the_reference_lifecycle() {
        let mut engine = transporting_node();
        hear_reference_announce(&mut engine);
        let identity = engine.destination_identity(&DESTINATION).unwrap().identity;
        assert_eq!(
            engine
                .drop_route(
                    &DESTINATION,
                    AttachedInterfaces::new(&transporting_interfaces()),
                )
                .outcome(),
            DropRouteOutcome::Dropped,
        );

        let retain = engine.retain_identity(&identity);
        assert_eq!(
            retain.outcome(),
            RetainIdentityOutcome {
                newly_retained_destination_count: 1,
                already_retained_destination_count: 0,
            },
        );
        assert_eq!(
            retain.wake_schedules().expired_destination_identities,
            WakeSchedule::Idle,
        );
        let used = engine.mark_destination_used(&DESTINATION, InstantMillis(5_000));
        assert_eq!(used.outcome(), MarkDestinationUsedOutcome::Retained);
        assert_eq!(used.wake_schedules(), WakeSchedules::UNCHANGED);
        assert_eq!(
            engine.destination_identity_expiry_wake(),
            WakeSchedule::Idle
        );
        engine.cull_expired_destination_identities(InstantMillis(u64::MAX));
        assert_eq!(engine.destination_identity_count(), 1);

        let release = engine.release_destination(&DESTINATION, InstantMillis(10_000));
        assert_eq!(release.outcome(), ReleaseDestinationOutcome::Released);
        assert_eq!(
            release.wake_schedules().expired_destination_identities,
            WakeSchedule::At(InstantMillis(10_000 + USED_DESTINATION_LINGER_MILLIS + 1)),
        );
        engine.cull_expired_destination_identities(InstantMillis(
            10_000 + USED_DESTINATION_LINGER_MILLIS + 1,
        ));
        assert_eq!(engine.destination_identity_count(), 0);
    }

    #[test]
    fn retention_mutations_return_only_the_wake_delta_they_change() {
        let mut engine = transporting_node();
        hear_reference_announce(&mut engine);
        let identity = engine.destination_identity(&DESTINATION).unwrap().identity;
        assert_eq!(
            engine
                .drop_route(
                    &DESTINATION,
                    AttachedInterfaces::new(&transporting_interfaces()),
                )
                .outcome(),
            DropRouteOutcome::Dropped,
        );

        let recorded = engine.mark_destination_used(&DESTINATION, InstantMillis(5_000));
        assert_eq!(recorded.outcome(), MarkDestinationUsedOutcome::Recorded);
        assert_eq!(
            recorded.wake_schedules().expired_destination_identities,
            WakeSchedule::At(InstantMillis(5_000 + USED_DESTINATION_LINGER_MILLIS + 1)),
        );

        let refreshed = engine.mark_destination_used(&DESTINATION, InstantMillis(6_000));
        assert_eq!(refreshed.outcome(), MarkDestinationUsedOutcome::Refreshed);
        assert_eq!(
            refreshed.wake_schedules().expired_destination_identities,
            WakeSchedule::At(InstantMillis(6_000 + USED_DESTINATION_LINGER_MILLIS + 1)),
        );

        let retained = engine.retain_destination(&DESTINATION);
        assert_eq!(retained.outcome(), RetainDestinationOutcome::Retained);
        assert_eq!(
            retained.wake_schedules().expired_destination_identities,
            WakeSchedule::Idle,
        );
        let already_retained = engine.retain_destination(&DESTINATION);
        assert_eq!(
            already_retained.outcome(),
            RetainDestinationOutcome::AlreadyRetained,
        );
        assert_eq!(already_retained.wake_schedules(), WakeSchedules::UNCHANGED);

        let released = engine.release_destination(&DESTINATION, InstantMillis(7_000));
        assert_eq!(released.outcome(), ReleaseDestinationOutcome::Released);
        assert_eq!(
            released.wake_schedules().expired_destination_identities,
            WakeSchedule::At(InstantMillis(7_000 + USED_DESTINATION_LINGER_MILLIS + 1)),
        );

        let identity_retained = engine.retain_identity(&identity);
        assert_eq!(
            identity_retained.outcome(),
            RetainIdentityOutcome {
                newly_retained_destination_count: 1,
                already_retained_destination_count: 0,
            },
        );
        assert_eq!(
            identity_retained
                .wake_schedules()
                .expired_destination_identities,
            WakeSchedule::Idle,
        );
        let identity_already_retained = engine.retain_identity(&identity);
        assert_eq!(
            identity_already_retained.outcome(),
            RetainIdentityOutcome {
                newly_retained_destination_count: 0,
                already_retained_destination_count: 1,
            },
        );
        assert_eq!(
            identity_already_retained.wake_schedules(),
            WakeSchedules::UNCHANGED,
        );
    }

    #[test]
    fn seed_observes_collision_expiry_and_route_protection() {
        let mut routed = transporting_node();
        hear_reference_announce(&mut routed);
        let heard = routed.destination_identity(&DESTINATION).unwrap();
        let row = crate::identity::destination_identity::DestinationIdentitySeed {
            destination: heard.destination,
            public_keys: heard.public_keys,
            announced_at: heard.announced_at,
            retention: heard.retention,
            app_data: b"hello-personal",
        };
        assert_eq!(
            routed.seed_destination_identity(
                row,
                InstantMillis(1_001 + UNUSED_DESTINATION_LINGER_MILLIS),
            ),
            DestinationIdentitySeedOutcome::Replaced,
        );
        assert_eq!(routed.destination_identity_count(), 1);

        let mut expired = transporting_node();
        assert_eq!(
            expired.seed_destination_identity(
                row,
                InstantMillis(1_001 + UNUSED_DESTINATION_LINGER_MILLIS),
            ),
            DestinationIdentitySeedOutcome::Expired,
        );
        assert_eq!(expired.destination_identity_count(), 0);

        let mut retained = row;
        retained.retention = DestinationIdentityRetentionState::Retained;
        assert_eq!(
            expired.seed_destination_identity(retained, InstantMillis(u64::MAX)),
            DestinationIdentitySeedOutcome::Seeded,
        );
        assert_eq!(expired.destination_identity_count(), 1);

        let mut changed = retained;
        changed.public_keys.signing = crate::identity::IdentitySigningPublicKey::new(
            crate::crypto::Ed25519PublicKey([0x7f; 32]),
        );
        assert_eq!(
            expired.seed_destination_identity(changed, InstantMillis(u64::MAX)),
            DestinationIdentitySeedOutcome::RefusedPublicKeyChanged,
        );
        assert_eq!(
            expired
                .destination_identity(&DESTINATION)
                .unwrap()
                .public_keys,
            retained.public_keys,
        );
    }
}
