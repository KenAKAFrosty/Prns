mod dirty;
mod impls;

pub use dirty::DirtyInterfaceSet;
pub use impls::*;

use crate::crypto::ratchets::SelfRatchetColumns;
use crate::identity::held::HeldIdentityColumns;
use crate::routing::announce::destination_announce_limit::DestinationAnnounceLimitColumns;
use crate::routing::announce::held::HeldStore;
use crate::routing::announce::interface_announce_limit::InterfaceAnnounceLimitColumns;
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecordColumns};
use crate::routing::dedup::PacketHashHistory;
use crate::routing::delivery::receipts::ReceiptColumns;
use crate::routing::group_keys::GroupKeyColumns;
use crate::routing::links::channel::columns::ChannelColumns;
use crate::routing::links::resources::assembly::{
    IncomingAssemblyColumns, OutgoingAssemblyColumns,
};
use crate::routing::links::resources::table::{
    IncomingResourceState, OutgoingResourceState, ResourceColumns,
};
use crate::routing::links::table::LinkColumns;
use crate::routing::links::transported::TransportedLinkColumns;
use crate::routing::path_requests::interface_path_request_limit::InterfacePathRequestLimitColumns;
use crate::routing::path_requests::pending::PendingPathRequestColumns;
use crate::routing::path_requests::recent::RecentPathRequestColumns;
use crate::routing::path_requests::recursive::RecursivePathRequestColumns;
use crate::routing::path_requests::seen::SeenPathRequestColumns;
use crate::routing::request_handlers::RequestHandlerColumns;
use crate::routing::reverse_routes::ReverseRouteColumns;
use crate::routing::routes::RouteColumns;
use crate::routing::tunnel::registry::TunnelColumns;
use crate::routing::upstream_app_destinations::UpstreamAppDestinationColumns;
use crate::routing::warmth::DepartedInterfaceColumns;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnsFull;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCapacity {
    Fixed(usize),
    Dynamic,
}

/// The sizing story a status face renders; enforcement lives in the columns themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayedStorageLimits {
    pub tracked_destinations: StorageCapacity,
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
    pub reverse_routes: StorageCapacity,
    pub pending_path_requests: StorageCapacity,
    pub held_announces: StorageCapacity,
    pub ratchets_per_destination: StorageCapacity,
}

impl DisplayedStorageLimits {
    pub const DYNAMIC: Self = Self {
        tracked_destinations: StorageCapacity::Dynamic,
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
        reverse_routes: StorageCapacity::Dynamic,
        pending_path_requests: StorageCapacity::Dynamic,
        held_announces: StorageCapacity::Dynamic,
        ratchets_per_destination: StorageCapacity::Dynamic,
    };
}

pub trait StorageLayout {
    const LIMITS: DisplayedStorageLimits;

    type Routes: RouteColumns + Default;
    type Announces: AnnounceRecordColumns + Default;
    type History: AnnounceIdHistory + Default;
    type AppData: AnnounceAppData + Default;
    type ScheduledAnnounces: ScheduledAnnounceQueue + Default;
    type UpstreamAppDestinations: UpstreamAppDestinationColumns + Default;
    type HeldIdentities: HeldIdentityColumns + Default;
    type SelfRatchets: SelfRatchetColumns + Default;
    type Receipts: ReceiptColumns + Default;
    type PacketHashes: PacketHashHistory + Default;
    type ReverseRoutes: ReverseRouteColumns + Default;
    type PendingPathRequests: PendingPathRequestColumns + Default;
    type RecentPathRequests: RecentPathRequestColumns + Default;
    type SeenPathRequests: SeenPathRequestColumns + Default;
    type Tunnels: TunnelColumns + Default;
    type DepartedInterfaces: DepartedInterfaceColumns + Default;
    type RecursivePathRequests: RecursivePathRequestColumns + Default;
    type InterfacePathRequestLimits: InterfacePathRequestLimitColumns + Default;
    type InterfaceAnnounceLimits: InterfaceAnnounceLimitColumns + Default;
    type HeldAnnounces: HeldStore + Default;
    type HeldAnnounceAppData: AnnounceAppData + Default;
    type DestinationAnnounceLimits: DestinationAnnounceLimitColumns + Default;
    type GroupKeys: GroupKeyColumns + Default;
    type RequestHandlers: RequestHandlerColumns + Default;
    type TransportedLinks: TransportedLinkColumns + Default;
    type Links: LinkColumns + Default;
    type OutgoingResources: ResourceColumns<OutgoingResourceState> + Default;
    type IncomingResources: ResourceColumns<IncomingResourceState> + Default;
    type IncomingAssemblies: IncomingAssemblyColumns + Default;
    type OutgoingAssemblies: OutgoingAssemblyColumns + Default;
    type Channels: ChannelColumns + Default;
    type DirtyInterfaces: DirtyInterfaceSet + Default;
}
