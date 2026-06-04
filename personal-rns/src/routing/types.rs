use crate::engine::InstantMillis;
use crate::routing::announce::Announce;
use crate::routing::storage::AnnounceIdHistoryView;

/// Whether a learned route is currently answering direct traffic. RNS tracks
/// this as a boolean `path_is_unresponsive`; modelled as a two-state type so
/// the predicate reads as intent rather than a bare flag
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteResponsiveness {
    Responsive,
    Unresponsive,
}

#[derive(Debug, Clone, Copy)]
pub struct ExistingRoute<'a> {
    pub hops: u8,
    pub expires: InstantMillis,
    pub announce_id_history: AnnounceIdHistoryView<'a>,
    pub responsiveness: RouteResponsiveness,
}

#[derive(Debug, Clone)]
pub struct RetainedAnnounce<'a> {
    pub hops: u8,
    pub announce: Announce<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropCause {
    RoutingTableFull,
    PayloadArenaFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertRouteOutcome {
    Inserted,
    Updated,
    Dropped(DropCause),
}
