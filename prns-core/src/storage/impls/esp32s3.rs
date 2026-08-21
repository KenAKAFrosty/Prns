use core::marker::PhantomData;

use allocator_api2::alloc::{Allocator, Global};

use crate::crypto::ratchets::FixedSelfRatchetTable;
use crate::identity::destination_identity::{
    NoDestinationIdentityAppData, NoDestinationIdentityTable,
};
use crate::identity::held::FixedHeldIdentityTable;
use crate::interfaces::EMBEDDED_MAX_LINK_MTU;
#[cfg(feature = "flash")]
use crate::persistence::{
    flash_journal_record_storage_len, maximum_route_upsert_payload_len, self_ratchets_snapshot_len,
};
use crate::routing::announce::defaults::MAX_ANNOUNCE_IDS_PER_DESTINATION;
use crate::routing::announce::destination_announce_limit::{
    destination_announce_limit_index_buckets, FixedHeapDestinationAnnounceLimitTable,
};
use crate::routing::announce::held::FixedHeapHeldAnnounceTable;
use crate::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitTable;
use crate::routing::announce::schedule::FixedHeapScheduledAnnounceQueue;
use crate::routing::announce::stored::{
    FixedHeapAnnounceIdHistory, FixedHeapAnnounceRecordTable, FixedHeapPackedAppDataArena,
};
use crate::routing::blackhole::{blackhole_index_buckets, FixedHeapBlackholeTable};
use crate::routing::dedup::FixedPacketHashHistory;
use crate::routing::delivery::receipts::FixedReceiptTable;
use crate::routing::group_keys::FixedGroupKeyTable;
use crate::routing::links::channel::channel_mdu;
use crate::routing::links::channel::receive::WINDOW_MAX_MESSAGES;
use crate::routing::links::channel::table::impls::FixedHeapChannelTable;
use crate::routing::links::resources::assembly::{
    FixedIncomingAssemblyTable, FixedStaticOutgoingAssemblyTable,
};
use crate::routing::links::resources::pending::FixedHeapPendingResourceOfferTable;
use crate::routing::links::resources::table::{
    FixedHeapResourceTable, IncomingResourceState, OutgoingResourceState,
};
use crate::routing::links::resources::{
    max_outgoing_resource_reaction_frames, max_part_count, sealed_transfer_bytes,
};
use crate::routing::links::table::FixedHeapLinkTable;
use crate::routing::links::transported::FixedHeapTransportedLinkTable;
use crate::routing::path_requests::interface_path_request_limit::FixedInterfacePathRequestLimitTable;
use crate::routing::path_requests::pending::FixedPendingPathRequestTable;
use crate::routing::path_requests::recent::FixedRecentPathRequestTable;
use crate::routing::path_requests::recursive::FixedRecursivePathRequestTable;
use crate::routing::path_requests::seen::FixedSeenPathRequestTable;
use crate::routing::request_handlers::FixedRequestHandlerTable;
use crate::routing::reverse_routes::FixedReverseRouteTable;
use crate::routing::route_expiry::LinearRouteExpiryIndex;
use crate::routing::routes::{route_index_buckets, FixedHeapRouteTable};
use crate::routing::tunnel::FixedTunnelTable;
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationTable;
use crate::routing::warmth::FixedDepartedInterfaceTable;
use crate::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

