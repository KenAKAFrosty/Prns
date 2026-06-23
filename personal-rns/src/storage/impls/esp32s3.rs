use core::marker::PhantomData;

use allocator_api2::alloc::{Allocator, Global};

use crate::crypto::ratchets::FixedSelfRatchetColumns;
use crate::identity::held::FixedHeldIdentityColumns;
use crate::routing::announce::held::FixedHeapHeldAnnounceColumns;
use crate::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitColumns;
use crate::routing::announce::rate_limit::FixedHeapAnnounceRateColumns;
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
use crate::storage::StorageLayout;

const MAX_TRACKED_DESTINATIONS: usize = 512;
const MAX_UPSTREAM_APP_DESTINATIONS: usize = 8;
const MAX_CONCURRENT_LINKS: usize = 8;
/// How many channels may be open at once. Independent of `MAX_CONCURRENT_LINKS` (most links never
/// open a channel) and now cheap: an open channel costs only its tight per-channel metadata row, not
/// a window. The bulk payloads live in a shared pool ([`CHANNEL_WINDOW_POOL`]), so this can be
/// generous without reserving a window per channel.
const MAX_CONCURRENT_CHANNELS: usize = 16;
/// The payload slots all open channels draw from freeform, one shared pool for the receive reorder
/// buffer and one for the send retransmit buffer (each `CHANNEL_WINDOW_POOL` slots of
/// `CHANNEL_MESSAGE_BYTES`). This is the real memory dial: ~94 KiB per 48 slots. Sized so the total
/// payload bytes match the old four-private-windows budget while letting any mix of channels use
/// them — 16 channels a few deep, or a few channels near the full window. A channel that finds the
/// pool dry simply cannot grow its window until another drains a slot.
const CHANNEL_WINDOW_POOL: usize = 192;
const MAX_RESOURCE_TRANSFER_BYTES: usize = 8192;
const ROUTE_INDEX_BUCKETS: usize = route_index_buckets(MAX_TRACKED_DESTINATIONS);
const MAX_RESOURCE_PARTS: usize = max_part_count(MAX_RESOURCE_TRANSFER_BYTES);
const CHANNEL_REORDER_DEPTH: usize = WINDOW_MAX as usize;
const LINK_MTU: usize = 2048;
const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(LINK_MTU);

pub struct Esp32S3<A: Allocator = Global>(PhantomData<A>);

impl<A: Allocator> Default for Esp32S3<A> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<A: Allocator + Default> StorageLayout for Esp32S3<A> {
    type Routes = FixedHeapRouteColumns<MAX_TRACKED_DESTINATIONS, ROUTE_INDEX_BUCKETS, A>;
    type Announces = FixedHeapRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS, A>;
    type History = FixedHeapTieredAnnounceIdHistory<4, 256, MAX_TRACKED_DESTINATIONS, 32, A>;
    type AppData = FixedHeapPackedAppDataArena<4096, MAX_TRACKED_DESTINATIONS, A>;
    type ScheduledAnnounces = FixedHeapScheduledAnnounceQueue<MAX_TRACKED_DESTINATIONS, A>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type HeldIdentities = FixedHeldIdentityColumns<4>;
    type SelfRatchets = FixedSelfRatchetColumns<MAX_UPSTREAM_APP_DESTINATIONS, 8>;
    type Receipts = FixedReceiptColumns<16>;
    type PacketHashes = FixedPacketHashHistory<32>;
    type ReverseRoutes = FixedReverseRouteColumns<48>;
    type PendingPathRequests = FixedPendingPathRequestColumns<8>;
    type RecentPathRequests = FixedRecentPathRequestColumns<8>;
    type SeenPathRequests = FixedSeenPathRequestColumns<8>;
    type Tunnels = FixedTunnelColumns<0>;
    type DiscoveryPathRequests = FixedDiscoveryPathRequestColumns<8>;
    type InterfacePathRequestLimits = FixedInterfacePathRequestLimitColumns<8>;
    type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitColumns<8>;
    type DirtyInterfaces = heapless::Vec<crate::interfaces::InterfaceId, 8>;
    type HeldAnnounces = FixedHeapHeldAnnounceColumns<64, A>;
    type HeldAnnounceAppData = FixedHeapPackedAppDataArena<8192, 64, A>;
    type AnnounceRates = FixedHeapAnnounceRateColumns<MAX_TRACKED_DESTINATIONS, A>;
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
    use crate::storage::StorageLayout;

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
    }
}
