use crate::crypto::ratchets::FixedSelfRatchetColumns;
use crate::identity::held::FixedHeldIdentityColumns;
use crate::routing::announce::held::FixedHeldAnnounceColumns;
use crate::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitColumns;
use crate::routing::announce::rate_limit::FixedAnnounceRateColumns;
use crate::routing::announce::retained::{
    FixedArrayRetainedAnnounceColumns, PackedAppDataArena, TieredAnnounceIdHistory,
};
use crate::routing::announce::schedule::FixedScheduledAnnounceQueue;
use crate::routing::dedup::FixedPacketHashHistory;
use crate::routing::delivery::receipts::FixedReceiptColumns;
use crate::routing::group_keys::FixedGroupKeyColumns;
use crate::routing::links::channel::channel_mdu;
use crate::routing::links::channel::impls::FixedArrayChannelColumns;
use crate::routing::links::resources::assembly::{
    FixedIncomingAssemblyColumns, FixedOutgoingAssemblyColumns,
};
use crate::routing::links::resources::table::{
    FixedResourceColumns, IncomingResourceState, OutgoingResourceState,
};
use crate::routing::links::table::FixedLinkColumns;
use crate::routing::links::transported::FixedTransportedLinkColumns;
use crate::routing::path_requests::discovery::FixedDiscoveryPathRequestColumns;
use crate::routing::path_requests::interface_path_request_limit::FixedInterfacePathRequestLimitColumns;
use crate::routing::path_requests::pending::FixedPendingPathRequestColumns;
use crate::routing::path_requests::recent::FixedRecentPathRequestColumns;
use crate::routing::path_requests::seen::FixedSeenPathRequestColumns;
use crate::routing::request_handlers::FixedRequestHandlerColumns;
use crate::routing::reverse_routes::FixedReverseRouteColumns;
use crate::routing::routes::FixedArrayRouteColumns;
use crate::routing::tunnel::FixedTunnelColumns;
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;
use crate::storage::{StorageCapacity, StorageLayout, StorageLimits};

pub struct TestFixedStorage<
    const MAX_TRACKED_DESTINATIONS: usize,
    const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
    const HISTORY_FLOOR_PER_DESTINATION: usize,
    const HISTORY_OVERFLOW_CAPACITY: usize,
    const MAX_UPSTREAM_APP_DESTINATIONS: usize,
    const MAX_HELD_IDENTITIES: usize,
    const PACKET_HASH_GENERATION_CAPACITY: usize,
    const RETAINED_RATCHETS_PER_DESTINATION: usize,
    const MAX_OUTSTANDING_RECEIPTS: usize,
    const MAX_REVERSE_ROUTES: usize,
    const MAX_PENDING_PATH_REQUESTS: usize,
    const MAX_SEEN_PATH_REQUESTS: usize,
    const MAX_LINKS: usize,
>;

