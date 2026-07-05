use core::marker::PhantomData;

use allocator_api2::alloc::{Allocator, Global};

use crate::crypto::ratchets::FixedSelfRatchetColumns;
use crate::identity::held::FixedHeldIdentityColumns;
use crate::routing::announce::held::FixedHeapHeldAnnounceColumns;
use crate::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitColumns;
use crate::routing::announce::rate_limit::{
    announce_rate_index_buckets, FixedHeapAnnounceRateColumns,
};
use crate::routing::announce::retained::{
    FixedHeapPackedAppDataArena, FixedHeapRetainedAnnounceColumns, FixedHeapTieredAnnounceIdHistory,
};
use crate::routing::announce::schedule::FixedHeapScheduledAnnounceQueue;
use crate::routing::dedup::FixedPacketHashHistory;
use crate::routing::delivery::receipts::FixedReceiptColumns;
use crate::routing::group_keys::FixedGroupKeyColumns;
use crate::routing::links::channel::channel_mdu;
use crate::routing::links::channel::impls::FixedHeapChannelColumns;
use crate::routing::links::channel::receive::WINDOW_MAX;
use crate::routing::links::resources::assembly::{
    FixedIncomingAssemblyColumns, FixedOutgoingAssemblyColumns,
};
use crate::routing::links::resources::max_part_count;
use crate::routing::links::resources::table::{
    FixedHeapResourceColumns, IncomingResourceState, OutgoingResourceState,
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
use crate::routing::routes::{route_index_buckets, FixedHeapRouteColumns};
use crate::routing::tunnel::FixedTunnelColumns;
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;
use crate::routing::warmth::FixedDepartedInterfaceColumns;
use crate::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

const MAX_TRACKED_DESTINATIONS: usize = 1024;
const MAX_UPSTREAM_APP_DESTINATIONS: usize = 1;
const MAX_HELD_IDENTITIES: usize = 1;
const MAX_CONCURRENT_LINKS: usize = 6;
const MAX_OUTSTANDING_RECEIPTS: usize = 8;
const MAX_PACKET_HASHES: usize = 48;
const MAX_REVERSE_ROUTES: usize = 32;
const MAX_PENDING_PATH_REQUESTS: usize = 8;
const MAX_HELD_ANNOUNCES: usize = 64;
const RETAINED_RATCHETS_PER_DESTINATION: usize = 8;
/// Cheap: an open channel costs a metadata row; the bulk payloads live in
/// [`CHANNEL_WINDOW_POOL`].
const MAX_CONCURRENT_CHANNELS: usize = 8;
/// The real PSRAM dial; a channel that finds the pool dry cannot grow its window
/// until another drains a slot.
const CHANNEL_WINDOW_POOL: usize = 192;
const MAX_RESOURCE_TRANSFER_BYTES: usize = 8192;
const RETAINED_ANNOUNCE_APP_DATA_BYTES: usize = 40 * 1024;
const ROUTE_INDEX_BUCKETS: usize = route_index_buckets(MAX_TRACKED_DESTINATIONS);
const ANNOUNCE_RATE_INDEX_BUCKETS: usize = announce_rate_index_buckets(MAX_TRACKED_DESTINATIONS);
const MAX_RESOURCE_PARTS: usize = max_part_count(MAX_RESOURCE_TRANSFER_BYTES);
const CHANNEL_REORDER_DEPTH: usize = WINDOW_MAX as usize;
/// Matches `reactor::interface_seam::EMBEDDED_MAX_LINK_MTU`; duplicated here because the reactor
/// seam is feature-gated, while the storage package also builds under `external-alloc` alone.
const LINK_MTU: usize = 1_472;
const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(LINK_MTU);

pub struct Esp32S3<A: Allocator = Global>(PhantomData<A>);

impl<A: Allocator> Default for Esp32S3<A> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<A: Allocator + Default> StorageLayout for Esp32S3<A> {
    const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
        tracked_destinations: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
        retained_announces: StorageCapacity::Fixed(MAX_TRACKED_DESTINATIONS),
        upstream_app_destinations: StorageCapacity::Fixed(MAX_UPSTREAM_APP_DESTINATIONS),
        held_identities: StorageCapacity::Fixed(MAX_HELD_IDENTITIES),
        links: StorageCapacity::Fixed(MAX_CONCURRENT_LINKS),
        channels: StorageCapacity::Fixed(MAX_CONCURRENT_CHANNELS),
        channel_window_pool: Some(CHANNEL_WINDOW_POOL),
        channel_reorder_depth: StorageCapacity::Fixed(CHANNEL_REORDER_DEPTH),
        link_mtu: StorageCapacity::Fixed(LINK_MTU),
        resource_transfer_bytes: StorageCapacity::Fixed(MAX_RESOURCE_TRANSFER_BYTES),
        receipts: StorageCapacity::Fixed(MAX_OUTSTANDING_RECEIPTS),
        packet_hashes: StorageCapacity::Fixed(MAX_PACKET_HASHES),
        reverse_routes: StorageCapacity::Fixed(MAX_REVERSE_ROUTES),
        pending_path_requests: StorageCapacity::Fixed(MAX_PENDING_PATH_REQUESTS),
        held_announces: StorageCapacity::Fixed(MAX_HELD_ANNOUNCES),
        ratchets_per_destination: StorageCapacity::Fixed(RETAINED_RATCHETS_PER_DESTINATION),
    };

    type Routes = FixedHeapRouteColumns<MAX_TRACKED_DESTINATIONS, ROUTE_INDEX_BUCKETS, A>;
    type Announces = FixedHeapRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS, A>;
    type History = FixedHeapTieredAnnounceIdHistory<4, 256, MAX_TRACKED_DESTINATIONS, 32, A>;
    type AppData =
        FixedHeapPackedAppDataArena<RETAINED_ANNOUNCE_APP_DATA_BYTES, MAX_TRACKED_DESTINATIONS, A>;
    type ScheduledAnnounces = FixedHeapScheduledAnnounceQueue<MAX_TRACKED_DESTINATIONS, A>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type HeldIdentities = FixedHeldIdentityColumns<MAX_HELD_IDENTITIES>;
    type SelfRatchets =
        FixedSelfRatchetColumns<MAX_UPSTREAM_APP_DESTINATIONS, RETAINED_RATCHETS_PER_DESTINATION>;
    type Receipts = FixedReceiptColumns<MAX_OUTSTANDING_RECEIPTS>;
    type PacketHashes = FixedPacketHashHistory<MAX_PACKET_HASHES>;
    type ReverseRoutes = FixedReverseRouteColumns<MAX_REVERSE_ROUTES>;
    type DepartedInterfaces = FixedDepartedInterfaceColumns<16>;
    type PendingPathRequests = FixedPendingPathRequestColumns<MAX_PENDING_PATH_REQUESTS>;
    type RecentPathRequests = FixedRecentPathRequestColumns<8>;
    type SeenPathRequests = FixedSeenPathRequestColumns<8>;
    type Tunnels = FixedTunnelColumns<0>;
    type DiscoveryPathRequests = FixedDiscoveryPathRequestColumns<8>;
    type InterfacePathRequestLimits = FixedInterfacePathRequestLimitColumns<8>;
    type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitColumns<8>;
    type DirtyInterfaces = heapless::Vec<crate::interfaces::InterfaceId, 8>;
    type HeldAnnounces = FixedHeapHeldAnnounceColumns<MAX_HELD_ANNOUNCES, A>;
    type HeldAnnounceAppData = FixedHeapPackedAppDataArena<8192, MAX_HELD_ANNOUNCES, A>;
    type AnnounceRates =
        FixedHeapAnnounceRateColumns<MAX_TRACKED_DESTINATIONS, ANNOUNCE_RATE_INDEX_BUCKETS, A>;
    type GroupKeys = FixedGroupKeyColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type RequestHandlers = FixedRequestHandlerColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type TransportedLinks = FixedTransportedLinkColumns<MAX_CONCURRENT_LINKS>;
    type Links = FixedLinkColumns<MAX_CONCURRENT_LINKS>;
    type OutgoingResources = FixedHeapResourceColumns<
        OutgoingResourceState,
        1,
        MAX_RESOURCE_TRANSFER_BYTES,
        MAX_RESOURCE_PARTS,
        A,
    >;
    type IncomingResources = FixedHeapResourceColumns<
        IncomingResourceState,
        1,
        MAX_RESOURCE_TRANSFER_BYTES,
        MAX_RESOURCE_PARTS,
        A,
    >;
    type IncomingAssemblies = FixedIncomingAssemblyColumns<MAX_CONCURRENT_LINKS>;
    type OutgoingAssemblies = FixedOutgoingAssemblyColumns<MAX_CONCURRENT_LINKS>;
    type Channels = FixedHeapChannelColumns<
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
            "  AnnounceRates {:>6} B (boxed)",
            core::mem::size_of::<<L as StorageLayout>::AnnounceRates>()
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
