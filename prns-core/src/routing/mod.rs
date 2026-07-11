pub mod announce;
pub mod dedup;
pub mod delivery;
pub mod group_keys;
pub mod ingress;
pub mod lemire_index;
pub mod links;
pub mod path_requests;
pub mod proof;
pub mod request_handlers;
pub mod reverse_routes;
pub mod routes;
pub mod tunnel;
pub mod types;
pub mod upstream_app_destinations;
pub mod warmth;

use crate::engine::InstantMillis;
use crate::interfaces::{descriptor_for, InterfaceDescriptor, InterfaceId};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;
use announce::defaults::route_expiry_millis;
use announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecord, AnnounceRecordTable};
use announce::Announce;
pub use announce::AnnounceArrival;
use routes::{RouteEntry, RouteTable};
pub use types::{
    DropCause, ExistingRoute, ForwardingRoute, NextHop, RemovedRoute, RouteRemovalCause,
    RouteResponsiveness, StoredAnnounce, UpsertRouteOutcome,
};
pub use upstream_app_destinations::{
    ProofStrategy, RegisterDestinationError, UpstreamAppDestination, UpstreamAppDestinationKind,
    UpstreamAppDestinationTable,
};
use warmth::RouteWarmth;

/// `PartialEq` compares backend representation byte-for-byte because the
/// determinism tests rely on that.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoutingTable<R, A, H, D>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
{
    routes: R,
    announce_records: A,
    announce_id_history: H,
    announce_app_data: D,
}

