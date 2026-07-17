mod command;
mod destination_identity_retention;
mod event;
mod health;
mod identity_blackhole;
pub mod node;
pub mod packet_phy_retention;
pub mod request_router;

pub use command::{
    ClearAnnounceQueuesOutcome, DestinationIdentityRetentionControl,
    DestinationIdentityRetentionControlError, DropRouteOutcome, DropRoutesViaOutcome, PrnsApi,
    RoutingControl, RoutingControlError, SendError,
};
pub use event::{Diagnostic, Message, PrnsEvent};
pub use health::RuntimeHealth;
pub use identity_blackhole::{
    IdentityBlackholeControl, IdentityBlackholeControlError, IdentityBlackholeSource,
    IdentityBlackholeSourceError,
};
pub use node::{assemble_node, AssembledNode, Manual, PreConfiguredDestination, PrnsRecipe};

cfg_if::cfg_if! {
    if #[cfg(feature = "std")] {
        mod identity_bootstrap;

        pub use identity_bootstrap::{
            ephemeral_ble_identity, generate_identity_secret, load_or_create_identity_secret,
            IdentitySecretFileError,
        };
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "tokio-host")] {
        mod byte_stream;
        mod interface_store;
        mod route_restore;
        mod tokio_bind;
        mod tokio_runner;

        pub use crate::reactor::impls::tokio_reactor::{
            CryptoPoolConfig, PersistedStateSnapshot, PoolWorkers, SelfRatchetSnapshot,
            SelfRatchetsSnapshot,
        };
        pub use byte_stream::{ByteStreamReader, ByteStreamWriter, StreamId};
        pub use interface_store::{InterfaceStore, Subscription};
        pub use tokio_bind::{
            boot_timeline_origin, AttachIntent, Attachable, AttachedInterface, AttachedSupervisor,
            BlackholeSeedReport, DetachedFleet, FlushError, FlushMark, FlushReport,
            InterfaceAttachmentMetadata, InterfaceSupervisor, DestinationIdentitySeedReport,
            NonRoutingIdentityError, PrepareFlushError, PreparedFlush, RatchetSeedReport,
            RegionFlush, ResourceReceipt,
            ResourceReceiveError, ResourceSendError, RouteSeedProgress, RouteSeedReport,
            SegmentCompression, SharedInstanceIdentityError, TokioPrnsHandle, TunnelSeedReport,
        };
        pub(crate) use destination_identity_retention::{
            apply_destination_identity_retention_command, settle_destination_identity_retention,
            DestinationIdentityRetentionHostCommand,
        };
        pub(crate) use identity_blackhole::{
            apply_identity_blackhole_command, IdentityBlackholeHostCommand,
        };
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "tokio-host")] {
        pub use tokio_bind::{Fleet, Prns};
    }
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

cfg_if::cfg_if! {
    if #[cfg(feature = "tracing")] {
        mod tracing_events;
    }
}