const MAX_TRACKED_DESTINATIONS: usize = 512;
const MAX_UPSTREAM_APP_DESTINATIONS: usize = 2;
const MAX_HELD_IDENTITIES: usize = 1;
const MAX_LINK_SESSIONS: usize = 512;
const MAX_TRANSPORTED_LINKS: usize = 32;
const MAX_OUTSTANDING_RECEIPTS: usize = 8;
const MAX_PACKET_HASHES: usize = 48;
const MAX_BLACKHOLED_IDENTITIES: usize = 32;
const BLACKHOLE_REASON_BYTES: usize = 64;
const MAX_REVERSE_ROUTES: usize = 32;
const MAX_PENDING_PATH_REQUESTS: usize = 8;
const MAX_HELD_ANNOUNCES: usize = 512;
const RETAINED_RATCHETS_PER_DESTINATION: usize = 8;
/// Cheap: an open channel costs a metadata row; the bulk payloads live in [`CHANNEL_WINDOW_POOL`].
const MAX_CONCURRENT_CHANNELS: usize = 8;
const MAX_RESOURCE_ASSEMBLIES: usize = 1;
const MAX_PENDING_RESOURCE_OFFERS: usize = 4;
/// The real PSRAM dial; a channel that finds the pool dry cannot grow its window until another drains a slot.
const CHANNEL_WINDOW_POOL: usize = 192;
/// Incoming resources retain the existing conservative PSRAM bound.
const MAX_INCOMING_RESOURCE_TRANSFER_BYTES: usize = 8192;
/// Source-capable S3 nodes reuse one outgoing plaintext window for each archive segment.
#[cfg(feature = "large-static-responses")]
const MAX_OUTGOING_RESOURCE_PLAINTEXT_BYTES: usize = 256 * 1024;
#[cfg(not(feature = "large-static-responses"))]
const MAX_OUTGOING_RESOURCE_PLAINTEXT_BYTES: usize = 8192;
const MAX_OUTGOING_RESOURCE_TRANSFER_BYTES: usize =
    sealed_transfer_bytes(MAX_OUTGOING_RESOURCE_PLAINTEXT_BYTES);
const RETAINED_ANNOUNCE_APP_DATA_BYTES: usize = 40 * 1024;
const ROUTE_INDEX_BUCKETS: usize = route_index_buckets(MAX_TRACKED_DESTINATIONS);
const DESTINATION_ANNOUNCE_LIMIT_INDEX_BUCKETS: usize =
    destination_announce_limit_index_buckets(MAX_TRACKED_DESTINATIONS);
const MAX_OUTGOING_RESOURCE_PARTS: usize = max_part_count(MAX_OUTGOING_RESOURCE_TRANSFER_BYTES);
const MAX_INCOMING_RESOURCE_PARTS: usize = max_part_count(MAX_INCOMING_RESOURCE_TRANSFER_BYTES);
const CHANNEL_REORDER_DEPTH: usize = WINDOW_MAX_MESSAGES as usize;
const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(EMBEDDED_MAX_LINK_MTU);

/// The PSRAM-backed ESP32-S3 storage profile.
///
/// `MAX_REQUEST_HANDLERS` is independent from the upstream-application destination capacity:
/// applications commonly register several request paths on one destination. It defaults to the
/// historical two rows, while application recipes with more routes should supply their exact
/// registration count.
pub struct Esp32S3<
    A: Allocator = Global,
    const MAX_REQUEST_HANDLERS: usize = MAX_UPSTREAM_APP_DESTINATIONS,
>(PhantomData<A>);

impl<A: Allocator, const MAX_REQUEST_HANDLERS: usize> Esp32S3<A, MAX_REQUEST_HANDLERS> {
    pub const TRACKED_DESTINATIONS: usize = MAX_TRACKED_DESTINATIONS;
    pub const PENDING_RESOURCE_OFFERS: usize = MAX_PENDING_RESOURCE_OFFERS;
    pub const PENDING_RESOURCE_OFFER_ROW_BYTES: usize =
        FixedHeapPendingResourceOfferTable::<MAX_PENDING_RESOURCE_OFFERS, A>::RESERVED_ROW_BYTES;

    /// Cheap retained sessions outnumber every configured auto-interface fleet member, leaving
    /// admission room without multiplying resource or channel workspaces.
    pub const LINK_SESSIONS: usize = MAX_LINK_SESSIONS;