impl<R, A, H, D> RoutingTable<R, A, H, D>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
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

    pub fn has_route(&self, destination: &DestinationHash) -> bool {
        self.index_of(destination).is_some()
    }

    pub fn responsiveness_of(&self, destination: &DestinationHash) -> Option<RouteResponsiveness> {
        self.index_of(destination)
            .map(|i| self.routes.responsiveness()[i])
    }

    pub fn path_rows(&self) -> impl Iterator<Item = (DestinationHash, RouteEntry)> + '_ {
        let routes = &self.routes;
        (0..routes.len()).map(move |i| {
            (
                routes.destinations()[i],
                RouteEntry {
                    hops: routes.hops()[i],
                    learned_at: routes.learned_at()[i],
                    responsiveness: routes.responsiveness()[i],
                    receiving_interface: routes.receiving_interfaces()[i],
                    next_hop: routes.next_hops()[i],
                    last_relayed_at: routes.last_relayed_at()[i],
                },
            )
        })
    }

    /// RNS's `Transport.next_hop`.
    pub fn path_row(&self, destination: &DestinationHash) -> Option<RouteEntry> {
        let i = self.index_of(destination)?;
        Some(RouteEntry {
            hops: self.routes.hops()[i],
            learned_at: self.routes.learned_at()[i],
            responsiveness: self.routes.responsiveness()[i],
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
            last_relayed_at: self.routes.last_relayed_at()[i],
        })
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.routes.index_of(destination)
    }

    pub fn existing_route_for(
        &self,
        destination: &DestinationHash,
        interfaces: &[InterfaceDescriptor],
    ) -> Option<ExistingRoute<'_>> {
        let i = self.index_of(destination)?;
        Some(ExistingRoute {
            hops: crate::units::HopCount(self.routes.hops()[i]),
            expires: self.expiry_of(i, interfaces),
            announce_id_history: self.announce_id_history.history(i),
            responsiveness: self.routes.responsiveness()[i],
        })
    }

    /// RNS folds learn and relay into one path-table TIMESTAMP (Transport.py:1638);
    /// we keep them apart and recombine here, so an actively-carried route never
    /// ages out mid-flow while its announces lull.
    fn last_active_at(&self, i: usize) -> InstantMillis {
        InstantMillis(
            self.routes.learned_at()[i]
                .0
                .max(self.routes.last_relayed_at()[i].0),
        )
    }

    /// Expiry is derived at evaluation, never stored: a hot-changed mode re-keys every
    /// route at the next evaluation, and a route whose interface is no longer attached is
    /// already due.
    fn expiry_of(&self, i: usize, interfaces: &[InterfaceDescriptor]) -> InstantMillis {
        self.expiry_of_with(i, interfaces, &())
    }

    fn expiry_of_with(
        &self,
        i: usize,
        interfaces: &[InterfaceDescriptor],
        warmth: &dyn RouteWarmth,
    ) -> InstantMillis {
        let last_active_at = self.last_active_at(i);
        let receiving_interface = self.routes.receiving_interfaces()[i];
        match descriptor_for(interfaces, receiving_interface) {
            Some(descriptor) => InstantMillis(
                last_active_at
                    .0
                    .saturating_add(route_expiry_millis(descriptor.mode)),
            ),
            None => warmth
                .warm_until(receiving_interface)
                .unwrap_or(last_active_at),
        }
    }

    pub fn forwarding_route_for(&self, destination: &DestinationHash) -> Option<ForwardingRoute> {
        let i = self.index_of(destination)?;
        Some(ForwardingRoute {
            hops: crate::units::HopCount(self.routes.hops()[i]),
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
        })
    }

    pub fn mark_responsiveness(
        &mut self,
        destination: &DestinationHash,
        responsiveness: RouteResponsiveness,
    ) {
        let Some(i) = self.index_of(destination) else {
            return;
        };
        self.routes.set_row(
            i,
            RouteEntry {
                hops: self.routes.hops()[i],
                learned_at: self.routes.learned_at()[i],
                last_relayed_at: self.routes.last_relayed_at()[i],
                responsiveness,
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
            },
        );
    }

    /// RNS bumps the path-table TIMESTAMP on every forwarded packet (Transport.py:1638).
    pub fn note_relayed(&mut self, destination: &DestinationHash, now: InstantMillis) {
        let Some(i) = self.index_of(destination) else {
            return;
        };
        self.routes.set_row(
            i,
            RouteEntry {
                hops: self.routes.hops()[i],
                learned_at: self.routes.learned_at()[i],
                last_relayed_at: now,
                responsiveness: self.routes.responsiveness()[i],
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
            },
        );
    }

    pub fn repoint_routes(
        &mut self,
        previous: InterfaceId,
        current: InterfaceId,
        now: InstantMillis,
    ) -> usize {
        let mut moved = 0;
        for i in 0..self.routes.len() {
            if self.routes.receiving_interfaces()[i] != previous {
                continue;
            }
            self.routes.set_row(
                i,
                RouteEntry {
                    hops: self.routes.hops()[i],
                    learned_at: self.routes.learned_at()[i],
                    last_relayed_at: now,
                    responsiveness: self.routes.responsiveness()[i],
                    receiving_interface: current,
                    next_hop: self.routes.next_hops()[i],
                },
            );
            moved += 1;
        }
        moved
    }

    pub fn upsert_route(
        &mut self,
        arrival: &AnnounceArrival<'_>,
        interfaces: &[InterfaceDescriptor],
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> UpsertRouteOutcome {
        self.upsert_route_with_warmth(arrival, interfaces, &(), on_removed)
    }

    pub fn upsert_route_with_warmth(
        &mut self,
        arrival: &AnnounceArrival<'_>,
        interfaces: &[InterfaceDescriptor],
        warmth: &dyn RouteWarmth,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> UpsertRouteOutcome {
        match self.index_of(&arrival.announce.destination) {
            None => {
                if self.routes.len() >= self.routes.capacity() {
                    self.cull_expired_routes_with_warmth(
                        arrival.arrived_at,
                        interfaces,
                        warmth,
                        on_removed,
                    );
                    if self.routes.len() >= self.routes.capacity() {
                        self.evict_route_nearest_expiry(interfaces, warmth, on_removed);
                    }
                }
                self.insert_new_route(arrival, interfaces, warmth, on_removed)
            }
            Some(i) => self.refresh_existing_route(i, arrival),
        }
    }

    fn evict_route_nearest_expiry(
        &mut self,
        interfaces: &[InterfaceDescriptor],
        warmth: &dyn RouteWarmth,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> bool {
        let Some(i) =
            (0..self.routes.len()).min_by_key(|&i| self.expiry_of_with(i, interfaces, warmth))
        else {
            return false;
        };
        on_removed(RemovedRoute {
            destination: self.routes.destinations()[i],
            receiving_interface: self.routes.receiving_interfaces()[i],
            cause: RouteRemovalCause::Evicted,
        });
        self.remove_route(i);
        true
    }

    fn insert_new_route(
        &mut self,
        arrival: &AnnounceArrival<'_>,
        interfaces: &[InterfaceDescriptor],
        warmth: &dyn RouteWarmth,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> UpsertRouteOutcome {
        let &AnnounceArrival {
            ref announce,
            hops,
            arrived_at,
            receiving_interface,
            next_hop,
            ..
        } = arrival;
        if self.routes.len() >= self.routes.capacity() {
            return UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull);
        }
        let handle = match self.announce_app_data.insert(announce.app_data) {
            Ok(handle) => handle,
            Err(_) => {
                if !self.evict_route_nearest_expiry(interfaces, warmth, on_removed) {
                    return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
                }
                match self.announce_app_data.insert(announce.app_data) {
                    Ok(handle) => handle,
                    Err(_) => return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull),
                }
            }
        };
        let route_entry = RouteEntry {
            hops,
            learned_at: arrived_at,
            last_relayed_at: InstantMillis(0),
            responsiveness: RouteResponsiveness::Unknown,
            receiving_interface,
            next_hop,
        };
        let announce_entry = AnnounceRecord {
            public_keys: announce.public_keys,
            dotted_name_hash: announce.dotted_name_hash,
            announce_id: announce.announce_id,
            ratchet: announce.ratchet,
            signature: announce.signature,
            maybe_app_data_handle: Some(handle),
        };
        let routes_slot = match self.routes.push(announce.destination, route_entry) {
            Ok(i) => i,
            Err(TablePushError::TableFull) => {
                return UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull);
            }
        };
        let _ = self.announce_records.push(announce_entry);
        self.announce_id_history
            .remember(routes_slot, announce.announce_id);
        UpsertRouteOutcome::Inserted
    }

    fn refresh_existing_route(
        &mut self,
        i: usize,
        arrival: &AnnounceArrival<'_>,
    ) -> UpsertRouteOutcome {
        let &AnnounceArrival {
            ref announce,
            hops,
            arrived_at,
            receiving_interface,
            next_hop,
            ..
        } = arrival;
        let Some(handle) = self.announce_records.app_data_handles()[i] else {
            debug_assert!(false, "existing destination missing app_data handle");
            return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
        };
        if self
            .announce_app_data
            .replace(handle, announce.app_data)
            .is_err()
        {
            return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
        }

        self.routes.set_row(
            i,
            RouteEntry {
                hops,
                learned_at: arrived_at,
                last_relayed_at: InstantMillis(0),
                responsiveness: RouteResponsiveness::Unknown,
                receiving_interface,
                next_hop,
            },
        );
        self.announce_records.set_row(
            i,
            AnnounceRecord {
                public_keys: announce.public_keys,
                dotted_name_hash: announce.dotted_name_hash,
                announce_id: announce.announce_id,
                ratchet: announce.ratchet,
                signature: announce.signature,
                maybe_app_data_handle: Some(handle),
            },
        );
        self.announce_id_history.remember(i, announce.announce_id);
        UpsertRouteOutcome::Updated
    }

    pub fn remove_route(&mut self, i: usize) {
        let last = self.routes.len() - 1;
        let freed = self.announce_records.app_data_handles()[i];
        if let Some(handle) = freed {
            self.announce_app_data.free(handle);
        }
        self.routes.swap_remove(i, last);
        self.announce_records.swap_remove(i, last);
        self.announce_id_history.swap_remove(i, last);
    }

    /// Boundary-inclusive: a deadline must be actionable at its own instant or a reactor waking exactly at `expires` busy-spins.
    /// The reference culls on a 5s float-time poll, so the boundary is unobservable to parity.
    pub fn cull_expired_routes(
        &mut self,
        now: InstantMillis,
        interfaces: &[InterfaceDescriptor],
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> usize {
        self.cull_expired_routes_with_warmth(now, interfaces, &(), on_removed)
    }

    pub fn cull_expired_routes_with_warmth(
        &mut self,
        now: InstantMillis,
        interfaces: &[InterfaceDescriptor],
        warmth: &dyn RouteWarmth,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> usize {
        let mut culled = 0;
        let mut i = 0;
        while i < self.routes.len() {
            if now >= self.expiry_of_with(i, interfaces, warmth) {
                let receiving_interface = self.routes.receiving_interfaces()[i];
                let cause = if interfaces
                    .iter()
                    .any(|descriptor| descriptor.id == receiving_interface)
                {
                    RouteRemovalCause::Expired
                } else {
                    RouteRemovalCause::InterfaceGone
                };
                on_removed(RemovedRoute {
                    destination: self.routes.destinations()[i],
                    receiving_interface,
                    cause,
                });
                self.remove_route(i);
                culled += 1;
            } else {
                i += 1;
            }
        }
        culled
    }

    pub fn soonest_route_expiry(
        &self,
        interfaces: &[InterfaceDescriptor],
    ) -> Option<InstantMillis> {
        self.soonest_route_expiry_with_warmth(interfaces, &())
    }

    pub fn soonest_route_expiry_with_warmth(
        &self,
        interfaces: &[InterfaceDescriptor],
        warmth: &dyn RouteWarmth,
    ) -> Option<InstantMillis> {
        (0..self.routes.len())
            .map(|i| self.expiry_of_with(i, interfaces, warmth))
            .min()
    }

    pub fn app_data_for(&self, destination: &DestinationHash) -> Option<&[u8]> {
        Some(self.stored_announce_for(destination)?.announce.app_data)
    }

    pub fn stored_announce_for(&self, destination: &DestinationHash) -> Option<StoredAnnounce<'_>> {
        let i = self.index_of(destination)?;
        let handle = self.announce_records.app_data_handles()[i]?;
        let app_data = self.announce_app_data.get(handle);
        Some(StoredAnnounce {
            hops: self.routes.hops()[i],
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
            announce: Announce {
                destination: self.routes.destinations()[i],
                public_keys: self.announce_records.public_keys()[i],
                dotted_name_hash: self.announce_records.dotted_name_hashes()[i],
                announce_id: self.announce_records.announce_ids()[i],
                ratchet: self.announce_records.ratchets()[i],
                signature: self.announce_records.signatures()[i],
                app_data,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
    use crate::engine::test_support::routable_descriptor;
    use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
    use crate::interfaces::InterfaceMode;
    use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
    use crate::routing::announce::stored::{
        FixedAnnounceIdHistory, FixedArrayAnnounceRecordTable, PackedAppDataArena,
    };
    use crate::routing::routes::FixedArrayRouteTable;

    type TestRoutingTable<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
        const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
    > = RoutingTable<
        FixedArrayRouteTable<MAX_TRACKED_DESTINATIONS>,
        FixedArrayAnnounceRecordTable<MAX_TRACKED_DESTINATIONS>,
        FixedAnnounceIdHistory<MAX_TRACKED_DESTINATIONS, MAX_ANNOUNCE_IDS_PER_DESTINATION>,
        PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>,
    >;
    type Rt = TestRoutingTable<64, 64, 4096>;
    const RT_HISTORY_CAP: usize = 64;
    use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
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
            ratchet,
            signature: Ed25519Signature([0u8; 64]),
            app_data,
        }
    }

    fn full_interfaces() -> [InterfaceDescriptor; 1] {
        [routable_descriptor(source())]
    }

    fn view_with(mode: InterfaceMode) -> [InterfaceDescriptor; 1] {
        [InterfaceDescriptor {
            mode,
            ..routable_descriptor(source())
        }]
    }

    fn record<const D: usize, const S: usize, const A: usize>(
        table: &mut TestRoutingTable<D, S, A>,
        destination: DestinationHash,
        hops: u8,
        arrival: InstantMillis,
        announce_id: AnnounceId,
        app_data: &[u8],
    ) -> UpsertRouteOutcome {
        table.upsert_route(
            &AnnounceArrival {
                announce: announce_for(destination, announce_id, None, app_data),
                hops,
                arrived_at: arrival,
                receiving_interface: source(),
                next_hop: NextHop::Direct,
                is_path_response: false,
            },
            &full_interfaces(),
            &mut |_| {},
        )
    }

    #[test]
    fn route_expiry_is_derived_from_the_mode_the_view_carries_now() {
        use crate::routing::announce::defaults::{
            ACCESS_POINT_ROUTE_EXPIRY_MILLIS, ROAMING_ROUTE_EXPIRY_MILLIS,
        };
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(1_000),
            announce_id(1, 1),
            &app_data(1),
        );

        for (mode, lifetime) in [
            (InterfaceMode::Full, DEFAULT_ROUTE_EXPIRY_MILLIS),
            (InterfaceMode::AccessPoint, ACCESS_POINT_ROUTE_EXPIRY_MILLIS),
            (InterfaceMode::Roaming, ROAMING_ROUTE_EXPIRY_MILLIS),
        ] {
            assert_eq!(
                table
                    .existing_route_for(&dest(1), &view_with(mode))
                    .unwrap()
                    .expires,
                InstantMillis(1_000 + lifetime),
                "the same stored route re-keys to {mode:?} the moment the attached interfaces say so",
            );
        }
    }

    #[test]
    fn a_refresh_restarts_the_lifetime_from_its_own_arrival() {
        use crate::routing::announce::defaults::ROAMING_ROUTE_EXPIRY_MILLIS;
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(1_000),
            announce_id(1, 1),
            &app_data(1),
        );
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(2_000),
            announce_id(1, 2),
            &app_data(1),
        );
        assert_eq!(
            table
                .existing_route_for(&dest(1), &full_interfaces())
                .unwrap()
                .expires,
            InstantMillis(2_000 + DEFAULT_ROUTE_EXPIRY_MILLIS),
            "the refresh restarts the clock",
        );
        assert_eq!(
            table
                .existing_route_for(&dest(1), &view_with(InterfaceMode::Roaming))
                .unwrap()
                .expires,
            InstantMillis(2_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            "and the lifetime still follows whatever mode the attached interface carries",
        );
    }

    #[test]
    fn a_relay_slides_a_routes_expiry_forward_so_it_survives_mid_flow() {
        use crate::routing::announce::defaults::ROAMING_ROUTE_EXPIRY_MILLIS;
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(1_000),
            announce_id(1, 1),
            &app_data(1),
        );
        let roaming = view_with(InterfaceMode::Roaming);

        assert_eq!(
            table
                .existing_route_for(&dest(1), &roaming)
                .unwrap()
                .expires,
            InstantMillis(1_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            "the announce sets the baseline expiry clock",
        );

        table.note_relayed(&dest(1), InstantMillis(1_000_000));
        assert_eq!(
            table
                .existing_route_for(&dest(1), &roaming)
                .unwrap()
                .expires,
            InstantMillis(1_000_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            "relaying across the route restarts the clock from the last carried packet",
        );

        let mut removed = std::vec::Vec::new();
        table.cull_expired_routes(
            InstantMillis(1_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            &roaming,
            &mut |r| removed.push(r.destination),
        );
        assert!(
            removed.is_empty(),
            "the route the announce alone would have culled survives, because traffic still flows across it",
        );

        table.cull_expired_routes(
            InstantMillis(1_000_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            &roaming,
            &mut |r| removed.push(r.destination),
        );
        assert_eq!(
            removed,
            std::vec![dest(1)],
            "and it still ages out a full lifetime after its last relay",
        );
    }

    #[test]
    fn an_announce_refresh_clears_prior_relay_activity() {
        use crate::routing::announce::defaults::ROAMING_ROUTE_EXPIRY_MILLIS;
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(1_000),
            announce_id(1, 1),
            &app_data(1),
        );
        let roaming = view_with(InterfaceMode::Roaming);

        table.note_relayed(&dest(1), InstantMillis(500_000));
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(2_000),
            announce_id(1, 2),
            &app_data(1),
        );
        assert_eq!(
            table.existing_route_for(&dest(1), &roaming).unwrap().expires,
            InstantMillis(2_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            "a fresh announce supersedes prior relay activity, exactly as RNS overwrites the path TIMESTAMP",
        );
    }

    #[test]
    fn eviction_prefers_a_newer_roaming_route_over_an_older_full_one() {
        const MAX: usize = 2;
        let mut table: TestRoutingTable<MAX, 8, 256> = TestRoutingTable::default();
        let full_interface = iface(0xA1);
        let roaming_interface = iface(0xB2);
        let two_mode_interfaces = [
            routable_descriptor(full_interface),
            InterfaceDescriptor {
                mode: InterfaceMode::Roaming,
                ..routable_descriptor(roaming_interface)
            },
        ];
        for (dest_byte, arrival, learned_on) in
            [(1u8, 0u64, full_interface), (2, 1_000, roaming_interface)]
        {
            assert_eq!(
                table.upsert_route(
                    &AnnounceArrival {
                        announce: announce_for(
                            dest(dest_byte),
                            announce_id(dest_byte, 1),
                            None,
                            &app_data(dest_byte)
                        ),
                        hops: 1,
                        arrived_at: InstantMillis(arrival),
                        receiving_interface: learned_on,
                        next_hop: NextHop::Direct,
                        is_path_response: false,
                    },
                    &two_mode_interfaces,
                    &mut |_| {},
                ),
                UpsertRouteOutcome::Inserted
            );
        }

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(3), announce_id(3, 1), None, &app_data(3)),
                    hops: 1,
                    arrived_at: InstantMillis(2_000),
                    receiving_interface: full_interface,
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                &two_mode_interfaces,
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(2),
                receiving_interface: roaming_interface,
                cause: RouteRemovalCause::Evicted,
            }],
            "the roaming route expires in six hours, nearer death than the full one with a week to live",
        );
        assert_eq!(table.hop_count_to(&dest(1)), Some(1));
        assert_eq!(table.hop_count_to(&dest(3)), Some(1));
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
                    &AnnounceArrival {
                        announce: announce_for(
                            dest(dest_byte),
                            announce_id(id_byte, 1),
                            None,
                            &app_data(id_byte)
                        ),
                        hops: 1,
                        arrived_at: InstantMillis(100),
                        receiving_interface: learned_on,
                        next_hop: NextHop::Direct,
                        is_path_response: false,
                    },
                    &full_interfaces(),
                    &mut |_| {},
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
                &AnnounceArrival {
                    announce: announce_for(dest(1), announce_id(0xB1, 2), None, &app_data(0xB1)),
                    hops: 1,
                    arrived_at: InstantMillis(200),
                    receiving_interface: usb,
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                &full_interfaces(),
                &mut |_| {},
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

        let route = table
            .existing_route_for(&dest(1), &full_interfaces())
            .unwrap();
        assert_eq!(route.announce_id_history.len(), 2);
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
                .existing_route_for(&dest(1), &full_interfaces())
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
        let route = table
            .existing_route_for(&dest(1), &full_interfaces())
            .unwrap();
        assert_eq!(route.announce_id_history.len(), RT_HISTORY_CAP);
        assert!(!route.announce_id_history.contains(&announce_id(0, 0)));
        assert!(route
            .announce_id_history
            .contains(&announce_id(0, RT_HISTORY_CAP as u64 + 2)));
    }

    #[test]
    fn a_full_table_of_fresh_routes_evicts_the_one_nearest_expiry_for_a_newcomer() {
        const MAX: usize = 8;
        let mut table: TestRoutingTable<MAX, 8, 256> = TestRoutingTable::default();
        for n in 1..=MAX {
            assert_eq!(
                record(
                    &mut table,
                    dest(n as u8),
                    1,
                    InstantMillis(n as u64 * 10),
                    announce_id(0, n as u64),
                    &app_data(n as u8)
                ),
                UpsertRouteOutcome::Inserted
            );
        }
        assert_eq!(table.route_count(), MAX);

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(0xFF), announce_id(0, 999), None, &app_data(0xFF)),
                    hops: 1,
                    arrived_at: InstantMillis(100),
                    receiving_interface: source(),
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                &full_interfaces(),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
            "a full table of fresh routes admits the newcomer by eviction",
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(1),
                receiving_interface: source(),
                cause: RouteRemovalCause::Evicted,
            }],
            "the victim is the earliest arrival — the route nearest its expiry",
        );
        assert_eq!(table.route_count(), MAX);
        assert_eq!(table.hop_count_to(&dest(1)), None);
        assert_eq!(table.hop_count_to(&dest(0xFF)), Some(1));
        assert_eq!(
            record(
                &mut table,
                dest(2),
                1,
                InstantMillis(101),
                announce_id(1, 1),
                &app_data(2)
            ),
            UpsertRouteOutcome::Updated,
            "refreshing a survivor needs no slot",
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
    fn a_new_path_that_overflows_the_arena_evicts_the_route_nearest_expiry() {
        let mut table: TestRoutingTable<4, 8, 8> = TestRoutingTable::default();
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

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(2), announce_id(2, 1), None, &[0xBB; 1]),
                    hops: 1,
                    arrived_at: InstantMillis(10),
                    receiving_interface: source(),
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                &full_interfaces(),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
            "arena pressure evicts to admit the newcomer",
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(1),
                receiving_interface: source(),
                cause: RouteRemovalCause::Evicted,
            }],
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.hop_count_to(&dest(1)), None);
        assert_eq!(table.app_data_for(&dest(2)), Some(&[0xBB; 1][..]));
    }

    #[test]
    fn an_oversized_newcomer_takes_one_eviction_per_attempt_until_it_fits() {
        let mut table: TestRoutingTable<4, 8, 8> = TestRoutingTable::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(10),
            announce_id(1, 1),
            &[0xA1; 3],
        );
        record(
            &mut table,
            dest(2),
            1,
            InstantMillis(20),
            announce_id(2, 1),
            &[0xB2; 3],
        );

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(3), announce_id(3, 1), None, &[0xC3; 8]),
                    hops: 1,
                    arrived_at: InstantMillis(30),
                    receiving_interface: source(),
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                &full_interfaces(),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull),
            "one eviction was not enough, so this attempt drops",
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(1),
                receiving_interface: source(),
                cause: RouteRemovalCause::Evicted,
            }],
            "each attempt evicts at most one victim",
        );

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(3), announce_id(3, 2), None, &[0xC3; 8]),
                    hops: 1,
                    arrived_at: InstantMillis(40),
                    receiving_interface: source(),
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                &full_interfaces(),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
            "the retransmitted announce finds the room the first attempt made",
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(2),
                receiving_interface: source(),
                cause: RouteRemovalCause::Evicted,
            }],
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.app_data_for(&dest(3)), Some(&[0xC3; 8][..]));
    }

    #[test]
    fn refresh_that_cannot_retain_a_better_announce_leaves_the_table_untouched() {
        let mut table: TestRoutingTable<4, 8, 8> = TestRoutingTable::default();
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
            &AnnounceArrival {
                announce: announce_for(dest(1), announce_id(0xAA, 1), ratchet, &body),
                hops: 3,
                arrived_at: InstantMillis(0),
                receiving_interface: source(),
                next_hop: NextHop::Direct,
                is_path_response: false,
            },
            &full_interfaces(),
            &mut |_| {},
        );
        let stored = table.stored_announce_for(&dest(1)).unwrap();
        assert_eq!(stored.announce.ratchet, ratchet);
        assert_eq!(stored.hops, 3);
        assert_eq!(stored.announce.app_data, &body[..]);

        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(1),
            announce_id(0xBB, 2),
            &app_data(0xBB),
        );
        let stored = table.stored_announce_for(&dest(1)).unwrap();
        assert_eq!(stored.announce.ratchet, None);
        assert_eq!(stored.hops, 2);

        assert!(table.stored_announce_for(&dest(2)).is_none());
    }

    #[test]
    fn remove_route_drops_a_destination_and_keeps_the_rest_aligned() {
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(100),
            announce_id(0xA1, 1),
            &app_data(0x11),
        );
        record(
            &mut table,
            dest(2),
            2,
            InstantMillis(100),
            announce_id(0xA2, 1),
            &app_data(0x22),
        );
        record(
            &mut table,
            dest(3),
            3,
            InstantMillis(100),
            announce_id(0xA3, 1),
            &app_data(0x33),
        );
        assert_eq!(table.route_count(), 3);

        let slot = table.index_of(&dest(1)).unwrap();
        table.remove_route(slot);

        assert_eq!(table.route_count(), 2);
        assert_eq!(table.hop_count_to(&dest(1)), None);
        assert!(table.stored_announce_for(&dest(1)).is_none());

        assert_eq!(table.hop_count_to(&dest(2)), Some(2));
        assert_eq!(table.hop_count_to(&dest(3)), Some(3));
        assert_eq!(table.app_data_for(&dest(2)), Some(&app_data(0x22)[..]));
        assert_eq!(
            table.app_data_for(&dest(3)),
            Some(&app_data(0x33)[..]),
            "the moved row's app-data handle survives the free of the removed row's",
        );
        assert!(
            table
                .existing_route_for(&dest(3), &full_interfaces())
                .unwrap()
                .announce_id_history
                .contains(&announce_id(0xA3, 1)),
            "dest 3's announce-id history moved into the hole intact",
        );
        assert!(table
            .existing_route_for(&dest(2), &full_interfaces())
            .unwrap()
            .announce_id_history
            .contains(&announce_id(0xA2, 1)));
    }

    fn cull_a_mixed_table<R, A, H, D>(table: &mut RoutingTable<R, A, H, D>)
    where
        R: RouteTable,
        A: AnnounceRecordTable,
        H: AnnounceIdHistory,
        D: AnnounceAppData,
    {
        let stale_arrival = InstantMillis(0);
        let fresh_arrival = InstantMillis(1);
        for (dest_byte, arrival) in [
            (1u8, stale_arrival),
            (2, stale_arrival),
            (3, fresh_arrival),
            (4, stale_arrival),
            (5, fresh_arrival),
        ] {
            assert_eq!(
                table.upsert_route(
                    &AnnounceArrival {
                        announce: announce_for(
                            dest(dest_byte),
                            announce_id(dest_byte, 1),
                            None,
                            &[dest_byte; 4]
                        ),
                        hops: dest_byte,
                        arrived_at: arrival,
                        receiving_interface: source(),
                        next_hop: NextHop::Direct,
                        is_path_response: false,
                    },
                    &full_interfaces(),
                    &mut |_| {},
                ),
                UpsertRouteOutcome::Inserted
            );
        }
        assert_eq!(
            table.cull_expired_routes(fresh_arrival, &full_interfaces(), &mut |_| {}),
            0,
            "nothing has expired yet"
        );
        assert_eq!(table.route_count(), 5);

        let mut culled_destinations = std::vec::Vec::new();
        let culled = table.cull_expired_routes(
            InstantMillis(DEFAULT_ROUTE_EXPIRY_MILLIS),
            &full_interfaces(),
            &mut |removed| culled_destinations.push(removed),
        );
        assert_eq!(
            culled, 3,
            "exactly the stale arrivals, expiry boundary inclusive"
        );
        assert_eq!(
            culled_destinations,
            std::vec![
                RemovedRoute {
                    destination: dest(1),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Expired,
                },
                RemovedRoute {
                    destination: dest(2),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Expired,
                },
                RemovedRoute {
                    destination: dest(4),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Expired,
                },
            ],
            "each removal reports the destination it dropped and why",
        );
        assert_eq!(table.route_count(), 2);
        for gone in [1u8, 2, 4] {
            assert_eq!(table.hop_count_to(&dest(gone)), None);
        }
        for kept in [3u8, 5] {
            assert_eq!(table.hop_count_to(&dest(kept)), Some(kept));
            assert_eq!(table.app_data_for(&dest(kept)), Some(&[kept; 4][..]));
            assert!(table
                .existing_route_for(&dest(kept), &full_interfaces())
                .unwrap()
                .announce_id_history
                .contains(&announce_id(kept, 1)));
        }
    }

    #[test]
    fn cull_expired_routes_removes_every_expired_route_and_keeps_survivors_intact() {
        cull_a_mixed_table(&mut Rt::default());
    }

    #[test]
    fn cull_expired_routes_behaves_identically_on_the_heap_backend() {
        use crate::routing::announce::stored::{
            HeapAnnounceAppData, HeapAnnounceIdHistory, HeapAnnounceRecordTable,
        };
        use crate::routing::routes::HeapRouteTable;
        let mut table: RoutingTable<
            HeapRouteTable,
            HeapAnnounceRecordTable,
            HeapAnnounceIdHistory,
            HeapAnnounceAppData,
        > = RoutingTable::default();
        cull_a_mixed_table(&mut table);
    }

    #[test]
    fn a_full_table_culls_expired_routes_to_admit_a_new_destination() {
        const MAX: usize = 4;
        let mut table: TestRoutingTable<MAX, 8, 256> = TestRoutingTable::default();
        for (dest_byte, arrival) in [(1u8, 0u64), (2, 0), (3, 1), (4, 1)] {
            assert_eq!(
                record(
                    &mut table,
                    dest(dest_byte),
                    1,
                    InstantMillis(arrival),
                    announce_id(dest_byte, 1),
                    &app_data(dest_byte)
                ),
                UpsertRouteOutcome::Inserted
            );
        }
        assert_eq!(table.route_count(), MAX);

        let now = InstantMillis(DEFAULT_ROUTE_EXPIRY_MILLIS);
        assert_eq!(
            record(&mut table, dest(5), 1, now, announce_id(5, 1), &app_data(5)),
            UpsertRouteOutcome::Inserted
        );
        assert_eq!(
            table.route_count(),
            3,
            "both expired occupants culled, the newcomer admitted"
        );
        assert_eq!(table.hop_count_to(&dest(1)), None);
        assert_eq!(table.hop_count_to(&dest(2)), None);
        assert_eq!(table.hop_count_to(&dest(3)), Some(1));
        assert_eq!(table.hop_count_to(&dest(4)), Some(1));
        assert_eq!(table.app_data_for(&dest(5)), Some(&app_data(5)[..]));
    }

    #[test]
    fn a_route_whose_interface_left_the_view_is_culled_as_interface_gone() {
        let mut table: Rt = Rt::default();
        let surviving_interface = iface(0xA1);
        let vanishing_interface = iface(0xB2);
        let both = [
            routable_descriptor(surviving_interface),
            routable_descriptor(vanishing_interface),
        ];
        for (dest_byte, learned_on) in [(1u8, surviving_interface), (2, vanishing_interface)] {
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(
                        dest(dest_byte),
                        announce_id(dest_byte, 1),
                        None,
                        &app_data(dest_byte),
                    ),
                    hops: 1,
                    arrived_at: InstantMillis(1_000),
                    receiving_interface: learned_on,
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                &both,
                &mut |_| {},
            );
        }

        let shrunk = [routable_descriptor(surviving_interface)];
        assert_eq!(
            table.soonest_route_expiry(&shrunk),
            Some(InstantMillis(1_000)),
            "the orphan earns no lifetime, so the lane is due the moment the attached interfaces shrink",
        );

        let mut removed = std::vec::Vec::new();
        let culled = table.cull_expired_routes(InstantMillis(2_000), &shrunk, &mut |removal| {
            removed.push(removal);
        });
        assert_eq!(culled, 1);
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(2),
                receiving_interface: vanishing_interface,
                cause: RouteRemovalCause::InterfaceGone,
            }],
        );
        assert_eq!(table.hop_count_to(&dest(2)), None);
        assert_eq!(
            table.hop_count_to(&dest(1)),
            Some(1),
            "the route on the surviving interface is untouched",
        );
    }

    #[test]
    fn at_capacity_an_orphan_goes_as_interface_gone_before_any_fresh_eviction() {
        const MAX: usize = 2;
        let mut table: TestRoutingTable<MAX, 8, 256> = TestRoutingTable::default();
        let surviving_interface = iface(0xA1);
        let vanishing_interface = iface(0xB2);
        let both = [
            routable_descriptor(surviving_interface),
            routable_descriptor(vanishing_interface),
        ];
        for (dest_byte, learned_on) in [(1u8, surviving_interface), (2, vanishing_interface)] {
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(
                        dest(dest_byte),
                        announce_id(dest_byte, 1),
                        None,
                        &app_data(dest_byte),
                    ),
                    hops: 1,
                    arrived_at: InstantMillis(1_000),
                    receiving_interface: learned_on,
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                &both,
                &mut |_| {},
            );
        }

        let shrunk = [routable_descriptor(surviving_interface)];
        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(3), announce_id(3, 1), None, &app_data(3)),
                    hops: 1,
                    arrived_at: InstantMillis(2_000),
                    receiving_interface: surviving_interface,
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                &shrunk,
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(2),
                receiving_interface: vanishing_interface,
                cause: RouteRemovalCause::InterfaceGone,
            }],
            "the orphan is already due, so the inline cull takes it before eviction is consulted",
        );
        assert_eq!(table.hop_count_to(&dest(1)), Some(1));
        assert_eq!(table.hop_count_to(&dest(3)), Some(1));
    }

    #[test]
    fn expired_occupants_are_culled_before_any_fresh_route_is_evicted() {
        const MAX: usize = 4;
        let mut table: TestRoutingTable<MAX, 8, 256> = TestRoutingTable::default();
        for (dest_byte, arrival) in [(1u8, 0u64), (2, 0), (3, 1_000), (4, 1_000)] {
            record(
                &mut table,
                dest(dest_byte),
                1,
                InstantMillis(arrival),
                announce_id(dest_byte, 1),
                &app_data(dest_byte),
            );
        }

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(5), announce_id(5, 1), None, &app_data(5)),
                    hops: 1,
                    arrived_at: InstantMillis(DEFAULT_ROUTE_EXPIRY_MILLIS),
                    receiving_interface: source(),
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                &full_interfaces(),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
        );
        assert_eq!(
            removed,
            std::vec![
                RemovedRoute {
                    destination: dest(1),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Expired,
                },
                RemovedRoute {
                    destination: dest(2),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Expired,
                },
            ],
            "the expired occupants go as expirations; no fresh route is evicted",
        );
        assert_eq!(table.route_count(), 3);
        assert_eq!(table.hop_count_to(&dest(3)), Some(1));
        assert_eq!(table.hop_count_to(&dest(4)), Some(1));
        assert_eq!(table.hop_count_to(&dest(5)), Some(1));
    }

    #[test]
    fn remove_route_of_the_only_route_empties_the_table() {
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(100),
            announce_id(0xA1, 1),
            &app_data(0x11),
        );
        assert_eq!(table.route_count(), 1);

        table.remove_route(0);

        assert_eq!(table.route_count(), 0);
        assert_eq!(table.hop_count_to(&dest(1)), None);

        record(
            &mut table,
            dest(2),
            2,
            InstantMillis(200),
            announce_id(0xA2, 1),
            &app_data(0x22),
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.app_data_for(&dest(2)), Some(&app_data(0x22)[..]));
    }
}
