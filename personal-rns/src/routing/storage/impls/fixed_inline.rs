use crate::crypto::ratchets::FixedSelfRatchetColumns;
use crate::identity::held::FixedHeldIdentityColumns;
use crate::routing::announce::announce_rate::FixedAnnounceRateColumns;
use crate::routing::announce::held_cache::FixedHeldAnnounces;
use crate::routing::announce::schedule::FixedRebroadcastQueue;
use crate::routing::dedup::FixedPacketHashHistory;
use crate::routing::delivery::receipts::FixedReceiptColumns;
use crate::routing::path_requests::pending::FixedPendingPathRequestColumns;
use crate::routing::path_requests::seen::FixedSeenPathRequestColumns;
use crate::routing::reverse_routes::FixedReverseRouteColumns;
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
    const PACKET_HASH_GENERATION_CAPACITY: usize,
    const RETAINED_RATCHETS_PER_DESTINATION: usize,
    const MAX_OUTSTANDING_RECEIPTS: usize,
    const MAX_REVERSE_ROUTES: usize,
    const MAX_PENDING_PATH_REQUESTS: usize,
    const MAX_SEEN_PATH_REQUESTS: usize,
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
        const PACKET_HASH_GENERATION_CAPACITY: usize,
        const RETAINED_RATCHETS_PER_DESTINATION: usize,
        const MAX_OUTSTANDING_RECEIPTS: usize,
        const MAX_REVERSE_ROUTES: usize,
        const MAX_PENDING_PATH_REQUESTS: usize,
        const MAX_SEEN_PATH_REQUESTS: usize,
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
        PACKET_HASH_GENERATION_CAPACITY,
        RETAINED_RATCHETS_PER_DESTINATION,
        MAX_OUTSTANDING_RECEIPTS,
        MAX_REVERSE_ROUTES,
        MAX_PENDING_PATH_REQUESTS,
        MAX_SEEN_PATH_REQUESTS,
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
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type HeldIdentities = FixedHeldIdentityColumns<MAX_HELD_IDENTITIES>;
    // Sized by the upstream registry: every registered Single may opt into
    // ratchets, and nothing else can.
    type SelfRatchets =
        FixedSelfRatchetColumns<MAX_UPSTREAM_APP_DESTINATIONS, RETAINED_RATCHETS_PER_DESTINATION>;
    type PacketHashes = FixedPacketHashHistory<PACKET_HASH_GENERATION_CAPACITY>;
    type Receipts = FixedReceiptColumns<MAX_OUTSTANDING_RECEIPTS>;
    type ReverseRoutes = FixedReverseRouteColumns<MAX_REVERSE_ROUTES>;
    type PendingPathRequests = FixedPendingPathRequestColumns<MAX_PENDING_PATH_REQUESTS>;
    type SeenPathRequests = FixedSeenPathRequestColumns<MAX_SEEN_PATH_REQUESTS>;
    // One rate entry per tracked destination at most (we rate-check only what we
    // would rebroadcast), so sized by the routing table like Pending/Directives.
    type AnnounceRates = FixedAnnounceRateColumns<MAX_TRACKED_DESTINATIONS>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::ratchets::SelfRatchetColumns;
    use crate::routing::dedup::PacketHashHistory;
    use crate::routing::delivery::receipts::ReceiptColumns;
    use crate::routing::storage::{RetainedAnnounceColumns, RouteColumns};
    use crate::routing::upstream_app_destinations::UpstreamAppDestinationColumns;

    #[test]
    fn bundles_fixed_array_backends_sized_by_the_consts() {
        type S = FixedInline<8, 16, 256, 2, 8, 4, 2, 2, 4, 3, 5, 8, 4, 8>;
        let routes = <S as EngineStorage>::Routes::default();
        let announces = <S as EngineStorage>::Announces::default();
        let _history = <S as EngineStorage>::History::default();
        let _app_data = <S as EngineStorage>::AppData::default();
        let _pending = <S as EngineStorage>::Pending::default();
        let _held = <S as EngineStorage>::Held::default();
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
