use crate::crypto::ratchets::FixedSelfRatchetColumns;
use crate::identity::held::FixedHeldIdentityColumns;
use crate::routing::announce::destination_announce_limit::FixedDestinationAnnounceLimitColumns;
use crate::routing::announce::held::FixedHeldAnnouncePool;
use crate::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitColumns;
use crate::routing::announce::retained::{
    FixedAnnounceIdHistory, FixedArrayRetainedAnnounceColumns, PackedAppDataArena,
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
use crate::routing::links::resources::max_part_count;
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
use crate::routing::warmth::FixedDepartedInterfaceColumns;
use crate::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

const CHANNEL_REORDER_DEPTH: usize = 8;
const LINK_MTU: usize = 8192;
const RESOURCE_TRANSFER_BYTES: usize = 4096;

pub struct TestFixedStorage<
    const MAX_TRACKED_DESTINATIONS: usize,
    const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
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
    const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
        tracked_destinations: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
        retained_announces: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
        upstream_app_destinations: StorageCapacity::Fixed(MAX_UPSTREAM_APP_DESTINATIONS),
        held_identities: StorageCapacity::Fixed(MAX_HELD_IDENTITIES),
        links: StorageCapacity::Fixed(MAX_LINKS),
        channels: StorageCapacity::Fixed(MAX_LINKS),
        channel_window_pool: None,
        channel_reorder_depth: StorageCapacity::Fixed(CHANNEL_REORDER_DEPTH),
        link_mtu: StorageCapacity::Fixed(LINK_MTU),
        resource_transfer_bytes: StorageCapacity::Fixed(RESOURCE_TRANSFER_BYTES),
        receipts: StorageCapacity::Fixed(MAX_OUTSTANDING_RECEIPTS),
        packet_hashes: StorageCapacity::Fixed(PACKET_HASH_GENERATION_CAPACITY),
        reverse_routes: StorageCapacity::Fixed(MAX_REVERSE_ROUTES),
        pending_path_requests: StorageCapacity::Fixed(MAX_PENDING_PATH_REQUESTS),
        held_announces: StorageCapacity::Fixed(MAX_PENDING_PATH_REQUESTS),
        ratchets_per_destination: StorageCapacity::Fixed(RETAINED_RATCHETS_PER_DESTINATION),
    };

    type Routes = FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>;
    type Announces = FixedArrayRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS>;
    type History =
        FixedAnnounceIdHistory<MAX_TRACKED_DESTINATIONS, MAX_ANNOUNCE_IDS_PER_DESTINATION>;
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
    type DepartedInterfaces = FixedDepartedInterfaceColumns<16>;
    type PendingPathRequests = FixedPendingPathRequestColumns<MAX_PENDING_PATH_REQUESTS>;
    type RecentPathRequests = FixedRecentPathRequestColumns<MAX_PENDING_PATH_REQUESTS>;
    type SeenPathRequests = FixedSeenPathRequestColumns<MAX_SEEN_PATH_REQUESTS>;
    type Tunnels = FixedTunnelColumns<8>;
    type DiscoveryPathRequests = FixedDiscoveryPathRequestColumns<MAX_PENDING_PATH_REQUESTS>;
    type InterfacePathRequestLimits =
        FixedInterfacePathRequestLimitColumns<MAX_PENDING_PATH_REQUESTS>;
    type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitColumns<MAX_PENDING_PATH_REQUESTS>;
    type DirtyInterfaces = heapless::Vec<crate::interfaces::InterfaceId, 8>;
    type HeldAnnounces = FixedHeldAnnouncePool<MAX_PENDING_PATH_REQUESTS>;
    type HeldAnnounceAppData =
        PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_PENDING_PATH_REQUESTS>;
    type DestinationAnnounceLimits = FixedDestinationAnnounceLimitColumns<MAX_TRACKED_DESTINATIONS>;
    type GroupKeys = FixedGroupKeyColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type RequestHandlers = FixedRequestHandlerColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type TransportedLinks = FixedTransportedLinkColumns<MAX_LINKS>;
    type Links = FixedLinkColumns<MAX_LINKS>;
    type OutgoingResources = FixedResourceColumns<
        OutgoingResourceState,
        1,
        RESOURCE_TRANSFER_BYTES,
        { max_part_count(RESOURCE_TRANSFER_BYTES) },
    >;
    type IncomingResources = FixedResourceColumns<
        IncomingResourceState,
        1,
        RESOURCE_TRANSFER_BYTES,
        { max_part_count(RESOURCE_TRANSFER_BYTES) },
    >;
    type IncomingAssemblies = FixedIncomingAssemblyColumns<MAX_LINKS>;
    type OutgoingAssemblies = FixedOutgoingAssemblyColumns<MAX_LINKS>;
    type Channels =
        FixedArrayChannelColumns<MAX_LINKS, CHANNEL_REORDER_DEPTH, { channel_mdu(LINK_MTU) }>;
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
        type S = TestFixedStorage<8, 16, 256, 2, 2, 4, 3, 5, 8, 4, 8, 6>;
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
