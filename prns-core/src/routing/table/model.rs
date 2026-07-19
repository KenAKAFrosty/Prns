use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecordTable};
use crate::routing::announce::{Announce, AnnounceId};
use crate::routing::route_expiry::{LinearRouteExpiryIndex, RouteExpiryIndex};
use crate::routing::routes::{NextHop, RouteResponsiveness, RouteTable};
use crate::units::HopCount;

/// RNS 1.3.5's `path_table`
///
/// NOTE: `PartialEq` compares backend representation byte-for-byte because the determinism tests rely on that. Do not use `==` and expect to compare the same set of routes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoutingTable<R, A, H, D, I = LinearRouteExpiryIndex>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
    I: RouteExpiryIndex,
{
    pub(super) routes: R,
    pub(super) route_expiries: I,
    pub(super) announce_records: A,
    pub(super) announce_id_history: H,
    pub(super) announce_app_data: D,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForwardingRoute {
    pub hops: HopCount,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
}

#[derive(Debug, Clone, Copy)]
pub struct ExistingRoute<'a> {
    pub hops: HopCount,
    pub expires_at: InstantMillis,
    pub announce_id_history: &'a [AnnounceId],
    pub responsiveness: RouteResponsiveness,
}

#[derive(Debug, Clone)]
pub struct StoredAnnounce<'a> {
    pub hops: u8,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
    pub announce: Announce<'a>,
}
