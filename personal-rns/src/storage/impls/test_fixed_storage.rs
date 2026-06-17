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
use crate::routing::links::resources::table::{
    FixedResourceColumns, IncomingResourceState, OutgoingResourceState,
};
use crate::routing::links::table::FixedLinkColumns;
use crate::routing::links::transported::FixedTransportedLinkColumns;
use crate::routing::path_requests::discovery::FixedDiscoveryPathRequestColumns;
use crate::routing::path_requests::pending::FixedPendingPathRequestColumns;
use crate::routing::path_requests::recent::FixedRecentPathRequestColumns;
use crate::routing::path_requests::seen::FixedSeenPathRequestColumns;
use crate::routing::request_handlers::FixedRequestHandlerColumns;
use crate::routing::reverse_routes::FixedReverseRouteColumns;
use crate::routing::routes::FixedArrayRouteColumns;
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;
use crate::storage::StorageLayout;

pub struct TestFixedStorage<
    const MAX_TRACKED_DESTINATIONS: usize,
    const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
    const HISTORY_FLOOR_PER_DESTINATION: usize,
    const HISTORY_OVERFLOW_CAPACITY: usize,
    const MAX_UPSTREAM_APP_DESTINATIONS: usize,
    const MAX_HELD_IDENTITIES: usize,
    const PACKET_HASH_GENERATION_CAPACITY: usize,
    const RETAINED_RATCHETS_PER_DESTINATION: usize,
    const MAX_OUTSTANDING_RECEIPTS: usize,
    const MAX_REVERSE_ROUTES: usize,
    const MAX_PENDING_PATH_REQUESTS: usize,
    const MAX_SEEN_PATH_REQUESTS: usize,
    const MAX_LINKS: usize,
>;

