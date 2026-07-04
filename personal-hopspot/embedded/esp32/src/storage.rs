#[cfg(target_arch = "xtensa")]
use allocator_api2::alloc::{AllocError, Allocator};
#[cfg(target_arch = "xtensa")]
use core::alloc::Layout;
#[cfg(target_arch = "xtensa")]
use core::ptr::NonNull;

#[cfg(target_arch = "xtensa")]
use personal_rns::storage::Esp32S3;

/// The engine's storage recipe: hot/synchronized columns stay inline in SRAM, the cold
/// per-destination bulk (routes, announces, history, app-data, resource buffers) is boxed
/// into PSRAM through `PsramAlloc`, each keeping its small index/metadata in SRAM.
#[cfg(target_arch = "xtensa")]
pub type EngineStorageType = Esp32S3<PsramAlloc>;

#[cfg(target_arch = "riscv32")]
pub use riscv::C6Storage;
#[cfg(target_arch = "riscv32")]
pub type EngineStorageType = riscv::C6Storage;

/// A `Default`-able allocator that places allocations in PSRAM. esp-alloc's own
/// `ExternalMemory` targets the same region but is not `Default`, which the column recipes
/// need to build themselves from `StorageLayout`; this thin ZST forwards to it.
#[cfg(target_arch = "xtensa")]
#[derive(Default, Clone, Copy)]
pub struct PsramAlloc;

#[cfg(target_arch = "xtensa")]
unsafe impl Allocator for PsramAlloc {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        esp_alloc::ExternalMemory.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { esp_alloc::ExternalMemory.deallocate(ptr, layout) }
    }
}

#[cfg(target_arch = "riscv32")]
mod riscv {
    use personal_rns::crypto::ratchets::FixedSelfRatchetColumns;
    use personal_rns::identity::held::FixedHeldIdentityColumns;
    use personal_rns::reactor::interface_seam::EMBEDDED_MAX_LINK_MTU;
    use personal_rns::routing::announce::held::FixedHeldAnnounceColumns;
    use personal_rns::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitColumns;
    use personal_rns::routing::announce::rate_limit::FixedAnnounceRateColumns;
    use personal_rns::routing::announce::retained::{
        FixedArrayRetainedAnnounceColumns, PackedAppDataArena, TieredAnnounceIdHistory,
    };
    use personal_rns::routing::announce::schedule::FixedScheduledAnnounceQueue;
    use personal_rns::routing::dedup::FixedPacketHashHistory;
    use personal_rns::routing::delivery::receipts::FixedReceiptColumns;
    use personal_rns::routing::group_keys::FixedGroupKeyColumns;
    use personal_rns::routing::links::channel::channel_mdu;
    use personal_rns::routing::links::channel::impls::FixedArrayChannelColumns;
    use personal_rns::routing::links::resources::assembly::{
        FixedIncomingAssemblyColumns, FixedOutgoingAssemblyColumns,
    };
    use personal_rns::routing::links::resources::max_part_count;
    use personal_rns::routing::links::resources::table::{
        FixedResourceColumns, IncomingResourceState, OutgoingResourceState,
    };
    use personal_rns::routing::links::table::FixedLinkColumns;
    use personal_rns::routing::links::transported::FixedTransportedLinkColumns;
    use personal_rns::routing::path_requests::discovery::FixedDiscoveryPathRequestColumns;
    use personal_rns::routing::path_requests::interface_path_request_limit::FixedInterfacePathRequestLimitColumns;
    use personal_rns::routing::path_requests::pending::FixedPendingPathRequestColumns;
    use personal_rns::routing::path_requests::recent::FixedRecentPathRequestColumns;
    use personal_rns::routing::path_requests::seen::FixedSeenPathRequestColumns;
    use personal_rns::routing::request_handlers::FixedRequestHandlerColumns;
    use personal_rns::routing::reverse_routes::FixedReverseRouteColumns;
    use personal_rns::routing::routes::FixedArrayRouteColumns;
    use personal_rns::routing::tunnel::FixedTunnelColumns;
    use personal_rns::routing::upstream_app_destinations::FixedUpstreamAppDestinationColumns;
    use personal_rns::routing::warmth::FixedDepartedInterfaceColumns;
    use personal_rns::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

