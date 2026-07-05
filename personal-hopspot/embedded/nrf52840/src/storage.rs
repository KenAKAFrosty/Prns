use personal_rns::crypto::ratchets::FixedSelfRatchetColumns;
use personal_rns::identity::held::FixedHeldIdentityColumns;
use personal_rns::routing::announce::destination_announce_limit::FixedDestinationAnnounceLimitColumns;
use personal_rns::routing::announce::held::FixedHeldAnnounceColumns;
use personal_rns::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitColumns;
use personal_rns::routing::announce::retained::{
    FixedArrayRetainedAnnounceColumns, PackedAppDataArena, TieredAnnounceIdHistory,
};
use personal_rns::routing::announce::schedule::FixedScheduledAnnounceQueue;
use personal_rns::routing::dedup::FixedPacketHashHistory;
use personal_rns::routing::delivery::receipts::FixedReceiptColumns;
use personal_rns::routing::group_keys::FixedGroupKeyColumns;
use personal_rns::routing::links::channel::channel_mdu;
use personal_rns::routing::links::channel::impls::FixedArrayChannelColumns;
use personal_rns::routing::links::resources::assembly::{
    FixedIncomingAssemblyColumns, FixedOutgoingAssemblyColumns,
};
use personal_rns::routing::links::resources::max_part_count;
use personal_rns::routing::links::resources::table::{
    FixedResourceColumns, IncomingResourceState, OutgoingResourceState,
};
use personal_rns::routing::links::table::FixedLinkColumns;
use personal_rns::routing::links::transported::FixedTransportedLinkColumns;
use personal_rns::routing::path_requests::discovery::FixedDiscoveryPathRequestColumns;
use personal_rns::routing::path_requests::interface_path_request_limit::FixedInterfacePathRequestLimitColumns;
use personal_rns::routing::path_requests::pending::FixedPendingPathRequestColumns;
use personal_rns::routing::path_requests::recent::FixedRecentPathRequestColumns;
use personal_rns::routing::path_requests::seen::FixedSeenPathRequestColumns;
use personal_rns::routing::request_handlers::FixedRequestHandlerColumns;
use personal_rns::routing::reverse_routes::FixedReverseRouteColumns;
use personal_rns::routing::routes::FixedArrayRouteColumns;
use personal_rns::routing::tunnel::FixedTunnelColumns;
use personal_rns::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;
use personal_rns::routing::warmth::FixedDepartedInterfaceColumns;
use personal_rns::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

pub struct TechoStorage;

impl TechoStorage {
    const TRACKED_DESTINATIONS: usize = 8;
    const UPSTREAM_APP_DESTINATIONS: usize = 2;
    const LINKS: usize = 2;
    const RESOURCE_TRANSFER_BYTES: usize = 1024;
    const CHANNEL_REORDER_DEPTH: usize = 2;
    const LINK_MTU: usize = 1024;
    const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(Self::LINK_MTU);
}

impl StorageLayout for TechoStorage {
    const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
        tracked_destinations: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
        retained_announces: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
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
        reverse_routes: StorageCapacity::Fixed(4),
        pending_path_requests: StorageCapacity::Fixed(4),
        held_announces: StorageCapacity::Fixed(4),
        ratchets_per_destination: StorageCapacity::Fixed(4),
    };

    type Routes = FixedArrayRouteColumns<{ Self::TRACKED_DESTINATIONS }>;
    type Tunnels = FixedTunnelColumns<0>;
    type Announces = FixedArrayRetainedAnnounceColumns<{ Self::TRACKED_DESTINATIONS }>;
    type History = TieredAnnounceIdHistory<4, 32, { Self::TRACKED_DESTINATIONS }, 16>;
    type AppData = PackedAppDataArena<256, { Self::TRACKED_DESTINATIONS }>;
    type ScheduledAnnounces = FixedScheduledAnnounceQueue<{ Self::TRACKED_DESTINATIONS }>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationColumns<{ Self::UPSTREAM_APP_DESTINATIONS }>;
    type HeldIdentities = FixedHeldIdentityColumns<2>;
    type SelfRatchets = FixedSelfRatchetColumns<{ Self::UPSTREAM_APP_DESTINATIONS }, 4>;
    type Receipts = FixedReceiptColumns<4>;
    type PacketHashes = FixedPacketHashHistory<16>;
    type ReverseRoutes = FixedReverseRouteColumns<4>;
    type DepartedInterfaces = FixedDepartedInterfaceColumns<4>;
    type PendingPathRequests = FixedPendingPathRequestColumns<4>;
    type RecentPathRequests = FixedRecentPathRequestColumns<4>;
    type SeenPathRequests = FixedSeenPathRequestColumns<4>;
    type DiscoveryPathRequests = FixedDiscoveryPathRequestColumns<4>;
    type InterfacePathRequestLimits = FixedInterfacePathRequestLimitColumns<4>;
    type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitColumns<4>;
    type DirtyInterfaces = heapless::Vec<personal_rns::interfaces::InterfaceId, 4>;
    type HeldAnnounces = FixedHeldAnnounceColumns<4>;
    type HeldAnnounceAppData = PackedAppDataArena<256, 4>;
    type DestinationAnnounceLimits =
        FixedDestinationAnnounceLimitColumns<{ Self::TRACKED_DESTINATIONS }>;
    type GroupKeys = FixedGroupKeyColumns<{ Self::UPSTREAM_APP_DESTINATIONS }>;
    type RequestHandlers = FixedRequestHandlerColumns<{ Self::UPSTREAM_APP_DESTINATIONS }>;
    type TransportedLinks = FixedTransportedLinkColumns<{ Self::LINKS }>;
    type Links = FixedLinkColumns<{ Self::LINKS }>;
    type OutgoingResources = FixedResourceColumns<
        OutgoingResourceState,
        1,
        { Self::RESOURCE_TRANSFER_BYTES },
        { max_part_count(Self::RESOURCE_TRANSFER_BYTES) },
    >;
    type IncomingResources = FixedResourceColumns<
        IncomingResourceState,
        1,
        { Self::RESOURCE_TRANSFER_BYTES },
        { max_part_count(Self::RESOURCE_TRANSFER_BYTES) },
    >;
    type IncomingAssemblies = FixedIncomingAssemblyColumns<{ Self::LINKS }>;
    type OutgoingAssemblies = FixedOutgoingAssemblyColumns<{ Self::LINKS }>;
    type Channels = FixedArrayChannelColumns<
        { Self::LINKS },
        { Self::CHANNEL_REORDER_DEPTH },
        { Self::CHANNEL_MESSAGE_BYTES },
    >;
}
