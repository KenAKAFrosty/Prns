mod impls;

pub use impls::*;

use crate::crypto::ratchets::SelfRatchetColumns;
use crate::identity::held::HeldIdentityColumns;
use crate::routing::announce::rate_limit::AnnounceRateColumns;
use crate::routing::announce::retained::{
    AnnounceIdHistory, RetainedAnnounceColumns, RetainedAppData,
};
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::dedup::PacketHashHistory;
use crate::routing::delivery::receipts::ReceiptColumns;
use crate::routing::group_keys::GroupKeyColumns;
use crate::routing::links::resources::table::{
    IncomingResourceState, OutgoingResourceState, ResourceColumns,
};
use crate::routing::links::table::LinkColumns;
use crate::routing::links::transported::TransportedLinkColumns;
use crate::routing::path_requests::pending::PendingPathRequestColumns;
use crate::routing::path_requests::seen::SeenPathRequestColumns;
use crate::routing::request_handlers::RequestHandlerColumns;
use crate::routing::reverse_routes::ReverseRouteColumns;
use crate::routing::routes::RouteColumns;
use crate::routing::upstream_app_destinations::UpstreamAppDestinationColumns;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnsFull;

pub trait StorageCapacities {
    const MAX_TRACKED_DESTINATIONS: usize;
    const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize;
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize;
    const HISTORY_FLOOR_PER_DESTINATION: usize;
    const HISTORY_OVERFLOW_CAPACITY: usize;
    const MAX_UPSTREAM_APP_DESTINATIONS: usize;
    const MAX_HELD_IDENTITIES: usize;
    const PACKET_HASH_GENERATION_CAPACITY: usize;
    const RETAINED_RATCHETS_PER_DESTINATION: usize;
    const MAX_OUTSTANDING_RECEIPTS: usize;
    const MAX_REVERSE_ROUTES: usize;
    const MAX_PENDING_PATH_REQUESTS: usize;
    const MAX_SEEN_PATH_REQUESTS: usize;
    const MAX_LINKS: usize;
}

/// A storage recipe: the bundle of column backends an engine runs on. One type
/// (`FixedInline` for no_std, `GrowableHeap` for a std host) picks every backend
/// at once; the `Default` bounds let the engine build an empty bundle.
pub trait EngineStorage {
    type Routes: RouteColumns + Default;
    type Announces: RetainedAnnounceColumns + Default;
    type History: AnnounceIdHistory + Default;
    type AppData: RetainedAppData + Default;
    type ScheduledAnnounces: ScheduledAnnounceQueue + Default;
    type UpstreamAppDestinations: UpstreamAppDestinationColumns + Default;
    type HeldIdentities: HeldIdentityColumns + Default;
    type SelfRatchets: SelfRatchetColumns + Default;
    type Receipts: ReceiptColumns + Default;
    type PacketHashes: PacketHashHistory + Default;
    type ReverseRoutes: ReverseRouteColumns + Default;
    type PendingPathRequests: PendingPathRequestColumns + Default;
    type SeenPathRequests: SeenPathRequestColumns + Default;
    type AnnounceRates: AnnounceRateColumns + Default;
    type GroupKeys: GroupKeyColumns + Default;
    type RequestHandlers: RequestHandlerColumns + Default;
    type TransportedLinks: TransportedLinkColumns + Default;
    type Links: LinkColumns + Default;
    type OutgoingResources: ResourceColumns<OutgoingResourceState> + Default;
    type IncomingResources: ResourceColumns<IncomingResourceState> + Default;
}