    /// The most frames one resource request can synchronously emit for this storage recipe.
    ///
    /// A request names at most `WINDOW_MAX` existing parts, and a response can append one hashmap
    /// update. Compact layouts cannot materialize a full 75-part resource, so deriving the bound
    /// from their outgoing store avoids reserving unreachable transport backlog on every lane.
    pub const MAX_OUTGOING_RESOURCE_REACTION_FRAMES: usize =
        max_outgoing_resource_reaction_frames(MAX_OUTGOING_RESOURCE_TRANSFER_BYTES);
}

#[cfg(feature = "flash")]
impl<A: Allocator, const MAX_REQUEST_HANDLERS: usize> Esp32S3<A, MAX_REQUEST_HANDLERS> {
    pub const MAX_CRITICAL_FLASH_JOURNAL_BYTES: usize = MAX_UPSTREAM_APP_DESTINATIONS
        * flash_journal_record_storage_len(
            crate::wire::TRUNCATED_HASH_BYTE_LEN
                + self_ratchets_snapshot_len(RETAINED_RATCHETS_PER_DESTINATION),
            4,
        );

    pub const MAX_COMPACTED_FLASH_JOURNAL_BYTES: usize = MAX_TRACKED_DESTINATIONS
        * (flash_journal_record_storage_len(maximum_route_upsert_payload_len(0, 0), 4) + 3)
        + RETAINED_ANNOUNCE_APP_DATA_BYTES
        + MAX_UPSTREAM_APP_DESTINATIONS
            * flash_journal_record_storage_len(
                crate::wire::TRUNCATED_HASH_BYTE_LEN
                    + self_ratchets_snapshot_len(RETAINED_RATCHETS_PER_DESTINATION),
                4,
            )
        + flash_journal_record_storage_len(0, 4);
}

