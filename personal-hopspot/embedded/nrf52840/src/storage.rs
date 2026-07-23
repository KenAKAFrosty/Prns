use personal_rns::crypto::ratchets::FixedSelfRatchetTable;
use personal_rns::identity::destination_identity::{
    NoDestinationIdentityAppData, NoDestinationIdentityTable,
};
use personal_rns::identity::held::FixedHeldIdentityTable;
use personal_rns::routing::announce::destination_announce_limit::FixedDestinationAnnounceLimitTable;
use personal_rns::routing::announce::held::FixedHeldAnnounceTable;
use personal_rns::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitTable;
use personal_rns::routing::announce::schedule::FixedScheduledAnnounceQueue;
use personal_rns::routing::announce::stored::{
    FixedAnnounceIdHistory, FixedArrayAnnounceRecordTable, PackedAppDataArena,
};
use personal_rns::routing::blackhole::{blackhole_index_buckets, FixedBlackholeTable};
use personal_rns::routing::dedup::FixedPacketHashHistory;
use personal_rns::routing::delivery::receipts::FixedReceiptTable;
use personal_rns::routing::group_keys::FixedGroupKeyTable;
use personal_rns::routing::links::channel::channel_mdu;
use personal_rns::routing::links::channel::table::impls::FixedArrayChannelTable;
use personal_rns::routing::links::resources::assembly::{
    FixedIncomingAssemblyTable, FixedOutgoingAssemblyTable,
};
use personal_rns::routing::links::resources::max_part_count;
use personal_rns::routing::links::resources::table::{
    FixedResourceTable, IncomingResourceState, OutgoingResourceState,
};
use personal_rns::routing::links::table::FixedLinkTable;
use personal_rns::routing::links::transported::FixedTransportedLinkTable;
use personal_rns::routing::path_requests::interface_path_request_limit::FixedInterfacePathRequestLimitTable;
use personal_rns::routing::path_requests::pending::FixedPendingPathRequestTable;
use personal_rns::routing::path_requests::recent::FixedRecentPathRequestTable;
use personal_rns::routing::path_requests::recursive::FixedRecursivePathRequestTable;
use personal_rns::routing::path_requests::seen::FixedSeenPathRequestTable;
use personal_rns::routing::request_handlers::FixedRequestHandlerTable;
use personal_rns::routing::reverse_routes::FixedReverseRouteTable;
use personal_rns::routing::routes::FixedArrayRouteTable;
use personal_rns::routing::tunnel::FixedTunnelTable;
use personal_rns::routing::upstream_app_destinations::FixedUpstreamAppDestinationTable;
use personal_rns::routing::warmth::FixedDepartedInterfaceTable;
use personal_rns::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

pub struct TechoStorage;

const _: () = assert!(
    personal_hopspot_core::node_pages::INDEX_RESPONSE_TRANSFER_BYTES
        <= TechoStorage::RESOURCE_TRANSFER_BYTES
);

impl TechoStorage {
    const TRACKED_DESTINATIONS: usize = 8;
    const UPSTREAM_APP_DESTINATIONS: usize = 2;
    const LINKS: usize = 2;
    const BLACKHOLED_IDENTITIES: usize = 0;
    const BLACKHOLE_REASON_BYTES: usize = 0;
    const HELD_ANNOUNCES: usize = 16;
    const HELD_ANNOUNCE_APP_DATA_BYTES: usize = Self::HELD_ANNOUNCES * 64;
    const RESOURCE_TRANSFER_BYTES: usize = 2048;
    const CHANNEL_REORDER_DEPTH: usize = 2;
    const LINK_MTU: usize = 1024;
    const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(Self::LINK_MTU);
}

