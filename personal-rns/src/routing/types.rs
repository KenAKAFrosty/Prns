//! Small data shapes the routing table hands the engine and the predicate.

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

/// The fields of an existing route used only for the acceptance predicate,
/// gathered from the table's columns on a lookup hit. The history is a
/// borrowed two-slice view ([`AnnounceIdHistoryView`]), so the predicate reads it in place.
#[derive(Debug, Clone, Copy)]
pub struct ExistingRoute<'a> {
    pub hops: u8,
    pub expires: InstantMillis,
    pub announce_id_history: AnnounceIdHistoryView<'a>,
    pub responsiveness: RouteResponsiveness,
}

/// What rebroadcasting a known destination's retained announce needs,
/// gathered from the routing table on a hit: the hop count to emit plus the
/// structured announce itself. Re-emission serializes it back to wire via
/// [`Announce::to_wire`], reproducing the original payload byte-identically
/// so the retained signature still validates.
///
/// `transport_id` (the upstream we got the announce from) lands here too once
/// the first interface arrives — RNS's `announce_table` carries `received_from`
/// for the same reason: emission needs to know which neighbour NOT to gossip
/// back to.
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
