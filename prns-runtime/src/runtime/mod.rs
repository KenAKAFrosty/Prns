mod command;
mod event;
mod health;
mod identity_blackhole;
mod known_destination_retention;
mod recipe;
pub mod request_router;

pub use command::{
    ClearAnnounceQueuesOutcome, DropRouteOutcome, DropRoutesViaOutcome,
    KnownDestinationRetentionControl, KnownDestinationRetentionControlError, PrnsApi,
    RoutingControl, RoutingControlError, SendError,
};
pub use event::{Diagnostic, Message, PrnsEvent};
pub use health::RuntimeHealth;
#[cfg(feature = "tokio-host")]
pub(crate) use identity_blackhole::{
    apply_identity_blackhole_command, IdentityBlackholeHostCommand,
};
pub use identity_blackhole::{
    IdentityBlackholeControl, IdentityBlackholeControlError, IdentityBlackholeSource,
    IdentityBlackholeSourceError,
};
#[cfg(feature = "tokio-host")]
pub(crate) use known_destination_retention::{
    apply_known_destination_retention_command, settle_known_destination_retention,
    KnownDestinationRetentionHostCommand,
};
pub use recipe::{Manual, PreConfiguredDestination, PrnsRecipe};

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
    if #[cfg(any(feature = "tokio-host", feature = "embassy-host", test))] {
        mod packet_phy_retention;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "tokio-host")] {
        mod byte_stream;
        mod interface_store;
        mod tokio_bind;
        mod tokio_runner;

        pub use crate::reactor::impls::tokio_reactor::{
            CryptoPoolConfig, PersistedStateSnapshot, PoolWorkers,
        };
        pub use byte_stream::{ByteStreamReader, ByteStreamWriter, StreamId};
        pub use interface_store::{InterfaceStore, Subscription};
        pub use tokio_bind::{
            boot_timeline_origin, AttachIntent, Attachable, AttachedInterface, AttachedSupervisor,
            BlackholeSeedReport, DetachedFleet, FlushError, FlushMark, FlushReport,
            InterfaceSupervisor, KnownDestinationSeedReport, RatchetSeedReport, RegionFlush,
            ResourceReceipt, ResourceReceiveError, ResourceSendError, RouteSeedProgress,
            RouteSeedReport, SegmentCompression, TokioPrnsHandle, TunnelSeedReport,
        };
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "embassy-host")] {
        mod embassy_bind;
        mod embassy_interface_store;

        pub use embassy_bind::{
            CompletionPool, EmbassyPrnsHandle, Fleet as EmbassyFleet, MemberWire, ReactorPlumbing,
        };
        pub use embassy_interface_store::EmbassyInterfaceStore;
        pub(crate) use embassy_interface_store::{
            InterfaceInspectionStore, NoInterfaceInspectionStore,
        };
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "tokio-host")] {
        pub use tokio_bind::{Fleet, Prns};
    } else if #[cfg(feature = "embassy-host")] {
        pub use embassy_bind::{Fleet, Prns};
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
