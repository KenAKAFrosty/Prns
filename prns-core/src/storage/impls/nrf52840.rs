use crate::crypto::ratchets::FixedSelfRatchetColumns;
use crate::identity::held::FixedHeldIdentityColumns;
use crate::routing::announce::held::FixedHeldAnnounceColumns;
use crate::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitColumns;
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
use crate::routing::links::resources::assembly::{
    FixedIncomingAssemblyColumns, FixedOutgoingAssemblyColumns,
};
use crate::routing::links::resources::max_part_count;
use crate::routing::links::resources::table::{
    FixedResourceColumns, IncomingResourceState, OutgoingResourceState,
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
use crate::routing::routes::FixedArrayRouteColumns;
use crate::routing::tunnel::FixedTunnelColumns;
use crate::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;
use crate::routing::warmth::FixedDepartedInterfaceColumns;
use crate::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

pub struct Nrf52840;

impl Nrf52840 {
    const TRACKED_DESTINATIONS: usize = 24;
    const UPSTREAM_APP_DESTINATIONS: usize = 4;
    const LINKS: usize = 4;
    const RESOURCE_TRANSFER_BYTES: usize = 4096;
    const CHANNEL_REORDER_DEPTH: usize = 8;
    const LINK_MTU: usize = 8192;
    const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(Self::LINK_MTU);
}

impl StorageLayout for Nrf52840 {
    const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
        tracked_destinations: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
        retained_announces: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
        upstream_app_destinations: StorageCapacity::Fixed(Self::UPSTREAM_APP_DESTINATIONS),
        held_identities: StorageCapacity::Fixed(4),
        links: StorageCapacity::Fixed(Self::LINKS),
        channels: StorageCapacity::Fixed(Self::LINKS),
        channel_window_pool: None,
        channel_reorder_depth: StorageCapacity::Fixed(Self::CHANNEL_REORDER_DEPTH),
        link_mtu: StorageCapacity::Fixed(Self::LINK_MTU),
        resource_transfer_bytes: StorageCapacity::Fixed(Self::RESOURCE_TRANSFER_BYTES),
        receipts: StorageCapacity::Fixed(8),
        packet_hashes: StorageCapacity::Fixed(32),
        reverse_routes: StorageCapacity::Fixed(8),
        pending_path_requests: StorageCapacity::Fixed(8),
        held_announces: StorageCapacity::Fixed(8),
        ratchets_per_destination: StorageCapacity::Fixed(8),
    };

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
    type DepartedInterfaces = FixedDepartedInterfaceColumns<8>;
    type PendingPathRequests = FixedPendingPathRequestColumns<8>;
    type RecentPathRequests = FixedRecentPathRequestColumns<8>;
    type SeenPathRequests = FixedSeenPathRequestColumns<8>;
    type Tunnels = FixedTunnelColumns<0>;
    type DiscoveryPathRequests = FixedDiscoveryPathRequestColumns<8>;
    type InterfacePathRequestLimits = FixedInterfacePathRequestLimitColumns<8>;
    type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitColumns<8>;
    type DirtyInterfaces = heapless::Vec<crate::interfaces::InterfaceId, 8>;
    type HeldAnnounces = FixedHeldAnnounceColumns<8>;
    type HeldAnnounceAppData = PackedAppDataArena<1024, 8>;
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
    type IncomingAssemblies = FixedIncomingAssemblyColumns<{ Self::LINKS }>;
    type OutgoingAssemblies = FixedOutgoingAssemblyColumns<{ Self::LINKS }>;
    type Channels = FixedArrayChannelColumns<
        { Self::LINKS },
        { Self::CHANNEL_REORDER_DEPTH },
        { Self::CHANNEL_MESSAGE_BYTES },
    >;
}

#[cfg(test)]
mod tests {
    use super::Nrf52840;
    use crate::storage::impls::assert_same_storage;
    use crate::storage::TestFixedStorage;

    #[test]
    fn nrf52840_is_type_identical_to_its_positional_config() {
        assert_same_storage::<
            Nrf52840,
            TestFixedStorage<24, 32, 1024, 4, 128, 4, 4, 32, 8, 8, 8, 8, 8, 4>,
        >();
    }
}
