mod command;
mod event;
mod health;
mod identity_blackhole;
pub mod node;
pub mod node_introspection;
pub mod packet_phy_retention;
pub mod request_router;
#[cfg(feature = "rns-management")]
pub mod rns_management;
#[cfg(feature = "rns-management")]
pub mod rns_remote_management;
#[cfg(feature = "rns-management")]
pub mod rns_rpc;
#[cfg(feature = "rnx")]
pub mod rnx;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        pub mod persistence_snapshots;

        pub use persistence_snapshots::{
            PersistedStateSnapshot, SelfRatchetSnapshot, SelfRatchetsSnapshot,
        };
    }
}

pub use crate::engine::BlackholeSeedReport;
pub use command::{
    ClearAnnounceQueuesOutcome, DestinationIdentityRetentionControl,
    DestinationIdentityRetentionControlError, DropRouteOutcome, DropRoutesViaOutcome, PrnsNodeApi,
    RoutingControl, RoutingControlError, SendError,
};
pub use event::{Diagnostic, Message, PrnsEvent};
pub use health::RuntimeHealth;
pub use identity_blackhole::{
    IdentityBlackholeControl, IdentityBlackholeControlError, IdentityBlackholeSource,
    IdentityBlackholeSourceError,
};
pub use node::{
    assemble_node, configure_preconfigured_destination, AssembledNode,
    ConfigurePreconfiguredDestinationError, Manual, PreConfiguredDestination, PrnsNodeRecipe,
    RequestHandlerRegistration,
};

#[doc(hidden)]
pub mod placement {
    pub use super::node::assemble_node_in_place;
}

cfg_if::cfg_if! {
    if #[cfg(feature = "runtime-metrics")] {
        mod metrics;
        mod observability;

        pub use metrics::{
            AnnounceEgressCounts, AnnounceEgressMetricsSnapshot, AnnounceEgressOutcome,
            AnnounceOriginCounts, CryptoMetricsSnapshot, EgressInterfaceKindCounts,
            EgressMetricsSnapshot, InterfaceAnnounceEgressMetricsSnapshot, RuntimeMetricsSnapshot,
        };
        pub use observability::{
            ReliabilityMetricsSnapshot, RuntimeLinkClosure, RuntimeLinkClosureCounts,
            RuntimeOperation, RuntimeOperationCounts, RuntimeOperationOutcome,
            RuntimeResourceFailure, RuntimeResourceFailureCounts, RuntimeRouteRemoval,
            RuntimeRouteRemovalCounts,
        };
    }
}