    /// The C6's storage profile, sized to internal SRAM. Distinct from the library's
    /// `personal_rns::storage::Esp32C6`, whose `LINK_MTU = 8192` + reorder 8 needs ~256 KB of channel
    /// buffers and overflows the chip it is named for. This board is a headless USB/ESP-NOW/BLE mesh
    /// bridge: keep one local app identity, bias the budget toward heard destinations, and leave links,
    /// resources, and channel windows modest.
    pub struct C6Storage;

    impl C6Storage {
        const TRACKED_DESTINATIONS: usize = 36;
        const UPSTREAM_APP_DESTINATIONS: usize = 1;
        const HELD_IDENTITIES: usize = 1;
        const LINKS: usize = 2;
        const PACKET_HASHES: usize = 64;
        const RESOURCE_TRANSFER_BYTES: usize = 1024;
        const CHANNEL_REORDER_DEPTH: usize = 1;
        const LINK_MTU: usize = EMBEDDED_MAX_LINK_MTU;
        const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(Self::LINK_MTU);
    }

    impl StorageLayout for C6Storage {
        const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
            tracked_destinations: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
            retained_announces: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
            upstream_app_destinations: StorageCapacity::Fixed(Self::UPSTREAM_APP_DESTINATIONS),
            held_identities: StorageCapacity::Fixed(Self::HELD_IDENTITIES),
            links: StorageCapacity::Fixed(Self::LINKS),
            channels: StorageCapacity::Fixed(Self::LINKS),
            channel_window_pool: None,
            channel_reorder_depth: StorageCapacity::Fixed(Self::CHANNEL_REORDER_DEPTH),
            link_mtu: StorageCapacity::Fixed(Self::LINK_MTU),
            resource_transfer_bytes: StorageCapacity::Fixed(Self::RESOURCE_TRANSFER_BYTES),
            receipts: StorageCapacity::Fixed(8),
            packet_hashes: StorageCapacity::Fixed(Self::PACKET_HASHES),
            reverse_routes: StorageCapacity::Fixed(8),
            pending_path_requests: StorageCapacity::Fixed(8),
            held_announces: StorageCapacity::Fixed(8),
            ratchets_per_destination: StorageCapacity::Fixed(8),
        };

        type Routes = FixedArrayRouteColumns<{ Self::TRACKED_DESTINATIONS }>;
        type Tunnels = FixedTunnelColumns<0>;
        type Announces = FixedArrayRetainedAnnounceColumns<{ Self::TRACKED_DESTINATIONS }>;
        type History = TieredAnnounceIdHistory<4, 64, { Self::TRACKED_DESTINATIONS }, 16>;
        type AppData = PackedAppDataArena<512, { Self::TRACKED_DESTINATIONS }>;
        type ScheduledAnnounces = FixedScheduledAnnounceQueue<{ Self::TRACKED_DESTINATIONS }>;
        type UpstreamAppDestinations =
            FixedUpstreamAppDestinationColumns<{ Self::UPSTREAM_APP_DESTINATIONS }>;
        type HeldIdentities = FixedHeldIdentityColumns<{ Self::HELD_IDENTITIES }>;
        type SelfRatchets = FixedSelfRatchetColumns<{ Self::UPSTREAM_APP_DESTINATIONS }, 8>;
        type Receipts = FixedReceiptColumns<8>;
        type PacketHashes = FixedPacketHashHistory<{ Self::PACKET_HASHES }>;
        type ReverseRoutes = FixedReverseRouteColumns<8>;
        type DepartedInterfaces = FixedDepartedInterfaceColumns<8>;
        type PendingPathRequests = FixedPendingPathRequestColumns<8>;
        type RecentPathRequests = FixedRecentPathRequestColumns<8>;
        type SeenPathRequests = FixedSeenPathRequestColumns<8>;
        type DiscoveryPathRequests = FixedDiscoveryPathRequestColumns<8>;
        type InterfacePathRequestLimits = FixedInterfacePathRequestLimitColumns<8>;
        type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitColumns<8>;
        type DirtyInterfaces = heapless::Vec<personal_rns::interfaces::InterfaceId, 8>;
        type HeldAnnounces = FixedHeldAnnounceColumns<8>;
        type HeldAnnounceAppData = PackedAppDataArena<512, 8>;
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
}
