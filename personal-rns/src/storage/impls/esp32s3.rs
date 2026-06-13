use crate::crypto::ratchets::FixedSelfRatchetColumns;
use crate::identity::held::FixedHeldIdentityColumns;
use crate::routing::announce::rate_limit::FixedAnnounceRateColumns;
use crate::routing::announce::retained::{
    FixedArrayRetainedAnnounceColumns, PackedAppDataArena, TieredAnnounceIdHistory,
};
use crate::routing::announce::schedule::FixedScheduledAnnounceQueue;
use crate::routing::dedup::FixedPacketHashHistory;
use crate::routing::delivery::receipts::FixedReceiptColumns;
use crate::routing::group_keys::FixedGroupKeyColumns;
use crate::routing::links::resources::max_part_count;
use crate::routing::links::resources::table::{
    FixedResourceColumns, IncomingResourceState, OutgoingResourceState,
};
use crate::routing::links::table::FixedLinkColumns;
use crate::routing::links::transported::FixedTransportedLinkColumns;
use crate::routing::path_requests::pending::FixedPendingPathRequestColumns;
use crate::routing::path_requests::seen::FixedSeenPathRequestColumns;
use crate::routing::request_handlers::FixedRequestHandlerColumns;
use crate::routing::reverse_routes::FixedReverseRouteColumns;
use crate::routing::routes::FixedArrayRouteColumns;
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;
use crate::storage::StorageLayout;

pub struct Esp32S3;

impl Esp32S3 {
    const MAX_TRACKED_DESTINATIONS: usize = 24;
    const MAX_UPSTREAM_APP_DESTINATIONS: usize = 4;
    const MAX_CONCURRENT_LINKS: usize = 4;
    const MAX_RESOURCE_TRANSFER_BYTES: usize = 4096;
}

impl StorageLayout for Esp32S3 {
    type Routes = FixedArrayRouteColumns<{ Self::MAX_TRACKED_DESTINATIONS }>;
    type Announces = FixedArrayRetainedAnnounceColumns<{ Self::MAX_TRACKED_DESTINATIONS }>;
    type History = TieredAnnounceIdHistory<4, 128, { Self::MAX_TRACKED_DESTINATIONS }, 32>;
    type AppData = PackedAppDataArena<1024, { Self::MAX_TRACKED_DESTINATIONS }>;
    type ScheduledAnnounces = FixedScheduledAnnounceQueue<{ Self::MAX_TRACKED_DESTINATIONS }>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationColumns<{ Self::MAX_UPSTREAM_APP_DESTINATIONS }>;
    type HeldIdentities = FixedHeldIdentityColumns<4>;
    type SelfRatchets = FixedSelfRatchetColumns<{ Self::MAX_UPSTREAM_APP_DESTINATIONS }, 8>;
    type Receipts = FixedReceiptColumns<8>;
    type PacketHashes = FixedPacketHashHistory<32>;
    type ReverseRoutes = FixedReverseRouteColumns<8>;
    type PendingPathRequests = FixedPendingPathRequestColumns<8>;
    type SeenPathRequests = FixedSeenPathRequestColumns<8>;
    type AnnounceRates = FixedAnnounceRateColumns<{ Self::MAX_TRACKED_DESTINATIONS }>;
    type GroupKeys = FixedGroupKeyColumns<{ Self::MAX_UPSTREAM_APP_DESTINATIONS }>;
    type RequestHandlers = FixedRequestHandlerColumns<{ Self::MAX_UPSTREAM_APP_DESTINATIONS }>;
    type TransportedLinks = FixedTransportedLinkColumns<{ Self::MAX_CONCURRENT_LINKS }>;
    type Links = FixedLinkColumns<{ Self::MAX_CONCURRENT_LINKS }>;
    type OutgoingResources = FixedResourceColumns<
        OutgoingResourceState,
        1,
        { Self::MAX_RESOURCE_TRANSFER_BYTES },
        { max_part_count(Self::MAX_RESOURCE_TRANSFER_BYTES) },
    >;
    type IncomingResources = FixedResourceColumns<
        IncomingResourceState,
        1,
        { Self::MAX_RESOURCE_TRANSFER_BYTES },
        { max_part_count(Self::MAX_RESOURCE_TRANSFER_BYTES) },
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
            "EngineState<Esp32S3> = {} bytes (host 64-bit; the 32-bit S3 measured 31160)",
            core::mem::size_of::<EngineState<L>>()
        );
        println!(
            "  Routes        {:>6} B",
            core::mem::size_of::<<L as StorageLayout>::Routes>()
        );
        println!(
            "  Announces     {:>6} B",
            core::mem::size_of::<<L as StorageLayout>::Announces>()
        );
        println!(
            "  History       {:>6} B",
            core::mem::size_of::<<L as StorageLayout>::History>()
        );
        println!(
            "  AppData       {:>6} B",
            core::mem::size_of::<<L as StorageLayout>::AppData>()
        );
        println!(
            "  PacketHashes  {:>6} B",
            core::mem::size_of::<<L as StorageLayout>::PacketHashes>()
        );
        println!(
            "  Receipts      {:>6} B",
            core::mem::size_of::<<L as StorageLayout>::Receipts>()
        );
        println!(
            "  Links         {:>6} B",
            core::mem::size_of::<<L as StorageLayout>::Links>()
        );
    }
}
