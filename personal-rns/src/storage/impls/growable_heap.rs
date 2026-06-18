use crate::crypto::ratchets::HeapSelfRatchetColumns;
use crate::identity::held::HeapHeldIdentityColumns;
use crate::routing::announce::rate_limit::HeapAnnounceRateColumns;
use crate::routing::announce::retained::{
    HeapAnnounceIdHistory, HeapRetainedAnnounceColumns, HeapRetainedAppData,
};
use crate::routing::announce::schedule::HeapScheduledAnnounceQueue;
use crate::routing::dedup::HeapPacketHashHistory;
use crate::routing::delivery::receipts::HeapReceiptColumns;
use crate::routing::group_keys::HeapGroupKeyColumns;
use crate::routing::links::channel::impls::HeapChannelColumns;
use crate::routing::links::resources::assembly::{
    HeapIncomingAssemblyColumns, HeapOutgoingAssemblyColumns,
};
use crate::routing::links::resources::table::{
    HeapResourceColumns, IncomingResourceState, OutgoingResourceState,
};
use crate::routing::links::table::HeapLinkColumns;
use crate::routing::links::transported::HeapTransportedLinkColumns;
use crate::routing::path_requests::discovery::HeapDiscoveryPathRequestColumns;
use crate::routing::path_requests::interface_path_request_limit::HeapInterfacePathRequestLimitColumns;
use crate::routing::path_requests::pending::HeapPendingPathRequestColumns;
use crate::routing::path_requests::recent::HeapRecentPathRequestColumns;
use crate::routing::path_requests::seen::HeapSeenPathRequestColumns;
use crate::routing::request_handlers::HeapRequestHandlerColumns;
use crate::routing::reverse_routes::HeapReverseRouteColumns;
use crate::routing::routes::HeapRouteColumns;
use crate::routing::upstream_app_destinations::HeapUpstreamAppDestinationColumns;
use crate::storage::StorageLayout;

#[derive(Debug, Clone, Copy, Default)]
pub struct GrowableHeap;

impl StorageLayout for GrowableHeap {
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
    type RecentPathRequests = HeapRecentPathRequestColumns;
    type SeenPathRequests = HeapSeenPathRequestColumns;
    type DiscoveryPathRequests = HeapDiscoveryPathRequestColumns;
    type InterfacePathRequestLimits = HeapInterfacePathRequestLimitColumns;
    type AnnounceRates = HeapAnnounceRateColumns;
    type GroupKeys = HeapGroupKeyColumns;
    type RequestHandlers = HeapRequestHandlerColumns;
    type TransportedLinks = HeapTransportedLinkColumns;
    type Links = HeapLinkColumns;
    type OutgoingResources = HeapResourceColumns<OutgoingResourceState>;
    type IncomingResources = HeapResourceColumns<IncomingResourceState>;
    type IncomingAssemblies = HeapIncomingAssemblyColumns;
    type OutgoingAssemblies = HeapOutgoingAssemblyColumns;
    type Channels = HeapChannelColumns;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::announce::retained::RetainedAnnounceColumns;
    use crate::routing::dedup::PacketHashHistory;
    use crate::routing::routes::RouteColumns;
    use crate::routing::upstream_app_destinations::UpstreamAppDestinationColumns;

    #[test]
    fn bundles_unbounded_heap_backends() {
        let routes = <GrowableHeap as StorageLayout>::Routes::default();
        let announces = <GrowableHeap as StorageLayout>::Announces::default();
        let _history = <GrowableHeap as StorageLayout>::History::default();
        let _app_data = <GrowableHeap as StorageLayout>::AppData::default();
        let _pending = <GrowableHeap as StorageLayout>::ScheduledAnnounces::default();
        let upstream_app_destinations =
            <GrowableHeap as StorageLayout>::UpstreamAppDestinations::default();
        let packet_hashes = <GrowableHeap as StorageLayout>::PacketHashes::default();
        assert_eq!(routes.capacity(), usize::MAX);
        assert_eq!(announces.capacity(), usize::MAX);
        assert_eq!(upstream_app_destinations.capacity(), usize::MAX);
        assert_eq!(packet_hashes.generation_capacity(), 500_000);
    }
}
