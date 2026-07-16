use crate::identity::known::{KnownDestination, RememberKnownDestinationError};
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
pub(crate) enum RememberAnnouncedDestinationOutcome {
    Remembered,
    PublicKeyChanged,
    CapacityExhausted,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn known_destination_count(&self) -> usize {
        self.known_destinations.len()
    }

    pub fn known_destination(&self, destination: &DestinationHash) -> Option<KnownDestination<'_>> {
        self.known_destinations.get(destination)
    }

    pub fn known_destinations(&self) -> impl Iterator<Item = KnownDestination<'_>> + '_ {
        self.known_destinations.rows()
    }

    pub fn mark_destination_used(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
    ) -> MarkDestinationUsedOutcome {
        self.known_destinations.mark_used(destination, now)
    }

    pub fn retain_destination(
        &mut self,
        destination: &DestinationHash,
    ) -> RetainDestinationOutcome {
        self.known_destinations.retain(destination)
    }

    pub fn release_destination(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
    ) -> ReleaseDestinationOutcome {
        self.known_destinations.release(destination, now)
    }

    pub fn retain_identity(&mut self, identity: &IdentityHash) -> RetainIdentityOutcome {
        self.known_destinations.retain_identity(identity)
    }

    pub fn cull_expired_known_destinations(&mut self, now: InstantMillis) -> WakeSchedules {
        let routing_table = &self.routing_table;
        self.known_destinations
            .cull_expired(now, |destination| routing_table.has_route(destination));
        WakeSchedules {
            expired_known_destinations: self.known_destination_expiry_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    pub(crate) fn remember_announced_destination(
        &mut self,
        announce: &Announce<'_>,
        announced_at: InstantMillis,
    ) -> RememberAnnouncedDestinationOutcome {
        loop {
            match self.known_destinations.remember(
                announce.destination,
                announce.public_keys,
                announce.app_data,
                announced_at,
            ) {
                Ok(_) => return RememberAnnouncedDestinationOutcome::Remembered,
                Err(RememberKnownDestinationError::PublicKeyChanged) => {
                    return RememberAnnouncedDestinationOutcome::PublicKeyChanged;
                }
                Err(
                    RememberKnownDestinationError::TableFull
                    | RememberKnownDestinationError::AppDataFull,
                ) => {
                    let routing_table = &self.routing_table;
                    if !self
                        .known_destinations
                        .evict_oldest_unretained_without_path(|destination| {
                            routing_table.has_route(destination)
                        })
                    {
                        return RememberAnnouncedDestinationOutcome::CapacityExhausted;
                    }
                }
            }
        }
    }

    pub(crate) fn known_destination_expiry(
        &self,
        destination: &DestinationHash,
    ) -> Option<InstantMillis> {
        self.known_destinations.expiry_at(destination)
    }

    pub(crate) fn unprotected_known_destination_expiry(
        &self,
        destination: &DestinationHash,
    ) -> Option<InstantMillis> {
        if self.routing_table.has_route(destination) {
            None
        } else {
            self.known_destination_expiry(destination)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{
        bytes_from_hex, test_fill_entropy, transporting_interfaces, transporting_node,
        RNS_1_3_5_ANNOUNCE,
    };
    use crate::engine::{AnnounceIngest, IngestPacketOutcome, WakeSchedule};
    use crate::identity::known::{
        KnownDestinationRetentionState, UNUSED_DESTINATION_LINGER_MILLIS,
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
        let mut wire = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        assert!(matches!(
            engine.ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0xA1; 8]),
                    bytes: &mut wire,
                },
                &mut test_fill_entropy,
                AttachedInterfaces::new(&interfaces),
                &mut |_| {},
                None,
            ),
            IngestPacketOutcome::Announce(AnnounceIngest::Accepted(_)),
        ));
    }

    #[test]
    fn a_verified_announce_populates_identity_memory_independently_of_its_route() {
        let mut engine = transporting_node();
        hear_reference_announce(&mut engine);

        let known = engine.known_destination(&DESTINATION).unwrap();
        assert_eq!(known.destination, DESTINATION);
        assert_eq!(known.announced_at, InstantMillis(1_000));
        assert_eq!(known.retention, KnownDestinationRetentionState::NeverUsed);
        assert_eq!(known.app_data, b"hello-personal");

        assert!(engine.drop_route(&DESTINATION).is_some());
        assert_eq!(engine.route_count(), 0);
        assert_eq!(engine.known_destination_count(), 1);
        assert_eq!(
            engine.known_destination_expiry_wake(),
            WakeSchedule::At(InstantMillis(1_000 + UNUSED_DESTINATION_LINGER_MILLIS + 1)),
        );

        engine.cull_expired_known_destinations(InstantMillis(
            1_000 + UNUSED_DESTINATION_LINGER_MILLIS,
        ));
        assert_eq!(engine.known_destination_count(), 1);
        engine.cull_expired_known_destinations(InstantMillis(
            1_000 + UNUSED_DESTINATION_LINGER_MILLIS + 1,
        ));
        assert_eq!(engine.known_destination_count(), 0);
    }

    #[test]
    fn retention_and_release_preserve_the_reference_lifecycle() {
        let mut engine = transporting_node();
        hear_reference_announce(&mut engine);
        let identity = engine.known_destination(&DESTINATION).unwrap().identity;
        assert!(engine.drop_route(&DESTINATION).is_some());

        assert_eq!(
            engine.retain_identity(&identity),
            RetainIdentityOutcome {
                newly_retained_destination_count: 1,
                already_retained_destination_count: 0,
            },
        );
        assert_eq!(
            engine.mark_destination_used(&DESTINATION, InstantMillis(5_000)),
            MarkDestinationUsedOutcome::Retained,
        );
        assert_eq!(engine.known_destination_expiry_wake(), WakeSchedule::Idle);
        engine.cull_expired_known_destinations(InstantMillis(u64::MAX));
        assert_eq!(engine.known_destination_count(), 1);

        assert_eq!(
            engine.release_destination(&DESTINATION, InstantMillis(10_000)),
            ReleaseDestinationOutcome::Released,
        );
        assert_eq!(
            engine.known_destination_expiry_wake(),
            WakeSchedule::At(InstantMillis(10_000 + USED_DESTINATION_LINGER_MILLIS + 1)),
        );
        engine.cull_expired_known_destinations(InstantMillis(
            10_000 + USED_DESTINATION_LINGER_MILLIS + 1,
        ));
        assert_eq!(engine.known_destination_count(), 0);
    }
}
