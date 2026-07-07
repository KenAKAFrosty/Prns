use crate::crypto::ratchets::FixedSelfRatchetColumns;
use crate::identity::held::FixedHeldIdentityColumns;
use crate::routing::announce::destination_announce_limit::FixedDestinationAnnounceLimitColumns;
use crate::routing::announce::held::FixedHeldStore;
use crate::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitColumns;
use crate::routing::announce::schedule::FixedScheduledAnnounceQueue;
use crate::routing::announce::stored::{
    FixedAnnounceIdHistory, FixedArrayAnnounceRecordColumns, PackedAppDataArena,
};
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
use crate::routing::path_requests::interface_path_request_limit::FixedInterfacePathRequestLimitColumns;
use crate::routing::path_requests::pending::FixedPendingPathRequestColumns;
use crate::routing::path_requests::recent::FixedRecentPathRequestColumns;
use crate::routing::path_requests::recursive::FixedRecursivePathRequestColumns;
use crate::routing::path_requests::seen::FixedSeenPathRequestColumns;
use crate::routing::request_handlers::FixedRequestHandlerColumns;
use crate::routing::reverse_routes::FixedReverseRouteColumns;
use crate::routing::routes::FixedArrayRouteColumns;
use crate::routing::tunnel::FixedTunnelColumns;
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;
use crate::routing::warmth::FixedDepartedInterfaceColumns;
use crate::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

pub struct Esp32C6;

impl Esp32C6 {
    const TRACKED_DESTINATIONS: usize = 24;
    const UPSTREAM_APP_DESTINATIONS: usize = 4;
    const HELD_IDENTITIES: usize = 4;
    const LINKS: usize = 4;
    const RESOURCE_TRANSFER_BYTES: usize = 4096;
    const CHANNEL_REORDER_DEPTH: usize = 8;
    const LINK_MTU: usize = 8192;
    const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(Self::LINK_MTU);
    const RECEIPTS: usize = 8;
    const PACKET_HASHES: usize = 32;
    const REVERSE_ROUTES: usize = 8;
    const PENDING_PATH_REQUESTS: usize = 8;
    const HELD_ANNOUNCES: usize = 32;
    const RATCHETS_PER_DESTINATION: usize = 8;
}

impl StorageLayout for Esp32C6 {
    const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
        tracked_destinations: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
        announce_records: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
        upstream_app_destinations: StorageCapacity::Fixed(Self::UPSTREAM_APP_DESTINATIONS),
        held_identities: StorageCapacity::Fixed(Self::HELD_IDENTITIES),
        links: StorageCapacity::Fixed(Self::LINKS),
        channels: StorageCapacity::Fixed(Self::LINKS),
        channel_window_pool: None,
        channel_reorder_depth: StorageCapacity::Fixed(Self::CHANNEL_REORDER_DEPTH),
        link_mtu: StorageCapacity::Fixed(Self::LINK_MTU),
        resource_transfer_bytes: StorageCapacity::Fixed(Self::RESOURCE_TRANSFER_BYTES),
        receipts: StorageCapacity::Fixed(Self::RECEIPTS),
        packet_hashes: StorageCapacity::Fixed(Self::PACKET_HASHES),
        reverse_routes: StorageCapacity::Fixed(Self::REVERSE_ROUTES),
        pending_path_requests: StorageCapacity::Fixed(Self::PENDING_PATH_REQUESTS),
        held_announces: StorageCapacity::Fixed(Self::HELD_ANNOUNCES),
        ratchets_per_destination: StorageCapacity::Fixed(Self::RATCHETS_PER_DESTINATION),
    };

    type Routes = FixedArrayRouteColumns<{ Self::TRACKED_DESTINATIONS }>;
    type Announces = FixedArrayAnnounceRecordColumns<{ Self::TRACKED_DESTINATIONS }>;
    type History = FixedAnnounceIdHistory<{ Self::TRACKED_DESTINATIONS }, 8>;
    type AppData = PackedAppDataArena<1024, { Self::TRACKED_DESTINATIONS }>;
    type ScheduledAnnounces = FixedScheduledAnnounceQueue<{ Self::TRACKED_DESTINATIONS }>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationColumns<{ Self::UPSTREAM_APP_DESTINATIONS }>;
    type HeldIdentities = FixedHeldIdentityColumns<{ Self::HELD_IDENTITIES }>;
    type SelfRatchets = FixedSelfRatchetColumns<
        { Self::UPSTREAM_APP_DESTINATIONS },
        { Self::RATCHETS_PER_DESTINATION },
    >;
    type Receipts = FixedReceiptColumns<{ Self::RECEIPTS }>;
    type PacketHashes = FixedPacketHashHistory<{ Self::PACKET_HASHES }>;
    type ReverseRoutes = FixedReverseRouteColumns<{ Self::REVERSE_ROUTES }>;
    type DepartedInterfaces = FixedDepartedInterfaceColumns<8>;
    type PendingPathRequests = FixedPendingPathRequestColumns<{ Self::PENDING_PATH_REQUESTS }>;
    type RecentPathRequests = FixedRecentPathRequestColumns<8>;
    type SeenPathRequests = FixedSeenPathRequestColumns<8>;
    type Tunnels = FixedTunnelColumns<0>;
    type RecursivePathRequests = FixedRecursivePathRequestColumns<8>;
    type InterfacePathRequestLimits = FixedInterfacePathRequestLimitColumns<8>;
    type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitColumns<8>;
    type DirtyInterfaces = heapless::Vec<crate::interfaces::InterfaceId, 8>;
    type HeldAnnounces = FixedHeldStore<{ Self::HELD_ANNOUNCES }>;
    type HeldAnnounceAppData = PackedAppDataArena<4096, { Self::HELD_ANNOUNCES }>;
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
