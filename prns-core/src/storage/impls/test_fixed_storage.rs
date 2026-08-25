use crate::crypto::ratchets::FixedSelfRatchetTable;
use crate::identity::destination_identity::FixedArrayDestinationIdentityTable;
use crate::identity::held::FixedHeldIdentityTable;
use crate::routing::announce::destination_announce_limit::FixedDestinationAnnounceLimitTable;
use crate::routing::announce::held::FixedHeldAnnounceTable;
use crate::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitTable;
use crate::routing::announce::schedule::FixedScheduledAnnounceQueue;
use crate::routing::announce::stored::{
    FixedAnnounceIdHistory, FixedArrayAnnounceRecordTable, PackedAppDataArena,
};
use crate::routing::blackhole::{blackhole_index_buckets, FixedBlackholeTable};
use crate::routing::dedup::FixedPacketHashHistory;
use crate::routing::delivery::receipts::FixedReceiptTable;
use crate::routing::group_keys::FixedGroupKeyTable;
use crate::routing::links::channel::channel_mdu;
use crate::routing::links::channel::table::impls::FixedArrayChannelTable;
use crate::routing::links::resources::assembly::{
    FixedIncomingAssemblyTable, FixedOutgoingAssemblyTable,
};
use crate::routing::links::resources::max_part_count;
use crate::routing::links::resources::pending::FixedPendingResourceOfferTable;
use crate::routing::links::resources::table::{
    FixedResourceTable, IncomingResourceState, OutgoingResourceState,
};
use crate::routing::links::table::FixedLinkTable;
use crate::routing::links::transported::FixedTransportedLinkTable;
use crate::routing::path_requests::interface_path_request_limit::FixedInterfacePathRequestLimitTable;
use crate::routing::path_requests::pending::FixedPendingPathRequestTable;
use crate::routing::path_requests::recent::FixedRecentPathRequestTable;
use crate::routing::path_requests::recursive::FixedRecursivePathRequestTable;
use crate::routing::path_requests::seen::FixedSeenPathRequestTable;
use crate::routing::request_handlers::FixedRequestHandlerTable;
use crate::routing::reverse_routes::FixedReverseRouteTable;
use crate::routing::route_expiry::LinearRouteExpiryIndex;
use crate::routing::routes::FixedArrayRouteTable;
use crate::routing::tunnel::FixedTunnelTable;
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationTable;
use crate::routing::warmth::FixedDepartedInterfaceTable;
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
        destination_identities: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
        announce_records: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
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
        blackholed_identities: StorageCapacity::Fixed(8),
        blackhole_reason_bytes: StorageCapacity::Fixed(64),
        reverse_routes: StorageCapacity::Fixed(MAX_REVERSE_ROUTES),
        pending_path_requests: StorageCapacity::Fixed(MAX_PENDING_PATH_REQUESTS),
        held_announces: StorageCapacity::Fixed(MAX_PENDING_PATH_REQUESTS),
        ratchets_per_destination: StorageCapacity::Fixed(RETAINED_RATCHETS_PER_DESTINATION),
    };

    type Routes = FixedArrayRouteTable<MAX_TRACKED_DESTINATIONS>;
    type RouteExpiries = LinearRouteExpiryIndex;
    type DestinationIdentities = FixedArrayDestinationIdentityTable<MAX_TRACKED_DESTINATIONS>;
    type DestinationIdentityAppData =
        PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>;
    type Announces = FixedArrayAnnounceRecordTable<MAX_TRACKED_DESTINATIONS>;
    type History =
        FixedAnnounceIdHistory<MAX_TRACKED_DESTINATIONS, MAX_ANNOUNCE_IDS_PER_DESTINATION>;
    type AppData = PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>;
    type ScheduledAnnounces = FixedScheduledAnnounceQueue<MAX_TRACKED_DESTINATIONS>;
    type UpstreamAppDestinations = FixedUpstreamAppDestinationTable<MAX_UPSTREAM_APP_DESTINATIONS>;
    type HeldIdentities = FixedHeldIdentityTable<MAX_HELD_IDENTITIES>;
    type SelfRatchets =
        FixedSelfRatchetTable<MAX_UPSTREAM_APP_DESTINATIONS, RETAINED_RATCHETS_PER_DESTINATION>;
    type PacketHashes = FixedPacketHashHistory<PACKET_HASH_GENERATION_CAPACITY>;
    type Blackholes = FixedBlackholeTable<8, { blackhole_index_buckets(8) }, 64>;
    type Receipts = FixedReceiptTable<MAX_OUTSTANDING_RECEIPTS>;
    type ReverseRoutes = FixedReverseRouteTable<MAX_REVERSE_ROUTES>;
    type DepartedInterfaces = FixedDepartedInterfaceTable<16>;
    type PendingPathRequests = FixedPendingPathRequestTable<MAX_PENDING_PATH_REQUESTS>;
    type RecentPathRequests = FixedRecentPathRequestTable<MAX_PENDING_PATH_REQUESTS>;
    type SeenPathRequests = FixedSeenPathRequestTable<MAX_SEEN_PATH_REQUESTS>;
    type Tunnels = FixedTunnelTable<8>;
    type RecursivePathRequests = FixedRecursivePathRequestTable<MAX_PENDING_PATH_REQUESTS>;
    type InterfacePathRequestLimits =
        FixedInterfacePathRequestLimitTable<MAX_PENDING_PATH_REQUESTS>;
    type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitTable<MAX_PENDING_PATH_REQUESTS>;
    type DirtyInterfaces = heapless::Vec<crate::interfaces::InterfaceId, 8>;
    type HeldAnnounces = FixedHeldAnnounceTable<MAX_PENDING_PATH_REQUESTS>;
    type HeldAnnounceAppData =
        PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_PENDING_PATH_REQUESTS>;
    type DestinationAnnounceLimits = FixedDestinationAnnounceLimitTable<MAX_TRACKED_DESTINATIONS>;
    type GroupKeys = FixedGroupKeyTable<MAX_UPSTREAM_APP_DESTINATIONS>;
    type RequestHandlers = FixedRequestHandlerTable<MAX_UPSTREAM_APP_DESTINATIONS>;
    type TransportedLinks = FixedTransportedLinkTable<MAX_LINKS>;
    type Links = FixedLinkTable<MAX_LINKS>;
    type OutgoingResources = FixedResourceTable<
        OutgoingResourceState,
        1,
        RESOURCE_TRANSFER_BYTES,
        { max_part_count(RESOURCE_TRANSFER_BYTES) },
    >;
    type IncomingResources = FixedResourceTable<
        IncomingResourceState,
        2,
        RESOURCE_TRANSFER_BYTES,
        { max_part_count(RESOURCE_TRANSFER_BYTES) },
    >;
    type PendingResourceOffers = FixedPendingResourceOfferTable<4>;
    type IncomingAssemblies = FixedIncomingAssemblyTable<MAX_LINKS>;
    type OutgoingAssemblies = FixedOutgoingAssemblyTable<MAX_LINKS>;
    type Channels =
        FixedArrayChannelTable<MAX_LINKS, CHANNEL_REORDER_DEPTH, { channel_mdu(LINK_MTU) }>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::ratchets::SelfRatchetTable;
    use crate::identity::destination_identity::DestinationIdentityTable;
    use crate::routing::announce::stored::AnnounceRecordTable;
    use crate::routing::blackhole::{BlackholeTable, FixedBlackholeTable};
    use crate::routing::dedup::PacketHashHistory;
    use crate::routing::delivery::receipts::ReceiptTable;
    use crate::routing::links::table::LinkTable;
    use crate::routing::routes::RouteTable;
    use crate::routing::upstream_app_destinations::UpstreamAppDestinationTable;

    #[test]
    fn bundles_fixed_array_backends_sized_by_the_consts() {
        type S = TestFixedStorage<8, 16, 256, 2, 2, 4, 3, 5, 8, 4, 8, 6>;
        let routes = <S as StorageLayout>::Routes::default();
        let announces = <S as StorageLayout>::Announces::default();
        let destination_identities = <S as StorageLayout>::DestinationIdentities::default();
        let _history = <S as StorageLayout>::History::default();
        let _app_data = <S as StorageLayout>::AppData::default();
        let _pending = <S as StorageLayout>::ScheduledAnnounces::default();
        let upstream_app_destinations = <S as StorageLayout>::UpstreamAppDestinations::default();
        let packet_hashes = <S as StorageLayout>::PacketHashes::default();
        let blackholes: FixedBlackholeTable<8, { blackhole_index_buckets(8) }, 64> =
            <S as StorageLayout>::Blackholes::default();
        let self_ratchets = <S as StorageLayout>::SelfRatchets::default();
        assert_eq!(routes.capacity(), 8);
        assert_eq!(announces.capacity(), 8);
        assert_eq!(destination_identities.capacity(), 8);
        assert_eq!(upstream_app_destinations.capacity(), 2);
        assert_eq!(packet_hashes.generation_capacity(), 4);
        assert!(blackholes.is_empty());
        assert_eq!(
            <S as StorageLayout>::LIMITS.blackholed_identities,
            StorageCapacity::Fixed(8)
        );
        assert_eq!(
            <S as StorageLayout>::LIMITS.blackhole_reason_bytes,
            StorageCapacity::Fixed(64)
        );
        assert_eq!(self_ratchets.capacity(), 2);
        assert_eq!(self_ratchets.retained_per_destination(), 3);
        let receipts = <S as StorageLayout>::Receipts::default();
        assert_eq!(receipts.capacity(), 5);
        let links = <S as StorageLayout>::Links::default();
        assert_eq!(links.capacity(), 6);
    }
}