impl<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
        const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
        const HISTORY_FLOOR_PER_DESTINATION: usize,
        const HISTORY_OVERFLOW_CAPACITY: usize,
        const MAX_UPSTREAM_APP_DESTINATIONS: usize,
        const MAX_HELD_IDENTITIES: usize,
        const PACKET_HASH_GENERATION_CAPACITY: usize,
        const RETAINED_RATCHETS_PER_DESTINATION: usize,
        const MAX_OUTSTANDING_RECEIPTS: usize,
        const MAX_REVERSE_ROUTES: usize,
        const MAX_PENDING_PATH_REQUESTS: usize,
        const MAX_SEEN_PATH_REQUESTS: usize,
        const MAX_LINKS: usize,
    > StorageLayout
    for TestFixedStorage<
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
        ANNOUNCE_APP_DATA_ARENA_BYTES,
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
        MAX_UPSTREAM_APP_DESTINATIONS,
        MAX_HELD_IDENTITIES,
        PACKET_HASH_GENERATION_CAPACITY,
        RETAINED_RATCHETS_PER_DESTINATION,
        MAX_OUTSTANDING_RECEIPTS,
        MAX_REVERSE_ROUTES,
        MAX_PENDING_PATH_REQUESTS,
        MAX_SEEN_PATH_REQUESTS,
        MAX_LINKS,
    >
{
    const LIMITS: StorageLimits = StorageLimits {
        tracked_destinations: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
        retained_announces: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
        upstream_app_destinations: StorageCapacity::Fixed(MAX_UPSTREAM_APP_DESTINATIONS),
        held_identities: StorageCapacity::Fixed(MAX_HELD_IDENTITIES),
        links: StorageCapacity::Fixed(MAX_LINKS),
        channels: StorageCapacity::Fixed(MAX_LINKS),
        channel_window_pool: None,
        channel_reorder_depth: StorageCapacity::Fixed(8),
        link_mtu: StorageCapacity::Fixed(8192),
        resource_transfer_bytes: StorageCapacity::Fixed(4096),
        receipts: StorageCapacity::Fixed(MAX_OUTSTANDING_RECEIPTS),
        packet_hashes: StorageCapacity::Fixed(PACKET_HASH_GENERATION_CAPACITY),
        reverse_routes: StorageCapacity::Fixed(MAX_REVERSE_ROUTES),
        pending_path_requests: StorageCapacity::Fixed(MAX_PENDING_PATH_REQUESTS),
        held_announces: StorageCapacity::Fixed(MAX_PENDING_PATH_REQUESTS),
        ratchets_per_destination: StorageCapacity::Fixed(RETAINED_RATCHETS_PER_DESTINATION),
    };

    type Routes = FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>;
    type Announces = FixedArrayRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS>;
    type History = TieredAnnounceIdHistory<
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
    >;
    type AppData = PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>;
    type ScheduledAnnounces = FixedScheduledAnnounceQueue<MAX_TRACKED_DESTINATIONS>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type HeldIdentities = FixedHeldIdentityColumns<MAX_HELD_IDENTITIES>;
    type SelfRatchets =
        FixedSelfRatchetColumns<MAX_UPSTREAM_APP_DESTINATIONS, RETAINED_RATCHETS_PER_DESTINATION>;
    type PacketHashes = FixedPacketHashHistory<PACKET_HASH_GENERATION_CAPACITY>;
    type Receipts = FixedReceiptColumns<MAX_OUTSTANDING_RECEIPTS>;
    type ReverseRoutes = FixedReverseRouteColumns<MAX_REVERSE_ROUTES>;
    type PendingPathRequests = FixedPendingPathRequestColumns<MAX_PENDING_PATH_REQUESTS>;
    type RecentPathRequests = FixedRecentPathRequestColumns<MAX_PENDING_PATH_REQUESTS>;
    type SeenPathRequests = FixedSeenPathRequestColumns<MAX_SEEN_PATH_REQUESTS>;
    type Tunnels = FixedTunnelColumns<8>;
    type DiscoveryPathRequests = FixedDiscoveryPathRequestColumns<MAX_PENDING_PATH_REQUESTS>;
    type InterfacePathRequestLimits =
        FixedInterfacePathRequestLimitColumns<MAX_PENDING_PATH_REQUESTS>;
    type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitColumns<MAX_PENDING_PATH_REQUESTS>;
    type DirtyInterfaces = heapless::Vec<crate::interfaces::InterfaceId, 8>;
    type HeldAnnounces = FixedHeldAnnounceColumns<MAX_PENDING_PATH_REQUESTS>;
    type HeldAnnounceAppData =
        PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_PENDING_PATH_REQUESTS>;
    type AnnounceRates = FixedAnnounceRateColumns<MAX_TRACKED_DESTINATIONS>;
    type GroupKeys = FixedGroupKeyColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type RequestHandlers = FixedRequestHandlerColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type TransportedLinks = FixedTransportedLinkColumns<MAX_LINKS>;
    type Links = FixedLinkColumns<MAX_LINKS>;
    type OutgoingResources = FixedResourceColumns<OutgoingResourceState, 1, 4096, 9>;
    type IncomingResources = FixedResourceColumns<IncomingResourceState, 1, 4096, 9>;
    type IncomingAssemblies = FixedIncomingAssemblyColumns<MAX_LINKS>;
    type OutgoingAssemblies = FixedOutgoingAssemblyColumns<MAX_LINKS>;
    type Channels = FixedArrayChannelColumns<MAX_LINKS, 8, { channel_mdu(8192) }>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::ratchets::SelfRatchetColumns;
    use crate::routing::announce::retained::RetainedAnnounceColumns;
    use crate::routing::dedup::PacketHashHistory;
    use crate::routing::delivery::receipts::ReceiptColumns;
    use crate::routing::links::table::LinkColumns;
    use crate::routing::routes::RouteColumns;
    use crate::routing::upstream_app_destinations::UpstreamAppDestinationColumns;

    #[test]
    fn bundles_fixed_array_backends_sized_by_the_consts() {
        type S = TestFixedStorage<8, 16, 256, 2, 8, 2, 2, 4, 3, 5, 8, 4, 8, 6>;
        let routes = <S as StorageLayout>::Routes::default();
        let announces = <S as StorageLayout>::Announces::default();
        let _history = <S as StorageLayout>::History::default();
        let _app_data = <S as StorageLayout>::AppData::default();
        let _pending = <S as StorageLayout>::ScheduledAnnounces::default();
        let upstream_app_destinations = <S as StorageLayout>::UpstreamAppDestinations::default();
        let packet_hashes = <S as StorageLayout>::PacketHashes::default();
        let self_ratchets = <S as StorageLayout>::SelfRatchets::default();
        assert_eq!(routes.capacity(), 8);
        assert_eq!(announces.capacity(), 8);
        assert_eq!(upstream_app_destinations.capacity(), 2);
        assert_eq!(packet_hashes.generation_capacity(), 4);
        assert_eq!(self_ratchets.capacity(), 2);
        assert_eq!(self_ratchets.retained_per_destination(), 3);
        let receipts = <S as StorageLayout>::Receipts::default();
        assert_eq!(receipts.capacity(), 5);
        let links = <S as StorageLayout>::Links::default();
        assert_eq!(links.capacity(), 6);
    }
}