impl<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
        const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
        const HISTORY_FLOOR_PER_DESTINATION: usize,
        const HISTORY_OVERFLOW_CAPACITY: usize,
        const MAX_UPSTREAM_APP_DESTINATIONS: usize,
        const MAX_HELD_IDENTITIES: usize,
        const PACKET_HASH_GENERATION_CAPACITY: usize,
        const RETAINED_RATCHETS_PER_DESTINATION: usize,
        const MAX_OUTSTANDING_RECEIPTS: usize,
        const MAX_REVERSE_ROUTES: usize,
        const MAX_PENDING_PATH_REQUESTS: usize,
        const MAX_SEEN_PATH_REQUESTS: usize,
        const MAX_LINKS: usize,
    > StorageLayout
    for TestFixedStorage<
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
        ANNOUNCE_APP_DATA_ARENA_BYTES,
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
        MAX_UPSTREAM_APP_DESTINATIONS,
        MAX_HELD_IDENTITIES,
        PACKET_HASH_GENERATION_CAPACITY,
        RETAINED_RATCHETS_PER_DESTINATION,
        MAX_OUTSTANDING_RECEIPTS,
        MAX_REVERSE_ROUTES,
        MAX_PENDING_PATH_REQUESTS,
        MAX_SEEN_PATH_REQUESTS,
        MAX_LINKS,
    >
{
    type Routes = FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>;
    type Announces = FixedArrayRetainedAnnounceColumns<MAX_TRACKED_DESTINATIONS>;
    type History = TieredAnnounceIdHistory<
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
    >;
    type AppData = PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>;
    // Sized by the routing table (one pending per tracked route), not the held
    // cache — there is never more than one pending rebroadcast per destination.
    type ScheduledAnnounces = FixedScheduledAnnounceQueue<MAX_TRACKED_DESTINATIONS>;
    type UpstreamAppDestinations =
        FixedUpstreamAppDestinationColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type HeldIdentities = FixedHeldIdentityColumns<MAX_HELD_IDENTITIES>;
    // Sized by the upstream registry: every registered Single may opt into
    // ratchets, and nothing else can.
    type SelfRatchets =
        FixedSelfRatchetColumns<MAX_UPSTREAM_APP_DESTINATIONS, RETAINED_RATCHETS_PER_DESTINATION>;
    type PacketHashes = FixedPacketHashHistory<PACKET_HASH_GENERATION_CAPACITY>;
    type Receipts = FixedReceiptColumns<MAX_OUTSTANDING_RECEIPTS>;
    type ReverseRoutes = FixedReverseRouteColumns<MAX_REVERSE_ROUTES>;
    type PendingPathRequests = FixedPendingPathRequestColumns<MAX_PENDING_PATH_REQUESTS>;
    type RecentPathRequests = FixedRecentPathRequestColumns<MAX_PENDING_PATH_REQUESTS>;
    type SeenPathRequests = FixedSeenPathRequestColumns<MAX_SEEN_PATH_REQUESTS>;
    type DiscoveryPathRequests = FixedDiscoveryPathRequestColumns<MAX_PENDING_PATH_REQUESTS>;
    // One rate entry per tracked destination at most (we rate-check only what we
    // would rebroadcast), so sized by the routing table like Pending/Directives.
    type AnnounceRates = FixedAnnounceRateColumns<MAX_TRACKED_DESTINATIONS>;
    // At most one key per registered GROUP destination, so sized by the upstream
    // registry like SelfRatchets.
    type GroupKeys = FixedGroupKeyColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type RequestHandlers = FixedRequestHandlerColumns<MAX_UPSTREAM_APP_DESTINATIONS>;
    type TransportedLinks = FixedTransportedLinkColumns<MAX_LINKS>;
    type Links = FixedLinkColumns<MAX_LINKS>;
    // One in-flight transfer per direction at a deliberate 4 KiB floor: the
    // engine queries these capacities and refuses larger offers by name, so
    // a target wanting real transfers assembles its own bundle with the
    // sizes its RAM affords.
    type OutgoingResources = FixedResourceColumns<OutgoingResourceState, 1, 4096, 9>;
    type IncomingResources = FixedResourceColumns<IncomingResourceState, 1, 4096, 9>;
    type Channels = FixedArrayChannelColumns<MAX_LINKS, 8, { channel_mdu(8192) }>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::ratchets::SelfRatchetColumns;
    use crate::routing::announce::retained::RetainedAnnounceColumns;
    use crate::routing::dedup::PacketHashHistory;
    use crate::routing::delivery::receipts::ReceiptColumns;
    use crate::routing::links::table::LinkColumns;
    use crate::routing::routes::RouteColumns;
    use crate::routing::upstream_app_destinations::UpstreamAppDestinationColumns;

    #[test]
    fn bundles_fixed_array_backends_sized_by_the_consts() {
        type S = TestFixedStorage<8, 16, 256, 2, 8, 2, 2, 4, 3, 5, 8, 4, 8, 6>;
        let routes = <S as StorageLayout>::Routes::default();
        let announces = <S as StorageLayout>::Announces::default();
        let _history = <S as StorageLayout>::History::default();
        let _app_data = <S as StorageLayout>::AppData::default();
        let _pending = <S as StorageLayout>::ScheduledAnnounces::default();
        let upstream_app_destinations = <S as StorageLayout>::UpstreamAppDestinations::default();
        let packet_hashes = <S as StorageLayout>::PacketHashes::default();
        let self_ratchets = <S as StorageLayout>::SelfRatchets::default();
        assert_eq!(routes.capacity(), 8);
        assert_eq!(announces.capacity(), 8);
        assert_eq!(upstream_app_destinations.capacity(), 2);
        assert_eq!(packet_hashes.generation_capacity(), 4);
        assert_eq!(self_ratchets.capacity(), 2);
        assert_eq!(self_ratchets.retained_per_destination(), 3);
        let receipts = <S as StorageLayout>::Receipts::default();
        assert_eq!(receipts.capacity(), 5);
        let links = <S as StorageLayout>::Links::default();
        assert_eq!(links.capacity(), 6);
    }
}
