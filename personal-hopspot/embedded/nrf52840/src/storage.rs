use personal_rns::crypto::ratchets::FixedSelfRatchetTable;
use personal_rns::identity::destination_identity::{
    NoDestinationIdentityAppData, NoDestinationIdentityTable,
};
use personal_rns::identity::held::FixedHeldIdentityTable;
use personal_rns::remote_control::REMOTE_CONTROL_REQUIRED_HELD_IDENTITY_CAPACITY;
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
    FixedIncomingAssemblyTable, FixedStaticOutgoingAssemblyTable,
};
use personal_rns::routing::links::resources::pending::{
    NoPendingResourceOfferTable, PendingResourceOffers,
};
use personal_rns::routing::links::resources::table::{
    FixedResourceTable, IncomingResourceState, OutgoingResourceState,
};
use personal_rns::routing::links::resources::{
    max_outgoing_resource_reaction_frames, max_part_count,
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
use personal_rns::runtime::request_endpoints::RequestEndpointSet;
use personal_rns::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

pub(crate) struct Nrf52840Storage;

impl Nrf52840Storage {
    // Relationship tables are intentionally independent from transfer workspaces. An idle link is
    // cheap; channels and resources borrow the smaller shared pools only while doing payload work.
    pub(crate) const TRACKED_DESTINATIONS: usize = 8;
    pub(crate) const UPSTREAM_APP_DESTINATIONS: usize = 2;
    const REQUEST_HANDLERS: usize =
        <personal_hopspot_core::node_pages::NodePageRoutes as RequestEndpointSet<()>>::REGISTRATIONS
            .len();
    pub const LINK_SESSIONS: usize = 32;
    const TRANSPORTED_LINKS: usize = 4;
    const CHANNELS: usize = 1;
    const RESOURCE_ASSEMBLIES: usize = 1;
    const HELD_IDENTITIES: usize = REMOTE_CONTROL_REQUIRED_HELD_IDENTITY_CAPACITY;
    const BLACKHOLED_IDENTITIES: usize = 0;
    const BLACKHOLE_REASON_BYTES: usize = 0;
    const HELD_ANNOUNCES: usize = 4;
    const HELD_ANNOUNCE_APP_DATA_BYTES: usize = Self::HELD_ANNOUNCES * 64;
    const ANNOUNCE_HISTORY_DEPTH: usize = 8;
    pub(crate) const RETAINED_ANNOUNCE_APP_DATA_BYTES: usize = 256;
    pub(crate) const RETAINED_RATCHETS_PER_DESTINATION: usize = 4;
    const JOURNAL_WRITE_ALIGNMENT_BYTES: usize = 4;
    const MAX_JOURNAL_RECORD_PADDING_BYTES: usize = Self::JOURNAL_WRITE_ALIGNMENT_BYTES - 1;
    const COMPACTED_ROUTE_BASE_PAYLOAD_BYTES: usize =
        personal_rns::persistence::maximum_route_upsert_payload_len(0, 0);
    const COMPACTED_ROUTE_BASE_RECORD_BYTES: usize =
        personal_rns::persistence::flash_journal_record_storage_len(
            Self::COMPACTED_ROUTE_BASE_PAYLOAD_BYTES,
            Self::JOURNAL_WRITE_ALIGNMENT_BYTES,
        );
    const MAX_COMPACTED_ROUTE_RECORD_BYTES: usize =
        Self::COMPACTED_ROUTE_BASE_RECORD_BYTES + Self::MAX_JOURNAL_RECORD_PADDING_BYTES;
    const SELF_RATCHET_PAYLOAD_BYTES: usize = personal_rns::wire::TRUNCATED_HASH_BYTE_LEN
        + personal_rns::persistence::self_ratchets_snapshot_len(
            Self::RETAINED_RATCHETS_PER_DESTINATION,
        );
    const SELF_RATCHET_RECORD_BYTES: usize =
        personal_rns::persistence::flash_journal_record_storage_len(
            Self::SELF_RATCHET_PAYLOAD_BYTES,
            Self::JOURNAL_WRITE_ALIGNMENT_BYTES,
        );
    pub(crate) const MAX_CRITICAL_FLASH_JOURNAL_BYTES: usize =
        Self::UPSTREAM_APP_DESTINATIONS * Self::SELF_RATCHET_RECORD_BYTES;
    pub(crate) const MAX_COMPACTED_FLASH_JOURNAL_BYTES: usize = Self::TRACKED_DESTINATIONS
        * Self::MAX_COMPACTED_ROUTE_RECORD_BYTES
        + Self::RETAINED_ANNOUNCE_APP_DATA_BYTES
        + Self::MAX_CRITICAL_FLASH_JOURNAL_BYTES
        + personal_rns::persistence::flash_journal_record_storage_len(
            0,
            Self::JOURNAL_WRITE_ALIGNMENT_BYTES,
        );
    const RESOURCE_TRANSFER_BYTES: usize = 1504;
    pub const MAX_OUTGOING_RESOURCE_REACTION_FRAMES: usize =
        max_outgoing_resource_reaction_frames(Self::RESOURCE_TRANSFER_BYTES);
    const CHANNEL_REORDER_DEPTH: usize = 2;
    const LINK_MTU: usize = 1024;
    const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(Self::LINK_MTU);
}

const _: () = assert!(Nrf52840Storage::LINK_SESSIONS > Nrf52840Storage::CHANNELS);
const _: () = assert!(Nrf52840Storage::RESOURCE_ASSEMBLIES == 1);
const _: () = assert!(core::mem::size_of::<NoPendingResourceOfferTable>() == 0);
const _: () =
    assert!(core::mem::size_of::<PendingResourceOffers<NoPendingResourceOfferTable>>() == 0);

impl StorageLayout for Nrf52840Storage {
    const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
        tracked_destinations: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
        destination_identities: StorageCapacity::Fixed(0),
        announce_records: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
        upstream_app_destinations: StorageCapacity::Fixed(Self::UPSTREAM_APP_DESTINATIONS),
        held_identities: StorageCapacity::Fixed(Self::HELD_IDENTITIES),
        links: StorageCapacity::Fixed(Self::LINK_SESSIONS),
        channels: StorageCapacity::Fixed(Self::CHANNELS),
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
        ratchets_per_destination: StorageCapacity::Fixed(Self::RETAINED_RATCHETS_PER_DESTINATION),
    };

    type Routes = FixedArrayRouteTable<{ Self::TRACKED_DESTINATIONS }>;
    type RouteExpiries = personal_rns::routing::LinearRouteExpiryIndex;
    type DestinationIdentities = NoDestinationIdentityTable;
    type DestinationIdentityAppData = NoDestinationIdentityAppData;
    type Tunnels = FixedTunnelTable<0>;
    type Announces = FixedArrayAnnounceRecordTable<{ Self::TRACKED_DESTINATIONS }>;
    type History =
        FixedAnnounceIdHistory<{ Self::TRACKED_DESTINATIONS }, { Self::ANNOUNCE_HISTORY_DEPTH }>;
    type AppData = PackedAppDataArena<
        { Self::RETAINED_ANNOUNCE_APP_DATA_BYTES },
        { Self::TRACKED_DESTINATIONS },
    >;
    type ScheduledAnnounces = FixedScheduledAnnounceQueue<{ Self::TRACKED_DESTINATIONS }>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationTable<{ Self::UPSTREAM_APP_DESTINATIONS }>;
    type HeldIdentities = FixedHeldIdentityTable<{ Self::HELD_IDENTITIES }>;
    type SelfRatchets = FixedSelfRatchetTable<
        { Self::UPSTREAM_APP_DESTINATIONS },
        { Self::RETAINED_RATCHETS_PER_DESTINATION },
    >;
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
    type RequestHandlers = FixedRequestHandlerTable<{ Self::REQUEST_HANDLERS }>;
    type TransportedLinks = FixedTransportedLinkTable<{ Self::TRANSPORTED_LINKS }>;
    type Links = FixedLinkTable<{ Self::LINK_SESSIONS }>;
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
    type PendingResourceOffers = NoPendingResourceOfferTable;
    type IncomingAssemblies = FixedIncomingAssemblyTable<{ Self::RESOURCE_ASSEMBLIES }>;
    type OutgoingAssemblies = FixedStaticOutgoingAssemblyTable<{ Self::RESOURCE_ASSEMBLIES }>;
    type Channels = FixedArrayChannelTable<
        { Self::CHANNELS },
        { Self::CHANNEL_REORDER_DEPTH },
        { Self::CHANNEL_MESSAGE_BYTES },
    >;
}
