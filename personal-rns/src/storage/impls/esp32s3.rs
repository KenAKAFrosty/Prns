use core::marker::PhantomData;

use allocator_api2::alloc::{Allocator, Global};

use crate::crypto::ratchets::FixedSelfRatchetColumns;
use crate::identity::held::FixedHeldIdentityColumns;
use crate::routing::announce::rate_limit::FixedHeapAnnounceRateColumns;
use crate::routing::announce::retained::{
    FixedHeapPackedAppDataArena, FixedHeapRetainedAnnounceColumns, FixedHeapTieredAnnounceIdHistory,
};
use crate::routing::announce::schedule::FixedHeapScheduledAnnounceQueue;
use crate::routing::dedup::FixedPacketHashHistory;
use crate::routing::delivery::receipts::FixedReceiptColumns;
use crate::routing::group_keys::FixedGroupKeyColumns;
use crate::routing::links::resources::max_part_count;
use crate::routing::links::resources::table::{
    FixedHeapResourceColumns, IncomingResourceState, OutgoingResourceState,
};
use crate::routing::links::table::FixedLinkColumns;
use crate::routing::links::transported::FixedTransportedLinkColumns;
use crate::routing::path_requests::pending::FixedPendingPathRequestColumns;
use crate::routing::path_requests::seen::FixedSeenPathRequestColumns;
use crate::routing::request_handlers::FixedRequestHandlerColumns;
use crate::routing::reverse_routes::FixedReverseRouteColumns;
use crate::routing::routes::{route_index_buckets, FixedHeapRouteColumns};
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;
use crate::storage::StorageLayout;

const MAX_TRACKED_DESTINATIONS: usize = 2000;
const MAX_UPSTREAM_APP_DESTINATIONS: usize = 8;
const MAX_CONCURRENT_LINKS: usize = 8;
const MAX_RESOURCE_TRANSFER_BYTES: usize = 8192;
const ROUTE_INDEX_BUCKETS: usize = route_index_buckets(MAX_TRACKED_DESTINATIONS);
const MAX_RESOURCE_PARTS: usize = max_part_count(MAX_RESOURCE_TRANSFER_BYTES);

pub struct Esp32S3<A: Allocator = Global>(PhantomData<A>);

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
    type Receipts = FixedReceiptColumns<32>;
    type PacketHashes = FixedPacketHashHistory<64>;
    type ReverseRoutes = FixedReverseRouteColumns<128>;
    type PendingPathRequests = FixedPendingPathRequestColumns<8>;
    type SeenPathRequests = FixedSeenPathRequestColumns<8>;
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
        println!("  Routes        {:>6} B (index inline; rows boxed)", core::mem::size_of::<<L as StorageLayout>::Routes>());
        println!("  Announces     {:>6} B (boxed)", core::mem::size_of::<<L as StorageLayout>::Announces>());
        println!("  History       {:>6} B (boxed)", core::mem::size_of::<<L as StorageLayout>::History>());
        println!("  AppData       {:>6} B (boxed)", core::mem::size_of::<<L as StorageLayout>::AppData>());
        println!("  ScheduledAnn  {:>6} B (boxed)", core::mem::size_of::<<L as StorageLayout>::ScheduledAnnounces>());
        println!("  AnnounceRates {:>6} B (boxed)", core::mem::size_of::<<L as StorageLayout>::AnnounceRates>());
        println!("  ReverseRoutes {:>6} B (inline)", core::mem::size_of::<<L as StorageLayout>::ReverseRoutes>());
        println!("  PacketHashes  {:>6} B (inline)", core::mem::size_of::<<L as StorageLayout>::PacketHashes>());
        println!("  Receipts      {:>6} B (inline)", core::mem::size_of::<<L as StorageLayout>::Receipts>());
        println!("  Links         {:>6} B (inline)", core::mem::size_of::<<L as StorageLayout>::Links>());
        println!("  OutResources  {:>6} B (boxed)", core::mem::size_of::<<L as StorageLayout>::OutgoingResources>());
        println!("  InResources   {:>6} B (boxed)", core::mem::size_of::<<L as StorageLayout>::IncomingResources>());
    }
}
