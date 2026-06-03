//! `FixedCapacity` — the no_std storage recipe: every column backend is a
//! fixed-size, alloc-free array sized by these const generics. The held-announce
//! cache capacity is the engine's `MAX_HELD`, not part of the column bundle.

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
>;

impl<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
        const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
        const HISTORY_FLOOR_PER_DESTINATION: usize,
        const HISTORY_OVERFLOW_CAPACITY: usize,
    > Storage
    for FixedCapacity<
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
        ANNOUNCE_APP_DATA_ARENA_BYTES,
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::storage::{RetainedAnnounceColumns, RouteColumns};

    #[test]
    fn bundles_fixed_array_backends_sized_by_the_consts() {
        type S = FixedCapacity<8, 16, 256, 2, 8>;
        let routes = <S as Storage>::Routes::default();
        let announces = <S as Storage>::Announces::default();
        let _history = <S as Storage>::History::default();
        let _app_data = <S as Storage>::AppData::default();
        // MAX_TRACKED_DESTINATIONS (8) flows into both slot-keyed columns.
        assert_eq!(routes.capacity(), 8);
        assert_eq!(announces.capacity(), 8);
    }
}