impl<A: Allocator, const MAX_REQUEST_HANDLERS: usize> Default for Esp32S3<A, MAX_REQUEST_HANDLERS> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<A: Allocator + Default, const MAX_REQUEST_HANDLERS: usize> StorageLayout
    for Esp32S3<A, MAX_REQUEST_HANDLERS>
{
    const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
        tracked_destinations: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
        destination_identities: StorageCapacity::Fixed(0),
        announce_records: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
        upstream_app_destinations: StorageCapacity::Fixed(MAX_UPSTREAM_APP_DESTINATIONS),
        held_identities: StorageCapacity::Fixed(MAX_HELD_IDENTITIES),
        links: StorageCapacity::Fixed(MAX_LINK_SESSIONS),
        channels: StorageCapacity::Fixed(MAX_CONCURRENT_CHANNELS),
        channel_window_pool: Some(CHANNEL_WINDOW_POOL),
        channel_reorder_depth: StorageCapacity::Fixed(CHANNEL_REORDER_DEPTH),
        link_mtu: StorageCapacity::Fixed(EMBEDDED_MAX_LINK_MTU),
        resource_transfer_bytes: StorageCapacity::Fixed(MAX_INCOMING_RESOURCE_TRANSFER_BYTES),
        receipts: StorageCapacity::Fixed(MAX_OUTSTANDING_RECEIPTS),
        packet_hashes: StorageCapacity::Fixed(MAX_PACKET_HASHES),
        blackholed_identities: StorageCapacity::Fixed(MAX_BLACKHOLED_IDENTITIES),
        blackhole_reason_bytes: StorageCapacity::Fixed(BLACKHOLE_REASON_BYTES),
        reverse_routes: StorageCapacity::Fixed(MAX_REVERSE_ROUTES),
        pending_path_requests: StorageCapacity::Fixed(MAX_PENDING_PATH_REQUESTS),
        held_announces: StorageCapacity::Fixed(MAX_HELD_ANNOUNCES),
        ratchets_per_destination: StorageCapacity::Fixed(RETAINED_RATCHETS_PER_DESTINATION),
    };

    type Routes = FixedHeapRouteTable<MAX_TRACKED_DESTINATIONS, ROUTE_INDEX_BUCKETS, A>;
    type RouteExpiries = LinearRouteExpiryIndex;
    type DestinationIdentities = NoDestinationIdentityTable;
    type DestinationIdentityAppData = NoDestinationIdentityAppData;
    type Announces = FixedHeapAnnounceRecordTable<MAX_TRACKED_DESTINATIONS, A>;
    type History =
        FixedHeapAnnounceIdHistory<MAX_TRACKED_DESTINATIONS, MAX_ANNOUNCE_IDS_PER_DESTINATION, A>;
    type AppData =
        FixedHeapPackedAppDataArena<RETAINED_ANNOUNCE_APP_DATA_BYTES, MAX_TRACKED_DESTINATIONS, A>;
    type ScheduledAnnounces = FixedHeapScheduledAnnounceQueue<MAX_TRACKED_DESTINATIONS, A>;
    type UpstreamAppDestinations = FixedUpstreamAppDestinationTable<MAX_UPSTREAM_APP_DESTINATIONS>;
    type HeldIdentities = FixedHeldIdentityTable<MAX_HELD_IDENTITIES>;
    type SelfRatchets =
        FixedSelfRatchetTable<MAX_UPSTREAM_APP_DESTINATIONS, RETAINED_RATCHETS_PER_DESTINATION>;
    type Receipts = FixedReceiptTable<MAX_OUTSTANDING_RECEIPTS>;
    type PacketHashes = FixedPacketHashHistory<MAX_PACKET_HASHES>;
    type Blackholes = FixedHeapBlackholeTable<
        MAX_BLACKHOLED_IDENTITIES,
        { blackhole_index_buckets(MAX_BLACKHOLED_IDENTITIES) },
        BLACKHOLE_REASON_BYTES,
        A,
    >;
    type ReverseRoutes = FixedReverseRouteTable<MAX_REVERSE_ROUTES>;
    type DepartedInterfaces = FixedDepartedInterfaceTable<16>;
    type PendingPathRequests = FixedPendingPathRequestTable<MAX_PENDING_PATH_REQUESTS>;
    type RecentPathRequests = FixedRecentPathRequestTable<8>;
    type SeenPathRequests = FixedSeenPathRequestTable<8>;
    type Tunnels = FixedTunnelTable<0>;
    type RecursivePathRequests = FixedRecursivePathRequestTable<8>;
    type InterfacePathRequestLimits = FixedInterfacePathRequestLimitTable<8>;
    type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitTable<8>;
    type DirtyInterfaces = heapless::Vec<crate::interfaces::InterfaceId, 8>;
    type HeldAnnounces = FixedHeapHeldAnnounceTable<MAX_HELD_ANNOUNCES, A>;
    type HeldAnnounceAppData = FixedHeapPackedAppDataArena<65536, MAX_HELD_ANNOUNCES, A>;
    type DestinationAnnounceLimits = FixedHeapDestinationAnnounceLimitTable<
        MAX_TRACKED_DESTINATIONS,
        DESTINATION_ANNOUNCE_LIMIT_INDEX_BUCKETS,
        A,
    >;
    type GroupKeys = FixedGroupKeyTable<MAX_UPSTREAM_APP_DESTINATIONS>;
    type RequestHandlers = FixedRequestHandlerTable<MAX_REQUEST_HANDLERS>;
    type TransportedLinks = FixedHeapTransportedLinkTable<MAX_TRANSPORTED_LINKS, A>;
    type Links = FixedHeapLinkTable<MAX_LINK_SESSIONS, A>;
    type OutgoingResources = FixedHeapResourceTable<
        OutgoingResourceState,
        1,
        MAX_OUTGOING_RESOURCE_TRANSFER_BYTES,
        MAX_OUTGOING_RESOURCE_PARTS,
        A,
    >;
    type IncomingResources = FixedHeapResourceTable<
        IncomingResourceState,
        1,
        MAX_INCOMING_RESOURCE_TRANSFER_BYTES,
        MAX_INCOMING_RESOURCE_PARTS,
        A,
    >;
    type PendingResourceOffers = FixedHeapPendingResourceOfferTable<MAX_PENDING_RESOURCE_OFFERS, A>;
    type IncomingAssemblies = FixedIncomingAssemblyTable<MAX_RESOURCE_ASSEMBLIES>;
    type OutgoingAssemblies = FixedStaticOutgoingAssemblyTable<MAX_RESOURCE_ASSEMBLIES>;
    type Channels = FixedHeapChannelTable<
        MAX_CONCURRENT_CHANNELS,
        CHANNEL_REORDER_DEPTH,
        CHANNEL_MESSAGE_BYTES,
        CHANNEL_WINDOW_POOL,
        A,
    >;
}

