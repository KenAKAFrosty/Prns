//! `GrowableHeap` — the std/alloc storage recipe: every backend is a heap-backed,
//! growable structure with no capacity knobs.

use crate::engine::directives::HeapEngineDirectives;
use crate::routing::held_cache::HeapHeldAnnounces;
use crate::routing::schedule::HeapRebroadcastQueue;
use crate::routing::storage::{
    HeapAnnounceIdHistory, HeapRetainedAnnounceColumns, HeapRetainedAppData, HeapRouteColumns,
    Storage,
};

pub struct GrowableHeap;

impl Storage for GrowableHeap {
    type Routes = HeapRouteColumns;
    type Announces = HeapRetainedAnnounceColumns;
    type History = HeapAnnounceIdHistory;
    type AppData = HeapRetainedAppData;
    type Pending = HeapRebroadcastQueue;
    type Held = HeapHeldAnnounces;
    type Directives = HeapEngineDirectives;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::storage::{RetainedAnnounceColumns, RouteColumns};

    #[test]
    fn bundles_unbounded_heap_backends() {
        let routes = <GrowableHeap as Storage>::Routes::default();
        let announces = <GrowableHeap as Storage>::Announces::default();
        let _history = <GrowableHeap as Storage>::History::default();
        let _app_data = <GrowableHeap as Storage>::AppData::default();
        let _pending = <GrowableHeap as Storage>::Pending::default();
        let _held = <GrowableHeap as Storage>::Held::default();
        let _directives = <GrowableHeap as Storage>::Directives::default();
        assert_eq!(routes.capacity(), usize::MAX);
        assert_eq!(announces.capacity(), usize::MAX);
    }
}
