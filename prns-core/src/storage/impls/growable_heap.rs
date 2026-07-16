use crate::crypto::ratchets::HeapSelfRatchetTable;
use crate::identity::held::HeapHeldIdentityTable;
use crate::identity::known::HeapKnownDestinationTable;
use crate::interfaces::InterfaceId;
use crate::routing::announce::destination_announce_limit::HeapDestinationAnnounceLimitTable;
use crate::routing::announce::held::HeapHeldAnnounceTable;
use crate::routing::announce::interface_announce_limit::HeapInterfaceAnnounceLimitTable;
use crate::routing::announce::schedule::HeapScheduledAnnounceQueue;
use crate::routing::announce::stored::{
    HeapAnnounceAppData, HeapAnnounceIdHistory, HeapAnnounceRecordTable,
};
use crate::routing::blackhole::HeapBlackholeTable;
use crate::routing::dedup::HeapPacketHashHistory;
use crate::routing::delivery::receipts::HeapReceiptTable;
use crate::routing::group_keys::HeapGroupKeyTable;
use crate::routing::links::channel::table::impls::HeapChannelTable;
use crate::routing::links::resources::assembly::{
    HeapIncomingAssemblyTable, HeapOutgoingAssemblyTable,
};
use crate::routing::links::resources::table::{
    HeapResourceTable, IncomingResourceState, OutgoingResourceState,
};
use crate::routing::links::table::HeapLinkTable;
use crate::routing::links::transported::HeapTransportedLinkTable;
use crate::routing::path_requests::interface_path_request_limit::HeapInterfacePathRequestLimitTable;
use crate::routing::path_requests::pending::HeapPendingPathRequestTable;
use crate::routing::path_requests::recent::HeapRecentPathRequestTable;
use crate::routing::path_requests::recursive::HeapRecursivePathRequestTable;
use crate::routing::path_requests::seen::HeapSeenPathRequestTable;
use crate::routing::request_handlers::HeapRequestHandlerTable;
use crate::routing::reverse_routes::HeapReverseRouteTable;
#[cfg(not(feature = "std"))]
use crate::routing::route_expiry::LinearRouteExpiryIndex;
#[cfg(feature = "std")]
use crate::routing::route_expiry::RoaringRouteExpiryIndex;
use crate::routing::routes::HeapRouteTable;
use crate::routing::tunnel::HeapTunnelTable;
use crate::routing::upstream_app_destinations::HeapUpstreamAppDestinationTable;
use crate::routing::warmth::HeapDepartedInterfaceTable;
use crate::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};
use alloc::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Default)]
pub struct GrowableHeap;

impl StorageLayout for GrowableHeap {
    const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
        packet_hashes: StorageCapacity::Fixed(HeapPacketHashHistory::RNS_GENERATION_CAPACITY),
        ..DisplayedStorageLimits::DYNAMIC
    };

    type Routes = HeapRouteTable;
    #[cfg(feature = "std")]
    type RouteExpiries = RoaringRouteExpiryIndex;
    #[cfg(not(feature = "std"))]
    type RouteExpiries = LinearRouteExpiryIndex;
    type KnownDestinations = HeapKnownDestinationTable;
    type KnownDestinationAppData = HeapAnnounceAppData;
    type Announces = HeapAnnounceRecordTable;
    type History = HeapAnnounceIdHistory;
    type AppData = HeapAnnounceAppData;
    type ScheduledAnnounces = HeapScheduledAnnounceQueue;
    type UpstreamAppDestinations = HeapUpstreamAppDestinationTable;
    type HeldIdentities = HeapHeldIdentityTable;
    type SelfRatchets = HeapSelfRatchetTable;
    type Receipts = HeapReceiptTable;
    type PacketHashes = HeapPacketHashHistory;
    type Blackholes = HeapBlackholeTable;
    type ReverseRoutes = HeapReverseRouteTable;
    type DepartedInterfaces = HeapDepartedInterfaceTable;
    type PendingPathRequests = HeapPendingPathRequestTable;
    type RecentPathRequests = HeapRecentPathRequestTable;
    type SeenPathRequests = HeapSeenPathRequestTable;
    type Tunnels = HeapTunnelTable;
    type RecursivePathRequests = HeapRecursivePathRequestTable;
    type InterfacePathRequestLimits = HeapInterfacePathRequestLimitTable;
    type InterfaceAnnounceLimits = HeapInterfaceAnnounceLimitTable;
    type HeldAnnounces = HeapHeldAnnounceTable;
    type HeldAnnounceAppData = HeapAnnounceAppData;
    type DestinationAnnounceLimits = HeapDestinationAnnounceLimitTable;
    type GroupKeys = HeapGroupKeyTable;
    type RequestHandlers = HeapRequestHandlerTable;
    type TransportedLinks = HeapTransportedLinkTable;
    type Links = HeapLinkTable;
    type OutgoingResources = HeapResourceTable<OutgoingResourceState>;
    type IncomingResources = HeapResourceTable<IncomingResourceState>;
    type IncomingAssemblies = HeapIncomingAssemblyTable;
    type OutgoingAssemblies = HeapOutgoingAssemblyTable;
    type Channels = HeapChannelTable;
    type DirtyInterfaces = BTreeSet<InterfaceId>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::known::KnownDestinationTable;
    use crate::routing::announce::stored::AnnounceRecordTable;
    use crate::routing::blackhole::{BlackholeTable, HeapBlackholeTable};
    use crate::routing::dedup::PacketHashHistory;
    use crate::routing::route_expiry::RouteExpiryIndex;
    use crate::routing::routes::RouteTable;
    use crate::routing::upstream_app_destinations::UpstreamAppDestinationTable;

    #[test]
    fn bundles_unbounded_heap_backends() {
        let routes = <GrowableHeap as StorageLayout>::Routes::default();
        let announces = <GrowableHeap as StorageLayout>::Announces::default();
        let known_destinations = <GrowableHeap as StorageLayout>::KnownDestinations::default();
        let _history = <GrowableHeap as StorageLayout>::History::default();
        let _app_data = <GrowableHeap as StorageLayout>::AppData::default();
        let _pending = <GrowableHeap as StorageLayout>::ScheduledAnnounces::default();
        let upstream_app_destinations =
            <GrowableHeap as StorageLayout>::UpstreamAppDestinations::default();
        let packet_hashes = <GrowableHeap as StorageLayout>::PacketHashes::default();
        let blackholes: HeapBlackholeTable = <GrowableHeap as StorageLayout>::Blackholes::default();
        assert_eq!(routes.capacity(), usize::MAX);
        assert_eq!(announces.capacity(), usize::MAX);
        assert_eq!(known_destinations.capacity(), usize::MAX);
        assert_eq!(upstream_app_destinations.capacity(), usize::MAX);
        assert_eq!(packet_hashes.generation_capacity(), 500_000);
        assert!(blackholes.is_empty());
        assert_eq!(
            <GrowableHeap as StorageLayout>::LIMITS.blackholed_identities,
            StorageCapacity::Dynamic
        );
        const {
            assert!(<<GrowableHeap as StorageLayout>::RouteExpiries as RouteExpiryIndex>::INDEXED);
        }
    }
}
