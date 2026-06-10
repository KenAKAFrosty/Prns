use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::Announce;
use crate::routing::storage::AnnounceIdHistoryView;
use crate::wire::TransportId;

/// RNS 1.3.1 `path_table` `received_from` (Transport.py:1714/1739).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextHop {
    Direct,
    Via(TransportId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForwardingRoute {
    pub hops: u8,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
}

/// RNS 1.3.1 `path_is_unresponsive`
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
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteRemovalCause {
    Expired,
    Evicted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemovedRoute {
    pub destination: crate::wire::DestinationHash,
    pub cause: RouteRemovalCause,
}
