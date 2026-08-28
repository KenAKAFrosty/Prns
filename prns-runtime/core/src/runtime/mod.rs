mod command;
mod event;
mod health;
mod identity_blackhole;
pub mod node;
pub mod node_introspection;
pub mod packet_phy_retention;
mod remote_control;
mod remote_control_access;
pub mod request_endpoints;
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
    AnnounceNowError, ClearAnnounceQueuesOutcome, DestinationIdentityRetentionControl,
    DestinationIdentityRetentionControlError, DropRouteOutcome, DropRoutesViaOutcome, PrnsNodeApi,
    RoutingControl, RoutingControlError, SendError, SetRegisteredAnnounceAppDataError,
};
pub use event::{Diagnostic, Message, PrnsEvent};
pub use health::RuntimeHealth;
pub use identity_blackhole::{
    IdentityBlackholeControl, IdentityBlackholeControlError, IdentityBlackholeSource,
    IdentityBlackholeSourceError,
};
pub use node::{
    assemble_node, configure_preconfigured_destination, configure_remote_control_service,
    AssembledNode, AssembledRemoteControl, ConfigurePreconfiguredDestinationError,
    ConfigureRemoteControlServiceError, ManuallyAttached, NoPersistence, PreConfiguredDestination,
    PrnsNodeRecipe, ServeMyRequestEndpoints,
};
pub use remote_control::{
    RemoteControlAnnounceSelf, RemoteControlAnnounceSelfFailure, RemoteControlDescribe,
    RemoteControlError,
};
pub use remote_control_access::{
    RemoteControlAccessControl, RevokeRemoteControlControllerControlError,
    SetRemoteControlControllerGrantControlError,
};

#[doc(hidden)]
pub mod placement {
    pub use super::node::assemble_node_in_place;
    pub use super::remote_control::{
        admit_remote_control_request, dispatch_admitted_remote_control_request,
        dispatch_remote_control_request, AdmittedRemoteControlRequest,
    };
}

cfg_if::cfg_if! {
    if #[cfg(feature = "runtime-metrics")] {
        mod metrics;
        mod observability;

        pub use metrics::{
            AnnounceBackpressureCounts, AnnounceBackpressureEvent, AnnounceEgressCounts,
            AnnounceEgressMetricsSnapshot, AnnounceEgressOutcome, AnnounceOriginCounts,
            CryptoMetricsSnapshot, EgressInterfaceKindCounts, EgressLaneMetricsSnapshot,
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
