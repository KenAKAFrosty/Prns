use crate::crypto::ratchets::FixedSelfRatchetTable;
use crate::identity::held::FixedHeldIdentityTable;
use crate::routing::announce::destination_announce_limit::FixedDestinationAnnounceLimitTable;
use crate::routing::announce::held::FixedHeldAnnounceTable;
use crate::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitTable;
use crate::routing::announce::schedule::FixedScheduledAnnounceQueue;
use crate::routing::announce::stored::{
    FixedAnnounceIdHistory, FixedArrayAnnounceRecordTable, PackedAppDataArena,
};
use crate::routing::dedup::FixedPacketHashHistory;
use crate::routing::delivery::receipts::FixedReceiptTable;
use crate::routing::group_keys::FixedGroupKeyTable;
use crate::routing::links::channel::channel_mdu;
use crate::routing::links::channel::table::impls::FixedArrayChannelTable;
use crate::routing::links::resources::assembly::{
    FixedIncomingAssemblyTable, FixedOutgoingAssemblyTable,
};
use crate::routing::links::resources::max_part_count;
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
use crate::routing::routes::FixedArrayRouteTable;
use crate::routing::tunnel::FixedTunnelTable;
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationTable;
use crate::routing::warmth::FixedDepartedInterfaceTable;
use crate::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

pub struct Nrf52840;

impl Nrf52840 {
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

impl StorageLayout for Nrf52840 {
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

    type Routes = FixedArrayRouteTable<{ Self::TRACKED_DESTINATIONS }>;
    type Announces = FixedArrayAnnounceRecordTable<{ Self::TRACKED_DESTINATIONS }>;
    type History = FixedAnnounceIdHistory<{ Self::TRACKED_DESTINATIONS }, 8>;
    type AppData = PackedAppDataArena<1024, { Self::TRACKED_DESTINATIONS }>;
    type ScheduledAnnounces = FixedScheduledAnnounceQueue<{ Self::TRACKED_DESTINATIONS }>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationTable<{ Self::UPSTREAM_APP_DESTINATIONS }>;
    type HeldIdentities = FixedHeldIdentityTable<{ Self::HELD_IDENTITIES }>;
    type SelfRatchets = FixedSelfRatchetTable<
        { Self::UPSTREAM_APP_DESTINATIONS },
        { Self::RATCHETS_PER_DESTINATION },
    >;
    type Receipts = FixedReceiptTable<{ Self::RECEIPTS }>;
    type PacketHashes = FixedPacketHashHistory<{ Self::PACKET_HASHES }>;
    type ReverseRoutes = FixedReverseRouteTable<{ Self::REVERSE_ROUTES }>;
    type DepartedInterfaces = FixedDepartedInterfaceTable<8>;
    type PendingPathRequests = FixedPendingPathRequestTable<{ Self::PENDING_PATH_REQUESTS }>;
    type RecentPathRequests = FixedRecentPathRequestTable<8>;
    type SeenPathRequests = FixedSeenPathRequestTable<8>;
    type Tunnels = FixedTunnelTable<0>;
    type RecursivePathRequests = FixedRecursivePathRequestTable<8>;
    type InterfacePathRequestLimits = FixedInterfacePathRequestLimitTable<8>;
    type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitTable<8>;
    type DirtyInterfaces = heapless::Vec<crate::interfaces::InterfaceId, 8>;
    type HeldAnnounces = FixedHeldAnnounceTable<{ Self::HELD_ANNOUNCES }>;
    type HeldAnnounceAppData = PackedAppDataArena<4096, { Self::HELD_ANNOUNCES }>;
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
