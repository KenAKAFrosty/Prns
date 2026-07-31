#[cfg(target_arch = "xtensa")]
use allocator_api2::alloc::{AllocError, Allocator};
#[cfg(target_arch = "xtensa")]
use allocator_api2::boxed::Box;
#[cfg(target_arch = "xtensa")]
use allocator_api2::vec::Vec;
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

#[cfg(any(target_arch = "xtensa", target_arch = "riscv32"))]
const _: () = assert!(
    match <EngineStorageType as personal_rns::storage::StorageLayout>::LIMITS
        .resource_transfer_bytes
    {
        personal_rns::storage::StorageCapacity::Fixed(bytes) =>
            personal_hopspot_core::node_pages::PAGE_RESPONSE_TRANSFER_BYTES <= bytes,
        personal_rns::storage::StorageCapacity::Dynamic => true,
    }
);

#[cfg(target_arch = "xtensa")]
const _: () = assert!(
    Esp32S3::<PsramAlloc>::MAX_COMPACTED_FLASH_JOURNAL_BYTES <= crate::persistence::S3_ARENA_BYTES
);

/// A `Default`-able allocator that places allocations in PSRAM. esp-alloc's own
/// `ExternalMemory` targets the same region but is not `Default`, which the column recipes
/// need to build themselves from `StorageLayout`; this thin ZST forwards to it.
#[cfg(target_arch = "xtensa")]
#[derive(Default, Clone, Copy)]
pub struct PsramAlloc;

#[cfg(target_arch = "xtensa")]
pub fn allocate_lora_tx_queue() -> &'static mut [u8; personal_rns::lora::LORA_TX_QUEUE_BYTES] {
    let mut storage = Vec::with_capacity_in(personal_rns::lora::LORA_TX_QUEUE_BYTES, PsramAlloc);
    storage.resize(personal_rns::lora::LORA_TX_QUEUE_BYTES, 0);
    Box::leak(storage.into_boxed_slice())
        .try_into()
        .expect("the LoRa transmit queue allocation has its requested length")
}

#[cfg(target_arch = "xtensa")]
// SAFETY: Every operation is forwarded unchanged to esp-alloc's ExternalMemory allocator. This ZST
// neither creates a second heap nor changes pointer/layout provenance.
unsafe impl Allocator for PsramAlloc {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        esp_alloc::ExternalMemory.allocate(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: The Allocator contract requires callers to return the same pointer/layout pair
        // produced by this allocator; forwarding preserves that contract and provenance.
        unsafe { esp_alloc::ExternalMemory.deallocate(ptr, layout) }
    }
}

#[cfg(target_arch = "riscv32")]
mod riscv {
    use personal_rns::crypto::ratchets::FixedSelfRatchetTable;
    use personal_rns::identity::destination_identity::{
        NoDestinationIdentityAppData, NoDestinationIdentityTable,
    };
    use personal_rns::identity::held::FixedHeldIdentityTable;
    use personal_rns::manifold::interface_seam::EMBEDDED_MAX_LINK_MTU;
    use personal_rns::routing::announce::destination_announce_limit::FixedDestinationAnnounceLimitTable;
    use personal_rns::routing::announce::held::FixedHeldAnnounceTable;
    use personal_rns::routing::announce::interface_announce_limit::FixedInterfaceAnnounceLimitTable;
    use personal_rns::routing::announce::schedule::FixedScheduledAnnounceQueue;
    use personal_rns::routing::announce::stored::{
        FixedAnnounceIdHistory, FixedArrayAnnounceRecordTable, PackedAppDataArena,
    };
    use personal_rns::routing::blackhole::{blackhole_index_buckets, FixedBlackholeTable};
    use personal_rns::routing::dedup::FixedPacketHashHistory;
    use personal_rns::routing::delivery::receipts::FixedReceiptTable;
    use personal_rns::routing::group_keys::FixedGroupKeyTable;
    use personal_rns::routing::links::channel::channel_mdu;
    use personal_rns::routing::links::channel::table::impls::FixedArrayChannelTable;
    use personal_rns::routing::links::resources::assembly::{
        FixedIncomingAssemblyTable, FixedOutgoingAssemblyTable,
    };
    use personal_rns::routing::links::resources::max_part_count;
    use personal_rns::routing::links::resources::table::{
        FixedResourceTable, IncomingResourceState, OutgoingResourceState,
    };
    use personal_rns::routing::links::table::FixedLinkTable;
    use personal_rns::routing::links::transported::FixedTransportedLinkTable;
    use personal_rns::routing::path_requests::interface_path_request_limit::FixedInterfacePathRequestLimitTable;
    use personal_rns::routing::path_requests::pending::FixedPendingPathRequestTable;
    use personal_rns::routing::path_requests::recent::FixedRecentPathRequestTable;
    use personal_rns::routing::path_requests::recursive::FixedRecursivePathRequestTable;
    use personal_rns::routing::path_requests::seen::FixedSeenPathRequestTable;
    use personal_rns::routing::request_handlers::FixedRequestHandlerTable;
    use personal_rns::routing::reverse_routes::FixedReverseRouteTable;
    use personal_rns::routing::routes::FixedArrayRouteTable;
    use personal_rns::routing::tunnel::FixedTunnelTable;
    use personal_rns::routing::upstream_app_destinations::FixedUpstreamAppDestinationTable;
    use personal_rns::routing::warmth::FixedDepartedInterfaceTable;
    use personal_rns::storage::{DisplayedStorageLimits, StorageCapacity, StorageLayout};

