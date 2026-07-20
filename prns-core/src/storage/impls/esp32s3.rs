use core::marker::PhantomData;

use allocator_api2::alloc::{Allocator, Global};

use crate::crypto::ratchets::FixedSelfRatchetTable;
use crate::identity::destination_identity::{
    NoDestinationIdentityAppData, NoDestinationIdentityTable,
};
use crate::identity::held::FixedHeldIdentityTable;
use crate::interfaces::EMBEDDED_MAX_LINK_MTU;
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
    FixedIncomingAssemblyTable, FixedOutgoingAssemblyTable,
};
use crate::routing::links::resources::max_part_count;
use crate::routing::links::resources::table::{
    FixedHeapResourceTable, IncomingResourceState, OutgoingResourceState,
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
use crate::routing::routes::{route_index_buckets, FixedHeapRouteTable};
use crate::routing::tunnel::FixedTunnelTable;
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationTable;
use crate::routing::warmth::FixedDepartedInterfaceTable;
use crate::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

const MAX_TRACKED_DESTINATIONS: usize = 1024;
const MAX_UPSTREAM_APP_DESTINATIONS: usize = 1;
const MAX_HELD_IDENTITIES: usize = 1;
const MAX_CONCURRENT_LINKS: usize = 6;
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
/// The real PSRAM dial; a channel that finds the pool dry cannot grow its window until another drains a slot.
const CHANNEL_WINDOW_POOL: usize = 192;
const MAX_RESOURCE_TRANSFER_BYTES: usize = 8192;
const RETAINED_ANNOUNCE_APP_DATA_BYTES: usize = 40 * 1024;
const ROUTE_INDEX_BUCKETS: usize = route_index_buckets(MAX_TRACKED_DESTINATIONS);
const DESTINATION_ANNOUNCE_LIMIT_INDEX_BUCKETS: usize =
    destination_announce_limit_index_buckets(MAX_TRACKED_DESTINATIONS);
const MAX_RESOURCE_PARTS: usize = max_part_count(MAX_RESOURCE_TRANSFER_BYTES);
const CHANNEL_REORDER_DEPTH: usize = WINDOW_MAX_MESSAGES as usize;
const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(EMBEDDED_MAX_LINK_MTU);

pub struct Esp32S3<A: Allocator = Global>(PhantomData<A>);

impl<A: Allocator> Default for Esp32S3<A> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<A: Allocator + Default> StorageLayout for Esp32S3<A> {
    const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
        tracked_destinations: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
        destination_identities: StorageCapacity::Fixed(0),
        announce_records: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
        upstream_app_destinations: StorageCapacity::Fixed(MAX_UPSTREAM_APP_DESTINATIONS),
        held_identities: StorageCapacity::Fixed(MAX_HELD_IDENTITIES),
        links: StorageCapacity::Fixed(MAX_CONCURRENT_LINKS),
        channels: StorageCapacity::Fixed(MAX_CONCURRENT_CHANNELS),
        channel_window_pool: Some(CHANNEL_WINDOW_POOL),
        channel_reorder_depth: StorageCapacity::Fixed(CHANNEL_REORDER_DEPTH),
        link_mtu: StorageCapacity::Fixed(EMBEDDED_MAX_LINK_MTU),
        resource_transfer_bytes: StorageCapacity::Fixed(MAX_RESOURCE_TRANSFER_BYTES),
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
    type RequestHandlers = FixedRequestHandlerTable<MAX_UPSTREAM_APP_DESTINATIONS>;
    type TransportedLinks = FixedTransportedLinkTable<MAX_CONCURRENT_LINKS>;
    type Links = FixedLinkTable<MAX_CONCURRENT_LINKS>;
    type OutgoingResources = FixedHeapResourceTable<
        OutgoingResourceState,
        1,
        MAX_RESOURCE_TRANSFER_BYTES,
        MAX_RESOURCE_PARTS,
        A,
    >;
    type IncomingResources = FixedHeapResourceTable<
        IncomingResourceState,
        1,
        MAX_RESOURCE_TRANSFER_BYTES,
        MAX_RESOURCE_PARTS,
        A,
    >;
    type IncomingAssemblies = FixedIncomingAssemblyTable<MAX_CONCURRENT_LINKS>;
    type OutgoingAssemblies = FixedOutgoingAssemblyTable<MAX_CONCURRENT_LINKS>;
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
    use crate::storage::{StorageCapacity, StorageLayout};

    #[test]
    fn limits_report_the_storage_constants() {
        type L = Esp32S3;
        assert_eq!(
            <L as StorageLayout>::LIMITS.upstream_app_destinations,
            StorageCapacity::Fixed(super::MAX_UPSTREAM_APP_DESTINATIONS)
        );
        assert_eq!(
            <L as StorageLayout>::LIMITS.held_identities,
            StorageCapacity::Fixed(super::MAX_HELD_IDENTITIES)
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
            "  Channels      {:>6} B (metadata inline; payload pools boxed)",
            core::mem::size_of::<<L as StorageLayout>::Channels>()
        );
    }
}
