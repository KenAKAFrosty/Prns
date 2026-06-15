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
use crate::routing::links::channel::columns::ChannelColumns;
use crate::routing::links::resources::table::{
    IncomingResourceState, OutgoingResourceState, ResourceColumns,
};
use crate::routing::links::table::LinkColumns;
use crate::routing::links::transported::TransportedLinkColumns;
use crate::routing::path_requests::pending::PendingPathRequestColumns;
use crate::routing::path_requests::recent::RecentPathRequestColumns;
use crate::routing::path_requests::seen::SeenPathRequestColumns;
use crate::routing::request_handlers::RequestHandlerColumns;
use crate::routing::reverse_routes::ReverseRouteColumns;
use crate::routing::routes::RouteColumns;
use crate::routing::upstream_app_destinations::UpstreamAppDestinationColumns;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnsFull;

/// A storage recipe: the bundle of column backends an engine runs on. A
/// prepackage (`Esp32S3`/`Esp32C6`/`Nrf52840` for no_std boards, `GrowableHeap`
/// for a std host) picks every backend at once; a target with other needs
/// hand-writes its own impl. The `Default` bounds let the engine build an empty
/// bundle.
pub trait StorageLayout {
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
    type RecentPathRequests: RecentPathRequestColumns + Default;
    type SeenPathRequests: SeenPathRequestColumns + Default;
    type AnnounceRates: AnnounceRateColumns + Default;
    type GroupKeys: GroupKeyColumns + Default;
    type RequestHandlers: RequestHandlerColumns + Default;
    type TransportedLinks: TransportedLinkColumns + Default;
    type Links: LinkColumns + Default;
    type OutgoingResources: ResourceColumns<OutgoingResourceState> + Default;
    type IncomingResources: ResourceColumns<IncomingResourceState> + Default;
    type Channels: ChannelColumns + Default;
}