    /// The XIAO ESP32-C6 Hopspot's storage profile, sized to its internal SRAM
    /// and application role. This board is a headless USB/ESP-NOW/BLE mesh
    /// bridge: keep one local app identity, bias the budget toward heard
    /// destinations, and leave links, resources, and channel windows modest.
    pub struct C6Storage;

    impl C6Storage {
        const TRACKED_DESTINATIONS: usize = 36;
        const UPSTREAM_APP_DESTINATIONS: usize = 2;
        const HELD_IDENTITIES: usize = 1;
        const LINKS: usize = 2;
        const PACKET_HASHES: usize = 64;
        const BLACKHOLED_IDENTITIES: usize = 16;
        const BLACKHOLE_REASON_BYTES: usize = 64;
        const RESOURCE_TRANSFER_BYTES: usize =
            personal_hopspot_core::node_pages::PAGE_RESPONSE_TRANSFER_BYTES;
        const CHANNEL_REORDER_DEPTH: usize = 1;
        const LINK_MTU: usize = EMBEDDED_MAX_LINK_MTU;
        const CHANNEL_MESSAGE_BYTES: usize = channel_mdu(Self::LINK_MTU);
        pub(super) const MAX_COMPACTED_FLASH_JOURNAL_BYTES: usize = Self::TRACKED_DESTINATIONS
            * (personal_rns::persistence::flash_journal_record_storage_len(
                personal_rns::persistence::maximum_route_upsert_payload_len(0, 0),
                4,
            ) + 3)
            + 512
            + Self::UPSTREAM_APP_DESTINATIONS
                * personal_rns::persistence::flash_journal_record_storage_len(
                    personal_rns::wire::TRUNCATED_HASH_BYTE_LEN
                        + personal_rns::persistence::self_ratchets_snapshot_len(8),
                    4,
                )
            + personal_rns::persistence::flash_journal_record_storage_len(0, 4);
    }

    impl StorageLayout for C6Storage {
        const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
            tracked_destinations: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
            destination_identities: StorageCapacity::Fixed(0),
            announce_records: StorageCapacity::Fixed(Self::TRACKED_DESTINATIONS),
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
            blackholed_identities: StorageCapacity::Fixed(Self::BLACKHOLED_IDENTITIES),
            blackhole_reason_bytes: StorageCapacity::Fixed(Self::BLACKHOLE_REASON_BYTES),
            reverse_routes: StorageCapacity::Fixed(8),
            pending_path_requests: StorageCapacity::Fixed(8),
            held_announces: StorageCapacity::Fixed(32),
            ratchets_per_destination: StorageCapacity::Fixed(8),
        };

