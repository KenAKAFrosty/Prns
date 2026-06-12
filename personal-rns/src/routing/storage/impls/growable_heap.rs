use crate::crypto::ratchets::HeapSelfRatchetColumns;
use crate::identity::held::HeapHeldIdentityColumns;
use crate::routing::announce::rate_limit::HeapAnnounceRateColumns;
use crate::routing::announce::schedule::HeapScheduledAnnounceQueue;
use crate::routing::dedup::HeapPacketHashHistory;
use crate::routing::delivery::receipts::HeapReceiptColumns;
use crate::routing::group_keys::HeapGroupKeyColumns;
use crate::routing::links::resources::table::{
    HeapResourceColumns, IncomingResourceState, OutgoingResourceState,
};
use crate::routing::links::table::HeapLinkColumns;
use crate::routing::links::transported::HeapTransportedLinkColumns;
use crate::routing::path_requests::pending::HeapPendingPathRequestColumns;
use crate::routing::path_requests::seen::HeapSeenPathRequestColumns;
use crate::routing::request_handlers::HeapRequestHandlerColumns;
use crate::routing::reverse_routes::HeapReverseRouteColumns;
use crate::routing::storage::{
    EngineStorage, HeapAnnounceIdHistory, HeapRetainedAnnounceColumns, HeapRetainedAppData,
    HeapRouteColumns,
};
use crate::routing::upstream_app_destinations::HeapUpstreamAppDestinationColumns;

pub struct GrowableHeap;

impl EngineStorage for GrowableHeap {
    type Routes = HeapRouteColumns;
    type Announces = HeapRetainedAnnounceColumns;
    type History = HeapAnnounceIdHistory;
    type AppData = HeapRetainedAppData;
    type ScheduledAnnounces = HeapScheduledAnnounceQueue;
    type UpstreamAppDestinations = HeapUpstreamAppDestinationColumns;
    type HeldIdentities = HeapHeldIdentityColumns;
    type SelfRatchets = HeapSelfRatchetColumns;
    type Receipts = HeapReceiptColumns;
    type PacketHashes = HeapPacketHashHistory;
    type ReverseRoutes = HeapReverseRouteColumns;
    type PendingPathRequests = HeapPendingPathRequestColumns;
    type SeenPathRequests = HeapSeenPathRequestColumns;
    type AnnounceRates = HeapAnnounceRateColumns;
    type GroupKeys = HeapGroupKeyColumns;
    type RequestHandlers = HeapRequestHandlerColumns;
    type TransportedLinks = HeapTransportedLinkColumns;
    type Links = HeapLinkColumns;
    type OutgoingResources = HeapResourceColumns<OutgoingResourceState>;
    type IncomingResources = HeapResourceColumns<IncomingResourceState>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::dedup::PacketHashHistory;
    use crate::routing::storage::{RetainedAnnounceColumns, RouteColumns};
    use crate::routing::upstream_app_destinations::UpstreamAppDestinationColumns;

    #[test]
    fn bundles_unbounded_heap_backends() {
        let routes = <GrowableHeap as EngineStorage>::Routes::default();
        let announces = <GrowableHeap as EngineStorage>::Announces::default();
        let _history = <GrowableHeap as EngineStorage>::History::default();
        let _app_data = <GrowableHeap as EngineStorage>::AppData::default();
        let _pending = <GrowableHeap as EngineStorage>::ScheduledAnnounces::default();
        let upstream_app_destinations =
            <GrowableHeap as EngineStorage>::UpstreamAppDestinations::default();
        let packet_hashes = <GrowableHeap as EngineStorage>::PacketHashes::default();
        assert_eq!(routes.capacity(), usize::MAX);
        assert_eq!(announces.capacity(), usize::MAX);
        assert_eq!(upstream_app_destinations.capacity(), usize::MAX);
        assert_eq!(packet_hashes.generation_capacity(), 500_000);
    }
}