#[cfg(test)]
mod tests {
    use super::Esp32S3;
    use crate::engine::EngineState;
    use crate::routing::links::resources::assembly::{
        IncomingAssemblyTable, OutgoingAssemblyTable,
    };
    use crate::routing::links::table::LinkTable;
    use crate::routing::links::transported::TransportedLinkTable;
    use crate::routing::request_handlers::RequestHandlerTable;
    use crate::routing::routes::RouteTable;
    use crate::storage::{StorageCapacity, StorageLayout};

    #[test]
    fn outgoing_resource_reaction_bound_matches_the_storage_recipe() {
        type Layout = Esp32S3<allocator_api2::alloc::Global>;
        #[cfg(not(feature = "large-static-responses"))]
        assert_eq!(Layout::MAX_OUTGOING_RESOURCE_REACTION_FRAMES, 19);
        #[cfg(feature = "large-static-responses")]
        assert_eq!(Layout::MAX_OUTGOING_RESOURCE_REACTION_FRAMES, 76);
    }

    #[test]
    fn limits_report_the_storage_constants() {
        type L = Esp32S3;
        assert_eq!(
            <L as StorageLayout>::LIMITS.tracked_destinations,
            StorageCapacity::Fixed(512)
        );
        assert_eq!(
            <L as StorageLayout>::LIMITS.upstream_app_destinations,
            StorageCapacity::Fixed(super::MAX_UPSTREAM_APP_DESTINATIONS)
        );
        assert_eq!(
            <L as StorageLayout>::LIMITS.held_identities,
            StorageCapacity::Fixed(super::MAX_HELD_IDENTITIES)
        );
        assert_eq!(
            <L as StorageLayout>::LIMITS.links,
            StorageCapacity::Fixed(512)
        );
        assert_eq!(
            <L as StorageLayout>::LIMITS.channels,
            StorageCapacity::Fixed(8)
        );
        assert_eq!(<L as StorageLayout>::LIMITS.channel_window_pool, Some(192));
        assert_eq!(
            <<L as StorageLayout>::Routes as Default>::default().capacity(),
            512
        );
        assert_eq!(
            <<L as StorageLayout>::Links as Default>::default().capacity(),
            512
        );
        assert_eq!(
            <<L as StorageLayout>::TransportedLinks as Default>::default().capacity(),
            32
        );
        assert_eq!(
            <<L as StorageLayout>::IncomingAssemblies as Default>::default().capacity(),
            1
        );
        assert_eq!(
            <<L as StorageLayout>::OutgoingAssemblies as Default>::default().capacity(),
            1
        );
        let engine = EngineState::<L>::default();
        assert_eq!(
            engine.pending_resource_offers.capacity(),
            L::PENDING_RESOURCE_OFFERS,
        );
        assert_eq!(
            engine.incoming_resources.transfer_capacity(),
            super::MAX_INCOMING_RESOURCE_TRANSFER_BYTES,
            "source serving must not raise the incoming resource limit"
        );
        assert_eq!(
            engine.outgoing_resources.transfer_capacity(),
            super::MAX_OUTGOING_RESOURCE_TRANSFER_BYTES
        );
        assert!(engine.outgoing_assemblies.supports_static_continuations());
        #[cfg(feature = "large-static-responses")]
        assert_eq!(super::MAX_OUTGOING_RESOURCE_PLAINTEXT_BYTES, 256 * 1024);
        #[cfg(not(feature = "large-static-responses"))]
        assert_eq!(
            super::MAX_OUTGOING_RESOURCE_PLAINTEXT_BYTES,
            super::MAX_INCOMING_RESOURCE_TRANSFER_BYTES
        );
    }

