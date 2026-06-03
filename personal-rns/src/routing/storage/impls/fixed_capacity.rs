//! `FixedCapacity` — the no_std storage recipe: every backend is a fixed-size,
//! alloc-free structure sized by these six const generics.

use crate::engine::directives::FixedEngineDirectives;
use crate::routing::held_cache::FixedHeldAnnounces;
use crate::routing::schedule::FixedRebroadcastQueue;
use crate::routing::storage::{
    FixedArrayRetainedAnnounceColumns, FixedArrayRouteColumns, PackedAppDataArena, Storage,
    TieredAnnounceIdHistory,
};

pub struct FixedCapacity<
    const MAX_TRACKED_DESTINATIONS: usize,
    const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
    const HISTORY_FLOOR_PER_DESTINATION: usize,
    const HISTORY_OVERFLOW_CAPACITY: usize,
    const HELD_CACHE_CAPACITY: usize,
>;

impl<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
        const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
        const HISTORY_FLOOR_PER_DESTINATION: usize,
        const HISTORY_OVERFLOW_CAPACITY: usize,
        const HELD_CACHE_CAPACITY: usize,
    > Storage
    for FixedCapacity<
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
        ANNOUNCE_APP_DATA_ARENA_BYTES,
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
        HELD_CACHE_CAPACITY,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::storage::{RetainedAnnounceColumns, RouteColumns};

    #[test]
    fn bundles_fixed_array_backends_sized_by_the_consts() {
        type S = FixedCapacity<8, 16, 256, 2, 8, 4>;
        let routes = <S as Storage>::Routes::default();
        let announces = <S as Storage>::Announces::default();
        let _history = <S as Storage>::History::default();
        let _app_data = <S as Storage>::AppData::default();
        let _pending = <S as Storage>::Pending::default();
        let _held = <S as Storage>::Held::default();
        let _directives = <S as Storage>::Directives::default();
        // MAX_TRACKED_DESTINATIONS (8) flows into the slot-keyed columns.
        assert_eq!(routes.capacity(), 8);
        assert_eq!(announces.capacity(), 8);
    }
}
