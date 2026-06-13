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
use crate::storage::{StorageLayout, StorageCapacities};

pub struct Esp32S3;

impl StorageCapacities for Esp32S3 {
    const MAX_TRACKED_DESTINATIONS: usize = 24;
    const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize = 32;
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize = 1024;
    const HISTORY_FLOOR_PER_DESTINATION: usize = 4;
    const HISTORY_OVERFLOW_CAPACITY: usize = 128;
    const MAX_UPSTREAM_APP_DESTINATIONS: usize = 4;
    const MAX_HELD_IDENTITIES: usize = 4;
    const PACKET_HASH_GENERATION_CAPACITY: usize = 32;
    const RETAINED_RATCHETS_PER_DESTINATION: usize = 8;
    const MAX_OUTSTANDING_RECEIPTS: usize = 8;
    const MAX_REVERSE_ROUTES: usize = 8;
    const MAX_PENDING_PATH_REQUESTS: usize = 8;
    const MAX_SEEN_PATH_REQUESTS: usize = 8;
    const MAX_LINKS: usize = 4;
}

impl StorageLayout for Esp32S3 {
    type Routes = FixedArrayRouteColumns<{ <Self as StorageCapacities>::MAX_TRACKED_DESTINATIONS }>;
    type Announces =
        FixedArrayRetainedAnnounceColumns<{ <Self as StorageCapacities>::MAX_TRACKED_DESTINATIONS }>;
    type History = TieredAnnounceIdHistory<
        { <Self as StorageCapacities>::HISTORY_FLOOR_PER_DESTINATION },
        { <Self as StorageCapacities>::HISTORY_OVERFLOW_CAPACITY },
        { <Self as StorageCapacities>::MAX_TRACKED_DESTINATIONS },
        { <Self as StorageCapacities>::MAX_ANNOUNCE_IDS_PER_DESTINATION },
    >;
    type AppData = PackedAppDataArena<
        { <Self as StorageCapacities>::ANNOUNCE_APP_DATA_ARENA_BYTES },
        { <Self as StorageCapacities>::MAX_TRACKED_DESTINATIONS },
    >;
    type ScheduledAnnounces =
        FixedScheduledAnnounceQueue<{ <Self as StorageCapacities>::MAX_TRACKED_DESTINATIONS }>;
    type UpstreamAppDestinations = FixedUpstreamAppDestinationColumns<
        { <Self as StorageCapacities>::MAX_UPSTREAM_APP_DESTINATIONS },
    >;
    type HeldIdentities =
        FixedHeldIdentityColumns<{ <Self as StorageCapacities>::MAX_HELD_IDENTITIES }>;
    type SelfRatchets = FixedSelfRatchetColumns<
        { <Self as StorageCapacities>::MAX_UPSTREAM_APP_DESTINATIONS },
        { <Self as StorageCapacities>::RETAINED_RATCHETS_PER_DESTINATION },
    >;
    type PacketHashes =
        FixedPacketHashHistory<{ <Self as StorageCapacities>::PACKET_HASH_GENERATION_CAPACITY }>;
    type Receipts = FixedReceiptColumns<{ <Self as StorageCapacities>::MAX_OUTSTANDING_RECEIPTS }>;
    type ReverseRoutes =
        FixedReverseRouteColumns<{ <Self as StorageCapacities>::MAX_REVERSE_ROUTES }>;
    type PendingPathRequests =
        FixedPendingPathRequestColumns<{ <Self as StorageCapacities>::MAX_PENDING_PATH_REQUESTS }>;
    type SeenPathRequests =
        FixedSeenPathRequestColumns<{ <Self as StorageCapacities>::MAX_SEEN_PATH_REQUESTS }>;
    type AnnounceRates =
        FixedAnnounceRateColumns<{ <Self as StorageCapacities>::MAX_TRACKED_DESTINATIONS }>;
    type GroupKeys =
        FixedGroupKeyColumns<{ <Self as StorageCapacities>::MAX_UPSTREAM_APP_DESTINATIONS }>;
    type RequestHandlers =
        FixedRequestHandlerColumns<{ <Self as StorageCapacities>::MAX_UPSTREAM_APP_DESTINATIONS }>;
    type TransportedLinks =
        FixedTransportedLinkColumns<{ <Self as StorageCapacities>::MAX_LINKS }>;
    type Links = FixedLinkColumns<{ <Self as StorageCapacities>::MAX_LINKS }>;
    type OutgoingResources = FixedResourceColumns<OutgoingResourceState, 1, 4096, 9>;
    type IncomingResources = FixedResourceColumns<IncomingResourceState, 1, 4096, 9>;
}

#[cfg(test)]
mod tests {
    use super::Esp32S3;
    use crate::storage::{StorageLayout, FixedInline};

    fn assert_same_storage<A, B>()
    where
        A: StorageLayout,
        B: StorageLayout<
            Routes = A::Routes,
            Announces = A::Announces,
            History = A::History,
            AppData = A::AppData,
            ScheduledAnnounces = A::ScheduledAnnounces,
            UpstreamAppDestinations = A::UpstreamAppDestinations,
            HeldIdentities = A::HeldIdentities,
            SelfRatchets = A::SelfRatchets,
            Receipts = A::Receipts,
            PacketHashes = A::PacketHashes,
            ReverseRoutes = A::ReverseRoutes,
            PendingPathRequests = A::PendingPathRequests,
            SeenPathRequests = A::SeenPathRequests,
            AnnounceRates = A::AnnounceRates,
            GroupKeys = A::GroupKeys,
            RequestHandlers = A::RequestHandlers,
            TransportedLinks = A::TransportedLinks,
            Links = A::Links,
            OutgoingResources = A::OutgoingResources,
            IncomingResources = A::IncomingResources,
        >,
    {
    }

    #[test]
    fn esp32s3_is_type_identical_to_its_positional_config() {
        assert_same_storage::<
            Esp32S3,
            FixedInline<24, 32, 1024, 4, 128, 4, 4, 32, 8, 8, 8, 8, 8, 4>,
        >();
    }
}
