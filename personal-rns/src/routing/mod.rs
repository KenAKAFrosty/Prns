pub mod announce;
pub mod defaults;
pub mod delivery;
pub mod held_cache;
pub mod schedule;
pub mod storage;
pub mod types;

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::wire::DestinationHash;
use announce::Announce;
pub use defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
use defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
pub use storage::AnnounceIdHistoryView;
use storage::{
    AnnounceIdHistory, ColumnsFull, RetainedAnnounceColumns, RetainedAnnounceEntry,
    RetainedAppData, RouteColumns, RouteEntry,
};
pub use types::{
    DropCause, ExistingRoute, RetainedAnnounce, RouteResponsiveness, UpsertRouteOutcome,
};

/// `PartialEq` compares backend representation byte-for-byte because the
/// determinism tests rely on that.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoutingTable<R, A, H, D>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    routes: R,
    retained_announces: A,
    announce_id_history: H,
    retained_app_data: D,
}

impl<R, A, H, D> RoutingTable<R, A, H, D>
where
    R: RouteColumns,
    A: RetainedAnnounceColumns,
    H: AnnounceIdHistory,
    D: RetainedAppData,
{
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    pub fn route_count_via(&self, interface: InterfaceId) -> usize {
        self.routes
            .receiving_interfaces()
            .iter()
            .filter(|&&learned_on| learned_on == interface)
            .count()
    }

    pub fn hop_count_to(&self, destination: &DestinationHash) -> Option<u8> {
        self.index_of(destination).map(|i| self.routes.hops()[i])
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.routes
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)
    }

    pub fn existing_route_for(&self, destination: &DestinationHash) -> Option<ExistingRoute<'_>> {
        let i = self.index_of(destination)?;
        Some(ExistingRoute {
            hops: self.routes.hops()[i],
            expires: self.routes.expires()[i],
            announce_id_history: self.announce_id_history.history(i),
            responsiveness: self.routes.responsiveness()[i],
        })
    }

    pub fn upsert_route(
        &mut self,
        hops: u8,
        arrived_at: InstantMillis,
        receiving_interface: InterfaceId,
        announce: &Announce<'_>,
    ) -> UpsertRouteOutcome {
        let expires_at = InstantMillis(arrived_at.0.saturating_add(DEFAULT_ROUTE_EXPIRY_MILLIS));
        match self.index_of(&announce.destination) {
            None => self.insert_new_route(hops, expires_at, receiving_interface, announce),
            Some(i) => {
                self.refresh_existing_route(i, hops, expires_at, receiving_interface, announce)
            }
        }
    }

    fn insert_new_route(
        &mut self,
        hops: u8,
        expires_at: InstantMillis,
        receiving_interface: InterfaceId,
        announce: &Announce<'_>,
    ) -> UpsertRouteOutcome {
        if self.routes.len() >= self.routes.capacity() {
            return UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull);
        }
        let Ok(handle) = self.retained_app_data.insert(announce.app_data) else {
            return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
        };
        let route_entry = RouteEntry {
            hops,
            expires: expires_at,
            responsiveness: RouteResponsiveness::Responsive,
            receiving_interface,
        };
        let announce_entry = RetainedAnnounceEntry {
            public_keys: announce.public_keys,
            dotted_name_hash: announce.dotted_name_hash,
            retained_announce_id: announce.announce_id,
            maybe_ratchet: announce.maybe_ratchet,
            signature: announce.signature,
            maybe_app_data_handle: Some(handle),
        };
        let routes_slot = match self.routes.push(announce.destination, route_entry) {
            Ok(i) => i,
            Err(ColumnsFull) => {
                return UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull);
            }
        };
        let _ = self.retained_announces.push(announce_entry);
        self.announce_id_history
            .remember(routes_slot, announce.announce_id);
        UpsertRouteOutcome::Inserted
    }

    fn refresh_existing_route(
        &mut self,
        i: usize,
        hops: u8,
        expires_at: InstantMillis,
        receiving_interface: InterfaceId,
        announce: &Announce<'_>,
    ) -> UpsertRouteOutcome {
        let Some(handle) = self.retained_announces.app_data_handle()[i] else {
            debug_assert!(false, "existing destination missing app_data handle");
            return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
        };
        if self
            .retained_app_data
            .replace(handle, announce.app_data)
            .is_err()
        {
            return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
        }

        self.routes.set_row(
            i,
            RouteEntry {
                hops,
                expires: expires_at,
                responsiveness: RouteResponsiveness::Responsive,
                receiving_interface,
            },
        );
        self.retained_announces.set_row(
            i,
            RetainedAnnounceEntry {
                public_keys: announce.public_keys,
                dotted_name_hash: announce.dotted_name_hash,
                retained_announce_id: announce.announce_id,
                maybe_ratchet: announce.maybe_ratchet,
                signature: announce.signature,
                maybe_app_data_handle: Some(handle),
            },
        );
        self.announce_id_history.remember(i, announce.announce_id);
        UpsertRouteOutcome::Updated
    }

    pub fn app_data_for(&self, destination: &DestinationHash) -> Option<&[u8]> {
        Some(self.retained_announce_for(destination)?.announce.app_data)
    }

    pub fn retained_announce_for(
        &self,
        destination: &DestinationHash,
    ) -> Option<RetainedAnnounce<'_>> {
        let i = self.index_of(destination)?;
        let handle = self.retained_announces.app_data_handle()[i]?;
        let app_data = self.retained_app_data.get(handle);
        Some(RetainedAnnounce {
            hops: self.routes.hops()[i],
            announce: Announce {
                destination: self.routes.destinations()[i],
                public_keys: self.retained_announces.public_keys()[i],
                dotted_name_hash: self.retained_announces.dotted_name_hash()[i],
                announce_id: self.retained_announces.retained_announce_id()[i],
                maybe_ratchet: self.retained_announces.ratchet()[i],
                signature: self.retained_announces.signature()[i],
                app_data,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
    use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
    use crate::routing::storage::{
        FixedArrayRetainedAnnounceColumns, FixedArrayRouteColumns, PackedAppDataArena,
        TieredAnnounceIdHistory,
    };

    type TestRoutingTable<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
        const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
        const HISTORY_FLOOR_PER_DESTINATION: usize,
        const HISTORY_OVERFLOW_CAPACITY: usize,
    > = RoutingTable<
        FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>,
        FixedArrayRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS>,
        TieredAnnounceIdHistory<
            HISTORY_FLOOR_PER_DESTINATION,
            HISTORY_OVERFLOW_CAPACITY,
            MAX_TRACKED_DESTINATIONS,
            MAX_ANNOUNCE_IDS_PER_DESTINATION,
        >,
        PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>,
    >;
    type Rt = TestRoutingTable<64, 64, 4096, 4, 512>;
    const RT_HISTORY_CAP: usize = 64;
    use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
    }

    fn source() -> InterfaceId {
        iface(0xEE)
    }

    fn announce_id(nonce_byte: u8, timebase: u64) -> AnnounceId {
        let mut bytes = [0u8; 10];
        bytes[..5].copy_from_slice(&[nonce_byte; 5]);
        bytes[5..].copy_from_slice(&timebase.to_be_bytes()[3..]);
        AnnounceId::from_wire(bytes)
    }

    fn app_data(tag: u8) -> [u8; 16] {
        [tag; 16]
    }

    fn announce_for<'a>(
        destination: DestinationHash,
        announce_id: AnnounceId,
        ratchet: Option<RatchetKey>,
        app_data: &'a [u8],
    ) -> Announce<'a> {
        Announce {
            destination,
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            },
            dotted_name_hash: DottedNameHash::new([0u8; 10]),
            announce_id,
            maybe_ratchet: ratchet,
            signature: Ed25519Signature([0u8; 64]),
            app_data,
        }
    }

    fn record<const D: usize, const S: usize, const A: usize, const F: usize, const O: usize>(
        table: &mut TestRoutingTable<D, S, A, F, O>,
        destination: DestinationHash,
        hops: u8,
        arrival: InstantMillis,
        announce_id: AnnounceId,
        app_data: &[u8],
    ) -> UpsertRouteOutcome {
        table.upsert_route(
            hops,
            arrival,
            source(),
            &announce_for(destination, announce_id, None, app_data),
        )
    }

    #[test]
    fn first_record_creates_a_path() {
        let mut table: Rt = Rt::default();
        assert_eq!(
            record(
                &mut table,
                dest(1),
                2,
                InstantMillis(100),
                announce_id(0xAA, 1),
                &app_data(0xAA)
            ),
            UpsertRouteOutcome::Inserted
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.hop_count_to(&dest(1)), Some(2));
        assert_eq!(table.hop_count_to(&dest(2)), None);
    }

    #[test]
    fn route_count_via_attributes_destinations_to_the_receiving_interface() {
        let mut table: Rt = Rt::default();
        let wifi = iface(0x01);
        let usb = iface(0x02);
        let silent = iface(0x03);

        for (dest_byte, id_byte, learned_on) in
            [(1u8, 0xA1u8, wifi), (2, 0xA2, wifi), (3, 0xA3, usb)]
        {
            assert_eq!(
                table.upsert_route(
                    1,
                    InstantMillis(100),
                    learned_on,
                    &announce_for(
                        dest(dest_byte),
                        announce_id(id_byte, 1),
                        None,
                        &app_data(id_byte)
                    ),
                ),
                UpsertRouteOutcome::Inserted
            );
        }

        assert_eq!(table.route_count(), 3);
        assert_eq!(table.route_count_via(wifi), 2);
        assert_eq!(table.route_count_via(usb), 1);
        assert_eq!(table.route_count_via(silent), 0);

        assert_eq!(
            table.upsert_route(
                1,
                InstantMillis(200),
                usb,
                &announce_for(dest(1), announce_id(0xB1, 2), None, &app_data(0xB1)),
            ),
            UpsertRouteOutcome::Updated
        );
        assert_eq!(table.route_count(), 3);
        assert_eq!(table.route_count_via(wifi), 1);
        assert_eq!(table.route_count_via(usb), 2);
    }

    #[test]
    fn refresh_updates_in_place_and_remembers_distinct_ids() {
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            4,
            InstantMillis(100),
            announce_id(0xAA, 1),
            &app_data(0xAA),
        );
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(200),
            announce_id(0xBB, 2),
            &app_data(0xBB),
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.hop_count_to(&dest(1)), Some(2));

        let view = table.existing_route_for(&dest(1)).unwrap();
        assert_eq!(view.announce_id_history.len(), 2);
    }

    #[test]
    fn re_recording_the_same_id_does_not_duplicate_it() {
        let mut table: Rt = Rt::default();
        let id = announce_id(0xAA, 1);
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(100),
            id,
            &app_data(0xAA),
        );
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(150),
            id,
            &app_data(0xAA),
        );
        assert_eq!(
            table
                .existing_route_for(&dest(1))
                .unwrap()
                .announce_id_history
                .len(),
            1
        );
    }

    #[test]
    fn seen_set_evicts_oldest_when_full() {
        let mut table: Rt = Rt::default();
        for n in 0..(RT_HISTORY_CAP as u64 + 3) {
            record(
                &mut table,
                dest(1),
                1,
                InstantMillis(n),
                announce_id(0, n),
                &app_data(0),
            );
        }
        let view = table.existing_route_for(&dest(1)).unwrap();
        assert_eq!(view.announce_id_history.len(), RT_HISTORY_CAP);
        assert!(!view.announce_id_history.contains(&announce_id(0, 0)));
        assert!(view
            .announce_id_history
            .contains(&announce_id(0, RT_HISTORY_CAP as u64 + 2)));
    }

    #[test]
    fn new_destinations_past_capacity_are_dropped() {
        const MAX: usize = 8;
        let mut table: TestRoutingTable<MAX, 8, 256, 4, 512> = TestRoutingTable::default();
        for n in 0..MAX {
            assert_eq!(
                record(
                    &mut table,
                    dest(n as u8),
                    1,
                    InstantMillis(0),
                    announce_id(0, n as u64),
                    &app_data(n as u8)
                ),
                UpsertRouteOutcome::Inserted
            );
        }
        assert_eq!(table.route_count(), MAX);
        assert_eq!(
            record(
                &mut table,
                dest(0xFF),
                1,
                InstantMillis(0),
                announce_id(0, 999),
                &app_data(0xFF)
            ),
            UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull)
        );
        assert_eq!(table.route_count(), MAX);
        assert_eq!(
            record(
                &mut table,
                dest(0),
                1,
                InstantMillis(1),
                announce_id(1, 1),
                &app_data(0)
            ),
            UpsertRouteOutcome::Updated
        );
    }

    #[test]
    fn record_retains_the_payload_and_refresh_replaces_it() {
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(100),
            announce_id(0xAA, 1),
            &[1, 2, 3],
        );
        assert_eq!(table.app_data_for(&dest(1)), Some(&[1, 2, 3][..]));
        assert_eq!(table.app_data_for(&dest(2)), None);

        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(200),
            announce_id(0xBB, 2),
            &[9, 9, 9, 9, 9],
        );
        assert_eq!(table.app_data_for(&dest(1)), Some(&[9, 9, 9, 9, 9][..]));
    }

    #[test]
    fn distinct_destinations_retain_independent_payloads() {
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(0),
            announce_id(1, 1),
            &[0xA1; 4],
        );
        record(
            &mut table,
            dest(2),
            1,
            InstantMillis(0),
            announce_id(2, 1),
            &[0xB2; 7],
        );
        record(
            &mut table,
            dest(3),
            1,
            InstantMillis(0),
            announce_id(3, 1),
            &[0xC3; 2],
        );
        assert_eq!(table.app_data_for(&dest(1)), Some(&[0xA1; 4][..]));
        assert_eq!(table.app_data_for(&dest(2)), Some(&[0xB2; 7][..]));
        assert_eq!(table.app_data_for(&dest(3)), Some(&[0xC3; 2][..]));
    }

    #[test]
    fn a_new_path_whose_payload_overflows_the_arena_is_dropped() {
        let mut table: TestRoutingTable<4, 8, 8, 4, 512> = TestRoutingTable::default();
        assert_eq!(
            record(
                &mut table,
                dest(1),
                1,
                InstantMillis(0),
                announce_id(1, 1),
                &[0xAA; 8]
            ),
            UpsertRouteOutcome::Inserted
        );
        assert_eq!(
            record(
                &mut table,
                dest(2),
                1,
                InstantMillis(0),
                announce_id(2, 1),
                &[0xBB; 1]
            ),
            UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull)
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.hop_count_to(&dest(2)), None);
    }

    #[test]
    fn refresh_that_cannot_retain_a_better_announce_leaves_the_table_untouched() {
        let mut table: TestRoutingTable<4, 8, 8, 4, 512> = TestRoutingTable::default();
        record(
            &mut table,
            dest(1),
            5,
            InstantMillis(0),
            announce_id(1, 1),
            &[0xAA; 6],
        );
        assert_eq!(table.app_data_for(&dest(1)), Some(&[0xAA; 6][..]));

        let outcome = record(
            &mut table,
            dest(1),
            2,
            InstantMillis(1),
            announce_id(2, 2),
            &[0xBB; 9],
        );
        assert_eq!(
            outcome,
            UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull)
        );
        assert_eq!(table.hop_count_to(&dest(1)), Some(5));
        assert_eq!(table.app_data_for(&dest(1)), Some(&[0xAA; 6][..]));
    }

    #[test]
    fn ratchet_is_retained_for_faithful_rebroadcast() {
        let mut table: Rt = Rt::default();
        let ratchet = Some(RatchetKey::new([0xFE; 32]));
        let body = app_data(0xAA);
        table.upsert_route(
            3,
            InstantMillis(0),
            source(),
            &announce_for(dest(1), announce_id(0xAA, 1), ratchet, &body),
        );
        let retained = table.retained_announce_for(&dest(1)).unwrap();
        assert_eq!(retained.announce.maybe_ratchet, ratchet);
        assert_eq!(retained.hops, 3);
        assert_eq!(retained.announce.app_data, &body[..]);

        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(1),
            announce_id(0xBB, 2),
            &app_data(0xBB),
        );
        let retained = table.retained_announce_for(&dest(1)).unwrap();
        assert_eq!(retained.announce.maybe_ratchet, None);
        assert_eq!(retained.hops, 2);

        assert!(table.retained_announce_for(&dest(2)).is_none());
    }
}
