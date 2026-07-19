use crate::routing::announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecordTable};
use crate::routing::route_expiry::{LinearRouteExpiryIndex, RouteExpiryIndex};
use crate::routing::routes::RouteTable;

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
