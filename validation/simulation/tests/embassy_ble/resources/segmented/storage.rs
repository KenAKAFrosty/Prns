use personal_rns::routing::links::resources::assembly::{
    FixedIncomingAssemblyTable, FixedStaticOutgoingAssemblyTable,
};
use personal_rns::routing::links::resources::max_part_count;
use personal_rns::routing::links::resources::table::{
    FixedResourceTable, IncomingResourceState, OutgoingResourceState,
};
use personal_rns::storage::{DisplayedStorageLimits, GrowableHeap, StorageCapacity, StorageLayout};

pub(super) const TRANSFER_WINDOW_BYTES: usize = 512;
const PARTS: usize = max_part_count(TRANSFER_WINDOW_BYTES);

// Assembly and transfer pressure are test-local. Ordinary scenarios use one
// transfer slot; overlap scenarios reserve room for a second link's transfer.
pub(super) struct SegmentedStorage<const RECEIPTS: usize, const TRANSFERS: usize = 1>;

impl<const RECEIPTS: usize, const TRANSFERS: usize> StorageLayout
    for SegmentedStorage<RECEIPTS, TRANSFERS>
{
    const LIMITS: DisplayedStorageLimits = DisplayedStorageLimits {
        resource_transfer_bytes: StorageCapacity::Fixed(TRANSFER_WINDOW_BYTES),
        receipts: StorageCapacity::Fixed(RECEIPTS),
        ..GrowableHeap::LIMITS
    };

    type Routes = <GrowableHeap as StorageLayout>::Routes;
    type RouteExpiries = <GrowableHeap as StorageLayout>::RouteExpiries;
    type DestinationIdentities = <GrowableHeap as StorageLayout>::DestinationIdentities;
    type DestinationIdentityAppData = <GrowableHeap as StorageLayout>::DestinationIdentityAppData;
    type Announces = <GrowableHeap as StorageLayout>::Announces;
    type History = <GrowableHeap as StorageLayout>::History;
    type AppData = <GrowableHeap as StorageLayout>::AppData;
    type ScheduledAnnounces = <GrowableHeap as StorageLayout>::ScheduledAnnounces;
    type UpstreamAppDestinations = <GrowableHeap as StorageLayout>::UpstreamAppDestinations;
    type HeldIdentities = <GrowableHeap as StorageLayout>::HeldIdentities;
    type SelfRatchets = <GrowableHeap as StorageLayout>::SelfRatchets;
    type Receipts = FixedReceiptTable<RECEIPTS>;
    type PacketHashes = <GrowableHeap as StorageLayout>::PacketHashes;
    type Blackholes = <GrowableHeap as StorageLayout>::Blackholes;
    type ReverseRoutes = <GrowableHeap as StorageLayout>::ReverseRoutes;
    type DepartedInterfaces = <GrowableHeap as StorageLayout>::DepartedInterfaces;
    type PendingPathRequests = <GrowableHeap as StorageLayout>::PendingPathRequests;
    type RecentPathRequests = <GrowableHeap as StorageLayout>::RecentPathRequests;
    type SeenPathRequests = <GrowableHeap as StorageLayout>::SeenPathRequests;
    type Tunnels = <GrowableHeap as StorageLayout>::Tunnels;
    type RecursivePathRequests = <GrowableHeap as StorageLayout>::RecursivePathRequests;
    type InterfacePathRequestLimits = <GrowableHeap as StorageLayout>::InterfacePathRequestLimits;
    type InterfaceAnnounceLimits = <GrowableHeap as StorageLayout>::InterfaceAnnounceLimits;
    type HeldAnnounces = <GrowableHeap as StorageLayout>::HeldAnnounces;
    type HeldAnnounceAppData = <GrowableHeap as StorageLayout>::HeldAnnounceAppData;
    type DestinationAnnounceLimits = <GrowableHeap as StorageLayout>::DestinationAnnounceLimits;
    type GroupKeys = <GrowableHeap as StorageLayout>::GroupKeys;
    type RequestHandlers = <GrowableHeap as StorageLayout>::RequestHandlers;
    type TransportedLinks = <GrowableHeap as StorageLayout>::TransportedLinks;
    type Links = <GrowableHeap as StorageLayout>::Links;
    type OutgoingResources =
        FixedResourceTable<OutgoingResourceState, TRANSFERS, TRANSFER_WINDOW_BYTES, PARTS>;
    type IncomingResources =
        FixedResourceTable<IncomingResourceState, TRANSFERS, TRANSFER_WINDOW_BYTES, PARTS>;
    type PendingResourceOffers = <GrowableHeap as StorageLayout>::PendingResourceOffers;
    type IncomingAssemblies = FixedIncomingAssemblyTable<1>;
    type OutgoingAssemblies = FixedStaticOutgoingAssemblyTable<TRANSFERS>;
    type Channels = <GrowableHeap as StorageLayout>::Channels;
    type DirtyInterfaces = <GrowableHeap as StorageLayout>::DirtyInterfaces;
}
use personal_rns::routing::delivery::receipts::FixedReceiptTable;
