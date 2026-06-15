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
use crate::routing::links::channel::channel_mdu;
use crate::routing::links::channel::impls::FixedArrayChannelColumns;
use crate::routing::links::resources::max_part_count;
use crate::routing::links::resources::table::{
    FixedResourceColumns, IncomingResourceState, OutgoingResourceState,
};
use crate::routing::links::table::FixedLinkColumns;
use crate::routing::links::transported::FixedTransportedLinkColumns;
use crate::routing::links::MAX_LINK_MTU;
use crate::routing::path_requests::pending::FixedPendingPathRequestColumns;
use crate::routing::path_requests::recent::FixedRecentPathRequestColumns;
use crate::routing::path_requests::seen::FixedSeenPathRequestColumns;
use crate::routing::request_handlers::FixedRequestHandlerColumns;
use crate::routing::reverse_routes::FixedReverseRouteColumns;
use crate::routing::routes::FixedArrayRouteColumns;
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;
use crate::storage::StorageLayout;

pub struct Esp32C6;

impl Esp32C6 {
    const TRACKED_DESTINATIONS: usize = 24;
    const UPSTREAM_APP_DESTINATIONS: usize = 4;
    const LINKS: usize = 4;
    const RESOURCE_TRANSFER_BYTES: usize = 4096;
    const CHANNEL_REORDER_DEPTH: usize = 8;
    const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(MAX_LINK_MTU);
}

impl StorageLayout for Esp32C6 {
    type Routes = FixedArrayRouteColumns<{ Self::TRACKED_DESTINATIONS }>;
    type Announces = FixedArrayRetainedAnnounceColumns<{ Self::TRACKED_DESTINATIONS }>;
    type History = TieredAnnounceIdHistory<4, 128, { Self::TRACKED_DESTINATIONS }, 32>;
    type AppData = PackedAppDataArena<1024, { Self::TRACKED_DESTINATIONS }>;
    type ScheduledAnnounces = FixedScheduledAnnounceQueue<{ Self::TRACKED_DESTINATIONS }>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationColumns<{ Self::UPSTREAM_APP_DESTINATIONS }>;
    type HeldIdentities = FixedHeldIdentityColumns<4>;
    type SelfRatchets = FixedSelfRatchetColumns<{ Self::UPSTREAM_APP_DESTINATIONS }, 8>;
    type Receipts = FixedReceiptColumns<8>;
    type PacketHashes = FixedPacketHashHistory<32>;
    type ReverseRoutes = FixedReverseRouteColumns<8>;
    type PendingPathRequests = FixedPendingPathRequestColumns<8>;
    type RecentPathRequests = FixedRecentPathRequestColumns<8>;
    type SeenPathRequests = FixedSeenPathRequestColumns<8>;
    type AnnounceRates = FixedAnnounceRateColumns<{ Self::TRACKED_DESTINATIONS }>;
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
    type Channels = FixedArrayChannelColumns<
        { Self::LINKS },
        { Self::CHANNEL_REORDER_DEPTH },
        { Self::CHANNEL_MESSAGE_BYTES },
    >;
}

#[cfg(test)]
mod tests {
    use super::Esp32C6;
    use crate::storage::impls::assert_same_storage;
    use crate::storage::TestFixedStorage;

    #[test]
    fn esp32c6_is_type_identical_to_its_positional_config() {
        assert_same_storage::<
            Esp32C6,
            TestFixedStorage<24, 32, 1024, 4, 128, 4, 4, 32, 8, 8, 8, 8, 8, 4>,
        >();
    }
}