impl StorageLayout for TechoStorage {
    const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
        tracked_destinations: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
        destination_identities: StorageCapacity::Fixed(0),
        announce_records: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
        upstream_app_destinations: StorageCapacity::Fixed(Self::UPSTREAM_APP_DESTINATIONS),
        held_identities: StorageCapacity::Fixed(2),
        links: StorageCapacity::Fixed(Self::LINKS),
        channels: StorageCapacity::Fixed(Self::LINKS),
        channel_window_pool: None,
        channel_reorder_depth: StorageCapacity::Fixed(Self::CHANNEL_REORDER_DEPTH),
        link_mtu: StorageCapacity::Fixed(Self::LINK_MTU),
        resource_transfer_bytes: StorageCapacity::Fixed(Self::RESOURCE_TRANSFER_BYTES),
        receipts: StorageCapacity::Fixed(4),
        packet_hashes: StorageCapacity::Fixed(16),
        blackholed_identities: StorageCapacity::Fixed(Self::BLACKHOLED_IDENTITIES),
        blackhole_reason_bytes: StorageCapacity::Fixed(Self::BLACKHOLE_REASON_BYTES),
        reverse_routes: StorageCapacity::Fixed(4),
        pending_path_requests: StorageCapacity::Fixed(4),
        held_announces: StorageCapacity::Fixed(Self::HELD_ANNOUNCES),
        ratchets_per_destination: StorageCapacity::Fixed(4),
    };

    type Routes = FixedArrayRouteTable<{ Self::TRACKED_DESTINATIONS }>;
    type RouteExpiries = personal_rns::routing::LinearRouteExpiryIndex;
    type DestinationIdentities = NoDestinationIdentityTable;
    type DestinationIdentityAppData = NoDestinationIdentityAppData;
    type Tunnels = FixedTunnelTable<0>;
    type Announces = FixedArrayAnnounceRecordTable<{ Self::TRACKED_DESTINATIONS }>;
    type History = FixedAnnounceIdHistory<{ Self::TRACKED_DESTINATIONS }, 8>;
    type AppData = PackedAppDataArena<256, { Self::TRACKED_DESTINATIONS }>;
    type ScheduledAnnounces = FixedScheduledAnnounceQueue<{ Self::TRACKED_DESTINATIONS }>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationTable<{ Self::UPSTREAM_APP_DESTINATIONS }>;
    type HeldIdentities = FixedHeldIdentityTable<2>;
    type SelfRatchets = FixedSelfRatchetTable<{ Self::UPSTREAM_APP_DESTINATIONS }, 4>;
    type Receipts = FixedReceiptTable<4>;
    type PacketHashes = FixedPacketHashHistory<16>;
    type Blackholes = FixedBlackholeTable<
        { Self::BLACKHOLED_IDENTITIES },
        { blackhole_index_buckets(Self::BLACKHOLED_IDENTITIES) },
        { Self::BLACKHOLE_REASON_BYTES },
    >;
    type ReverseRoutes = FixedReverseRouteTable<4>;
    type DepartedInterfaces = FixedDepartedInterfaceTable<4>;
    type PendingPathRequests = FixedPendingPathRequestTable<4>;
    type RecentPathRequests = FixedRecentPathRequestTable<4>;
    type SeenPathRequests = FixedSeenPathRequestTable<4>;
    type RecursivePathRequests = FixedRecursivePathRequestTable<4>;
    type InterfacePathRequestLimits = FixedInterfacePathRequestLimitTable<4>;
    type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitTable<4>;
    type DirtyInterfaces = heapless::Vec<personal_rns::interfaces::InterfaceId, 4>;
    type HeldAnnounces = FixedHeldAnnounceTable<{ Self::HELD_ANNOUNCES }>;
    type HeldAnnounceAppData =
        PackedAppDataArena<{ Self::HELD_ANNOUNCE_APP_DATA_BYTES }, { Self::HELD_ANNOUNCES }>;
    type DestinationAnnounceLimits =
        FixedDestinationAnnounceLimitTable<{ Self::TRACKED_DESTINATIONS }>;
    type GroupKeys = FixedGroupKeyTable<{ Self::UPSTREAM_APP_DESTINATIONS }>;
    type RequestHandlers = FixedRequestHandlerTable<{ Self::UPSTREAM_APP_DESTINATIONS }>;
    type TransportedLinks = FixedTransportedLinkTable<{ Self::LINKS }>;
    type Links = FixedLinkTable<{ Self::LINKS }>;
    type OutgoingResources = FixedResourceTable<
        OutgoingResourceState,
        1,
        { Self::RESOURCE_TRANSFER_BYTES },
        { max_part_count(Self::RESOURCE_TRANSFER_BYTES) },
    >;
    type IncomingResources = FixedResourceTable<
        IncomingResourceState,
        1,
        { Self::RESOURCE_TRANSFER_BYTES },
        { max_part_count(Self::RESOURCE_TRANSFER_BYTES) },
    >;
    type IncomingAssemblies = FixedIncomingAssemblyTable<{ Self::LINKS }>;
    type OutgoingAssemblies = FixedOutgoingAssemblyTable<{ Self::LINKS }>;
    type Channels = FixedArrayChannelTable<
        { Self::LINKS },
        { Self::CHANNEL_REORDER_DEPTH },
        { Self::CHANNEL_MESSAGE_BYTES },
    >;
}