    #[test]
    fn request_handler_capacity_is_an_application_owned_dimension() {
        type L = Esp32S3<allocator_api2::alloc::Global, 5>;
        let handlers = <L as StorageLayout>::RequestHandlers::default();
        assert_eq!(handlers.capacity(), 5);
        assert_eq!(
            <L as StorageLayout>::LIMITS.upstream_app_destinations,
            StorageCapacity::Fixed(super::MAX_UPSTREAM_APP_DESTINATIONS)
        );
    }

    #[test]
    fn print_footprint() {
        type L = Esp32S3;
        println!(
            "EngineState<Esp32S3<Global>> = {} bytes SRAM-resident (host 64-bit; the cold bulk lives behind boxes in PSRAM and is not counted here)",
            core::mem::size_of::<EngineState<L>>()
        );
        println!(
            "  Routes        {:>6} B (index inline; rows boxed)",
            core::mem::size_of::<<L as StorageLayout>::Routes>()
        );
        println!(
            "  Announces     {:>6} B (boxed)",
            core::mem::size_of::<<L as StorageLayout>::Announces>()
        );
        println!(
            "  History       {:>6} B (boxed)",
            core::mem::size_of::<<L as StorageLayout>::History>()
        );
        println!(
            "  AppData       {:>6} B (boxed)",
            core::mem::size_of::<<L as StorageLayout>::AppData>()
        );
        println!(
            "  ScheduledAnn  {:>6} B (boxed)",
            core::mem::size_of::<<L as StorageLayout>::ScheduledAnnounces>()
        );
        println!(
            "  DestinationAnnounceLimits {:>6} B (boxed)",
            core::mem::size_of::<<L as StorageLayout>::DestinationAnnounceLimits>()
        );
        println!(
            "  ReverseRoutes {:>6} B (inline)",
            core::mem::size_of::<<L as StorageLayout>::ReverseRoutes>()
        );
        println!(
            "  PacketHashes  {:>6} B (inline)",
            core::mem::size_of::<<L as StorageLayout>::PacketHashes>()
        );
        println!(
            "  Blackholes    {:>6} B (index inline; rows allocated)",
            core::mem::size_of::<<L as StorageLayout>::Blackholes>()
        );
        println!(
            "  UpstreamApps  {:>6} B (inline)",
            core::mem::size_of::<<L as StorageLayout>::UpstreamAppDestinations>()
        );
        println!(
            "  HeldIds       {:>6} B (inline)",
            core::mem::size_of::<<L as StorageLayout>::HeldIdentities>()
        );
        println!(
            "  Receipts      {:>6} B (inline)",
            core::mem::size_of::<<L as StorageLayout>::Receipts>()
        );
        println!(
            "  Links         {:>6} B (inline)",
            core::mem::size_of::<<L as StorageLayout>::Links>()
        );
        println!(
            "  OutResources  {:>6} B (boxed)",
            core::mem::size_of::<<L as StorageLayout>::OutgoingResources>()
        );
        println!(
            "  InResources   {:>6} B (boxed)",
            core::mem::size_of::<<L as StorageLayout>::IncomingResources>()
        );
        println!(
            "  PendingOffers {:>6} B control; {:>6} B allocator row request",
            core::mem::size_of::<<L as StorageLayout>::PendingResourceOffers>(),
            L::PENDING_RESOURCE_OFFER_ROW_BYTES,
        );
        println!(
            "  Channels      {:>6} B (metadata inline; payload pools boxed)",
            core::mem::size_of::<<L as StorageLayout>::Channels>()
        );
    }
}
