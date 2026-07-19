mod learning;
mod lifetime;
mod lookup;
mod model;
mod persistence;
mod removal;
mod updates;

pub use learning::{DropCause, UpsertRouteOutcome};
pub use model::{ExistingRoute, ForwardingRoute, RoutingTable, StoredAnnounce};
pub use persistence::{AnnounceIdRing, PersistedRouteRow, SeedRouteOutcome};
pub use removal::{RemovedRoute, RouteRemovalCause};

#[cfg(test)]
mod tests;
