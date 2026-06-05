use crate::engine::directives::FixedEngineDirectives;
use crate::routing::announce::held_cache::FixedHeldAnnounces;
use crate::routing::announce::schedule::FixedRebroadcastQueue;
use crate::routing::dedup::FixedPacketHashHistory;
use crate::routing::storage::{
    EngineStorage, FixedArrayRetainedAnnounceColumns, FixedArrayRouteColumns, PackedAppDataArena,
    TieredAnnounceIdHistory,
};
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;

pub struct FixedCapacity<
    const MAX_TRACKED_DESTINATIONS: usize,
    const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
    const HISTORY_FLOOR_PER_DESTINATION: usize,
    const HISTORY_OVERFLOW_CAPACITY: usize,
    const HELD_CACHE_CAPACITY: usize,
    const MAX_LOCAL_DESTINATIONS: usize,
    const PACKET_HASH_GENERATION_CAPACITY: usize,
>;

impl<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
        const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
        const HISTORY_FLOOR_PER_DESTINATION: usize,
        const HISTORY_OVERFLOW_CAPACITY: usize,
        const HELD_CACHE_CAPACITY: usize,
        const MAX_LOCAL_DESTINATIONS: usize,
        const PACKET_HASH_GENERATION_CAPACITY: usize,
    > EngineStorage
    for FixedCapacity<
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
        ANNOUNCE_APP_DATA_ARENA_BYTES,
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
        HELD_CACHE_CAPACITY,
        MAX_LOCAL_DESTINATIONS,
        PACKET_HASH_GENERATION_CAPACITY,
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
    type UpstreamAppDestinations = FixedUpstreamAppDestinationColumns<MAX_LOCAL_DESTINATIONS>;
    type PacketHashes = FixedPacketHashHistory<PACKET_HASH_GENERATION_CAPACITY>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::dedup::PacketHashHistory;
    use crate::routing::storage::{RetainedAnnounceColumns, RouteColumns};
    use crate::routing::upstream_app_destinations::UpstreamAppDestinationColumns;

    #[test]
    fn bundles_fixed_array_backends_sized_by_the_consts() {
        type S = FixedCapacity<8, 16, 256, 2, 8, 4, 2, 4>;
        let routes = <S as EngineStorage>::Routes::default();
        let announces = <S as EngineStorage>::Announces::default();
        let _history = <S as EngineStorage>::History::default();
        let _app_data = <S as EngineStorage>::AppData::default();
        let _pending = <S as EngineStorage>::Pending::default();
        let _held = <S as EngineStorage>::Held::default();
        let _directives = <S as EngineStorage>::Directives::default();
        let upstream_app_destinations = <S as EngineStorage>::UpstreamAppDestinations::default();
        let packet_hashes = <S as EngineStorage>::PacketHashes::default();
        assert_eq!(routes.capacity(), 8);
        assert_eq!(announces.capacity(), 8);
        assert_eq!(upstream_app_destinations.capacity(), 2);
        assert_eq!(packet_hashes.generation_capacity(), 4);
    }
}