        type Routes = FixedArrayRouteTable<{ Self::TRACKED_DESTINATIONS }>;
        type RouteExpiries = personal_rns::routing::LinearRouteExpiryIndex;
        type DestinationIdentities = NoDestinationIdentityTable;
        type DestinationIdentityAppData = NoDestinationIdentityAppData;
        type Tunnels = FixedTunnelTable<0>;
        type Announces = FixedArrayAnnounceRecordTable<{ Self::TRACKED_DESTINATIONS }>;
        type History = FixedAnnounceIdHistory<{ Self::TRACKED_DESTINATIONS }, 8>;
        type AppData = PackedAppDataArena<512, { Self::TRACKED_DESTINATIONS }>;
        type ScheduledAnnounces = FixedScheduledAnnounceQueue<{ Self::TRACKED_DESTINATIONS }>;
        type UpstreamAppDestinations =
            FixedUpstreamAppDestinationTable<{ Self::UPSTREAM_APP_DESTINATIONS }>;
        type HeldIdentities = FixedHeldIdentityTable<{ Self::HELD_IDENTITIES }>;
        type SelfRatchets = FixedSelfRatchetTable<{ Self::UPSTREAM_APP_DESTINATIONS }, 8>;
        type Receipts = FixedReceiptTable<8>;
        type PacketHashes = FixedPacketHashHistory<{ Self::PACKET_HASHES }>;
        type Blackholes = FixedBlackholeTable<
            { Self::BLACKHOLED_IDENTITIES },
            { blackhole_index_buckets(Self::BLACKHOLED_IDENTITIES) },
            { Self::BLACKHOLE_REASON_BYTES },
        >;
        type ReverseRoutes = FixedReverseRouteTable<8>;
        type DepartedInterfaces = FixedDepartedInterfaceTable<8>;
        type PendingPathRequests = FixedPendingPathRequestTable<8>;
        type RecentPathRequests = FixedRecentPathRequestTable<8>;
        type SeenPathRequests = FixedSeenPathRequestTable<8>;
        type RecursivePathRequests = FixedRecursivePathRequestTable<8>;
        type InterfacePathRequestLimits = FixedInterfacePathRequestLimitTable<8>;
        type InterfaceAnnounceLimits = FixedInterfaceAnnounceLimitTable<8>;
        type DirtyInterfaces = heapless::Vec<personal_rns::interfaces::InterfaceId, 8>;
        type HeldAnnounces = FixedHeldAnnounceTable<32>;
        type HeldAnnounceAppData = PackedAppDataArena<2048, 32>;
        type DestinationAnnounceLimits =
            FixedDestinationAnnounceLimitTable<{ Self::TRACKED_DESTINATIONS }>;
        type GroupKeys = FixedGroupKeyTable<{ Self::UPSTREAM_APP_DESTINATIONS }>;
        type RequestHandlers = FixedRequestHandlerTable<{ Self::UPSTREAM_APP_DESTINATIONS }>;
        type TransportedLinks = FixedTransportedLinkTable<{ Self::LINKS }>;
        type Links = FixedLinkTable<{ Self::LINKS }>;
        type OutgoingResources = FixedResourceTable<
            OutgoingResourceState,
            1,
            { Self::RESOURCE_TRANSFER_BYTES },
            { max_part_count(Self::RESOURCE_TRANSFER_BYTES) },
        >;
        type IncomingResources = FixedResourceTable<
            IncomingResourceState,
            1,
            { Self::RESOURCE_TRANSFER_BYTES },
            { max_part_count(Self::RESOURCE_TRANSFER_BYTES) },
        >;
        type IncomingAssemblies = FixedIncomingAssemblyTable<{ Self::LINKS }>;
        type OutgoingAssemblies = FixedOutgoingAssemblyTable<{ Self::LINKS }>;
        type Channels = FixedArrayChannelTable<
            { Self::LINKS },
            { Self::CHANNEL_REORDER_DEPTH },
            { Self::CHANNEL_MESSAGE_BYTES },
        >;
    }
}

#[cfg(target_arch = "riscv32")]
const _: () =
    assert!(C6Storage::MAX_COMPACTED_FLASH_JOURNAL_BYTES <= crate::persistence::C6_ARENA_BYTES);
