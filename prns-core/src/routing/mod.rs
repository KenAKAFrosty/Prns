pub mod announce;
pub mod blackhole;
pub mod dedup;
pub mod delivery;
pub mod group_keys;
pub mod ingress;
pub mod links;
pub mod path_requests;
pub mod proof;
pub mod request_handlers;
pub mod reverse_routes;
pub mod route_expiry;
pub mod routes;
pub mod table;
pub mod timing;
pub mod tunnel;
pub mod upstream_app_destinations;
pub mod warmth;

cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        mod temporal_index;

        pub use route_expiry::RoaringRouteExpiryIndex;
    }
}

pub use announce::AnnounceArrival;
pub use blackhole::{
    BlackholeExpiry, BlackholeIdentityOutcome, BlackholeInsertFailure, BlackholedIdentity,
    UnblackholeIdentityOutcome,
};
pub use route_expiry::{LinearRouteExpiryIndex, RouteExpiryIndex, ROUTE_EXPIRY_QUANTUM_MS};
pub use routes::{NextHop, RouteResponsiveness};
pub use table::{
    AnnounceIdRing, DropCause, ExistingRoute, ForwardingRoute, PersistedRouteRow, RemovedRoute,
    RouteRemovalCause, RoutingTable, SeedRouteOutcome, StoredAnnounce, UpsertRouteOutcome,
};
pub use upstream_app_destinations::{
    LinkRequestPolicy, ProofStrategy, RegisterDestinationError, UpstreamAppDestination,
    UpstreamAppDestinationKind, UpstreamAppDestinationTable,
};
