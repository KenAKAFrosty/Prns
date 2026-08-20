use super::DirtyInterfaceSet;
use crate::crypto::ratchets::SelfRatchetTable;
use crate::identity::destination_identity::DestinationIdentityTable;
use crate::identity::held::HeldIdentityTable;
use crate::routing::announce::destination_announce_limit::DestinationAnnounceLimitTable;
use crate::routing::announce::held::HeldAnnounceTable;
use crate::routing::announce::interface_announce_limit::InterfaceAnnounceLimitTable;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecordTable};
use crate::routing::blackhole::BlackholeTable;
use crate::routing::dedup::PacketHashHistory;
use crate::routing::delivery::receipts::ReceiptTable;
use crate::routing::group_keys::GroupKeyTable;
use crate::routing::links::channel::table::ChannelTable;
use crate::routing::links::resources::assembly::{IncomingAssemblyTable, OutgoingAssemblyTable};
use crate::routing::links::resources::pending::PendingResourceOfferTable;
use crate::routing::links::resources::table::{
    IncomingResourceState, OutgoingResourceState, ResourceTable,
};
use crate::routing::links::table::LinkTable;
use crate::routing::links::transported::TransportedLinkTable;
use crate::routing::path_requests::interface_path_request_limit::InterfacePathRequestLimitTable;
use crate::routing::path_requests::pending::PendingPathRequestTable;
use crate::routing::path_requests::recent::RecentPathRequestTable;
use crate::routing::path_requests::recursive::RecursivePathRequestTable;
use crate::routing::path_requests::seen::SeenPathRequestTable;
use crate::routing::request_handlers::RequestHandlerTable;
use crate::routing::reverse_routes::ReverseRouteTable;
use crate::routing::route_expiry::RouteExpiryIndex;
use crate::routing::routes::RouteTable;
use crate::routing::tunnel::registry::TunnelTable;
use crate::routing::upstream_app_destinations::UpstreamAppDestinationTable;
use crate::routing::warmth::DepartedInterfaceTable;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TablePushError {
    TableFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCapacity {
    Fixed(usize),
    Dynamic,
}

/// The sizing story a status face renders; enforcement lives in the tables themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayedStorageLimits {
    pub tracked_destinations: StorageCapacity,
    pub destination_identities: StorageCapacity,
    pub announce_records: StorageCapacity,
    pub upstream_app_destinations: StorageCapacity,
    pub held_identities: StorageCapacity,
    pub links: StorageCapacity,
    pub channels: StorageCapacity,
    pub channel_window_pool: Option<usize>,
    pub channel_reorder_depth: StorageCapacity,
    pub link_mtu: StorageCapacity,
    pub resource_transfer_bytes: StorageCapacity,
    pub receipts: StorageCapacity,
    pub packet_hashes: StorageCapacity,
    pub blackholed_identities: StorageCapacity,
    pub blackhole_reason_bytes: StorageCapacity,
    pub reverse_routes: StorageCapacity,
    pub pending_path_requests: StorageCapacity,
    pub held_announces: StorageCapacity,
    pub ratchets_per_destination: StorageCapacity,
}

impl DisplayedStorageLimits {
    pub const DYNAMIC: Self = Self {
        tracked_destinations: StorageCapacity::Dynamic,
        destination_identities: StorageCapacity::Dynamic,
        announce_records: StorageCapacity::Dynamic,
        upstream_app_destinations: StorageCapacity::Dynamic,
        held_identities: StorageCapacity::Dynamic,
        links: StorageCapacity::Dynamic,
        channels: StorageCapacity::Dynamic,
        channel_window_pool: None,
        channel_reorder_depth: StorageCapacity::Dynamic,
        link_mtu: StorageCapacity::Dynamic,
        resource_transfer_bytes: StorageCapacity::Dynamic,
        receipts: StorageCapacity::Dynamic,
        packet_hashes: StorageCapacity::Dynamic,
        blackholed_identities: StorageCapacity::Dynamic,
        blackhole_reason_bytes: StorageCapacity::Dynamic,
        reverse_routes: StorageCapacity::Dynamic,
        pending_path_requests: StorageCapacity::Dynamic,
        held_announces: StorageCapacity::Dynamic,
        ratchets_per_destination: StorageCapacity::Dynamic,
    };
}

pub trait StorageLayout {
    const LIMITS: DisplayedStorageLimits;

    type Routes: RouteTable + Default;
    type RouteExpiries: RouteExpiryIndex;
    type DestinationIdentities: DestinationIdentityTable + Default;
    type DestinationIdentityAppData: AnnounceAppData + Default;
    type Announces: AnnounceRecordTable + Default;
    type History: AnnounceIdHistory + Default;
    type AppData: AnnounceAppData + Default;
    type ScheduledAnnounces: ScheduledAnnounceQueue + Default;
    type UpstreamAppDestinations: UpstreamAppDestinationTable + Default;
    type HeldIdentities: HeldIdentityTable + Default;
    type SelfRatchets: SelfRatchetTable + Default;
    type Receipts: ReceiptTable + Default;
    type PacketHashes: PacketHashHistory + Default;
    type Blackholes: BlackholeTable + Default;
    type ReverseRoutes: ReverseRouteTable + Default;
    type PendingPathRequests: PendingPathRequestTable + Default;
    type RecentPathRequests: RecentPathRequestTable + Default;
    type SeenPathRequests: SeenPathRequestTable + Default;
    type Tunnels: TunnelTable + Default;
    type DepartedInterfaces: DepartedInterfaceTable + Default;
    type RecursivePathRequests: RecursivePathRequestTable + Default;
    type InterfacePathRequestLimits: InterfacePathRequestLimitTable + Default;
    type InterfaceAnnounceLimits: InterfaceAnnounceLimitTable + Default;
    type HeldAnnounces: HeldAnnounceTable + Default;
    type HeldAnnounceAppData: AnnounceAppData + Default;
    type DestinationAnnounceLimits: DestinationAnnounceLimitTable + Default;
    type GroupKeys: GroupKeyTable + Default;
    type RequestHandlers: RequestHandlerTable + Default;
    type TransportedLinks: TransportedLinkTable + Default;
    type Links: LinkTable + Default;
    type OutgoingResources: ResourceTable<OutgoingResourceState> + Default;
    type IncomingResources: ResourceTable<IncomingResourceState> + Default;
    type PendingResourceOffers: PendingResourceOfferTable + Default;
    type IncomingAssemblies: IncomingAssemblyTable + Default;
    type OutgoingAssemblies: OutgoingAssemblyTable + Default;
    type Channels: ChannelTable + Default;
    type DirtyInterfaces: DirtyInterfaceSet + Default;
}
