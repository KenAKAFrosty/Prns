use crate::engine::directives::HeapEngineDirectives;
use crate::engine::local_destinations::HeapLocalDestinationColumns;
use crate::routing::announce::held_cache::HeapHeldAnnounces;
use crate::routing::announce::schedule::HeapRebroadcastQueue;
use crate::routing::storage::{
    EngineStorage, HeapAnnounceIdHistory, HeapRetainedAnnounceColumns, HeapRetainedAppData,
    HeapRouteColumns,
};

pub struct GrowableHeap;

impl EngineStorage for GrowableHeap {
    type Routes = HeapRouteColumns;
    type Announces = HeapRetainedAnnounceColumns;
    type History = HeapAnnounceIdHistory;
    type AppData = HeapRetainedAppData;
    type Pending = HeapRebroadcastQueue;
    type Held = HeapHeldAnnounces;
    type Directives = HeapEngineDirectives;
    type LocalDestinations = HeapLocalDestinationColumns;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::local_destinations::LocalDestinationColumns;
    use crate::routing::storage::{RetainedAnnounceColumns, RouteColumns};

    #[test]
    fn bundles_unbounded_heap_backends() {
        let routes = <GrowableHeap as EngineStorage>::Routes::default();
        let announces = <GrowableHeap as EngineStorage>::Announces::default();
        let _history = <GrowableHeap as EngineStorage>::History::default();
        let _app_data = <GrowableHeap as EngineStorage>::AppData::default();
        let _pending = <GrowableHeap as EngineStorage>::Pending::default();
        let _held = <GrowableHeap as EngineStorage>::Held::default();
        let _directives = <GrowableHeap as EngineStorage>::Directives::default();
        let local_destinations = <GrowableHeap as EngineStorage>::LocalDestinations::default();
        assert_eq!(routes.capacity(), usize::MAX);
        assert_eq!(announces.capacity(), usize::MAX);
        assert_eq!(local_destinations.capacity(), usize::MAX);
    }
}
