use crate::engine::directives::FixedEngineDirectives;
use crate::engine::receipts::FixedReceiptColumns;
use crate::engine::reverse_routes::FixedReverseRouteColumns;
use crate::engine::self_announce::FixedSelfAnnounceColumns;
use crate::engine::self_ratchets::FixedSelfRatchetColumns;
use crate::identity::held::FixedHeldIdentityColumns;
use crate::routing::announce::held_cache::FixedHeldAnnounces;
use crate::routing::announce::schedule::FixedRebroadcastQueue;
use crate::routing::dedup::FixedPacketHashHistory;
use crate::routing::storage::{
    EngineStorage, FixedArrayRetainedAnnounceColumns, FixedArrayRouteColumns, PackedAppDataArena,
    TieredAnnounceIdHistory,
};
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;

pub struct FixedInline<
    const MAX_TRACKED_DESTINATIONS: usize,
    const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
    const HISTORY_FLOOR_PER_DESTINATION: usize,
    const HISTORY_OVERFLOW_CAPACITY: usize,
    const HELD_CACHE_CAPACITY: usize,
    const MAX_UPSTREAM_APP_DESTINATIONS: usize,
    const MAX_HELD_IDENTITIES: usize,
    const MAX_SELF_ANNOUNCED_DESTINATIONS: usize,
    const PACKET_HASH_GENERATION_CAPACITY: usize,
    const RETAINED_RATCHETS_PER_DESTINATION: usize,
    const MAX_OUTSTANDING_RECEIPTS: usize,
    const MAX_REVERSE_ROUTES: usize,
>;

impl<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
        const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
        const HISTORY_FLOOR_PER_DESTINATION: usize,
        const HISTORY_OVERFLOW_CAPACITY: usize,
        const HELD_CACHE_CAPACITY: usize,
        const MAX_UPSTREAM_APP_DESTINATIONS: usize,
        const MAX_HELD_IDENTITIES: usize,
        const MAX_SELF_ANNOUNCED_DESTINATIONS: usize,
        const PACKET_HASH_GENERATION_CAPACITY: usize,
        const RETAINED_RATCHETS_PER_DESTINATION: usize,
        const MAX_OUTSTANDING_RECEIPTS: usize,
        const MAX_REVERSE_ROUTES: usize,
    > EngineStorage
    for FixedInline<
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
        ANNOUNCE_APP_DATA_ARENA_BYTES,
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
        HELD_CACHE_CAPACITY,
        MAX_UPSTREAM_APP_DESTINATIONS,
        MAX_HELD_IDENTITIES,
        MAX_SELF_ANNOUNCED_DESTINATIONS,
        PACKET_HASH_GENERATION_CAPACITY,
        RETAINED_RATCHETS_PER_DESTINATION,
        MAX_OUTSTANDING_RECEIPTS,
        MAX_REVERSE_ROUTES,
    >
{
    type Routes = FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>;
    type Announces = FixedArrayRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS>;
    type History = TieredAnnounceIdHistory<
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
    >;
    type AppData = PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>;
    // Sized by the routing table (one pending per tracked route), not the held
    // cache — there is never more than one pending rebroadcast per destination.
    type Pending = FixedRebroadcastQueue<MAX_TRACKED_DESTINATIONS>;
    type Held = FixedHeldAnnounces<HELD_CACHE_CAPACITY>;
    // One directive per tracked destination at most, so sized by the routing table.
    type Directives = FixedEngineDirectives<MAX_TRACKED_DESTINATIONS>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type HeldIdentities = FixedHeldIdentityColumns<MAX_HELD_IDENTITIES>;
    type SelfAnnounces = FixedSelfAnnounceColumns<MAX_SELF_ANNOUNCED_DESTINATIONS>;
    // Sized by the upstream registry: every registered Single may opt into
    // ratchets, and nothing else can.
    type SelfRatchets =
        FixedSelfRatchetColumns<MAX_UPSTREAM_APP_DESTINATIONS, RETAINED_RATCHETS_PER_DESTINATION>;
    type PacketHashes = FixedPacketHashHistory<PACKET_HASH_GENERATION_CAPACITY>;
    type Receipts = FixedReceiptColumns<MAX_OUTSTANDING_RECEIPTS>;
    type ReverseRoutes = FixedReverseRouteColumns<MAX_REVERSE_ROUTES>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::receipts::ReceiptColumns;
    use crate::engine::self_ratchets::SelfRatchetColumns;
    use crate::routing::dedup::PacketHashHistory;
    use crate::routing::storage::{RetainedAnnounceColumns, RouteColumns};
    use crate::routing::upstream_app_destinations::UpstreamAppDestinationColumns;

    #[test]
    fn bundles_fixed_array_backends_sized_by_the_consts() {
        type S = FixedInline<8, 16, 256, 2, 8, 4, 2, 2, 2, 4, 3, 5, 8>;
        let routes = <S as EngineStorage>::Routes::default();
        let announces = <S as EngineStorage>::Announces::default();
        let _history = <S as EngineStorage>::History::default();
        let _app_data = <S as EngineStorage>::AppData::default();
        let _pending = <S as EngineStorage>::Pending::default();
        let _held = <S as EngineStorage>::Held::default();
        let _directives = <S as EngineStorage>::Directives::default();
        let upstream_app_destinations = <S as EngineStorage>::UpstreamAppDestinations::default();
        let packet_hashes = <S as EngineStorage>::PacketHashes::default();
        let self_ratchets = <S as EngineStorage>::SelfRatchets::default();
        assert_eq!(routes.capacity(), 8);
        assert_eq!(announces.capacity(), 8);
        assert_eq!(upstream_app_destinations.capacity(), 2);
        assert_eq!(packet_hashes.generation_capacity(), 4);
        assert_eq!(self_ratchets.capacity(), 2);
        assert_eq!(self_ratchets.retained_per_destination(), 3);
        let receipts = <S as EngineStorage>::Receipts::default();
        assert_eq!(receipts.capacity(), 5);
    }
}
