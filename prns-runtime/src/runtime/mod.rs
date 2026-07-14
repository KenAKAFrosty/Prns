mod command;
mod event;
mod health;
#[cfg(feature = "std")]
mod identity_bootstrap;
#[cfg(feature = "runtime-metrics")]
mod metrics;
mod recipe;
pub mod request_router;
#[cfg(feature = "tracing")]
mod tracing_events;

pub use command::{PrnsApi, SendError};
pub use event::{Diagnostic, Message, PrnsEvent};
pub use health::RuntimeHealth;
#[cfg(feature = "std")]
pub use identity_bootstrap::{
    ephemeral_ble_identity, generate_identity_secret, load_or_create_identity_secret,
    IdentitySecretFileError,
};
#[cfg(feature = "runtime-metrics")]
pub use metrics::{CryptoMetricsSnapshot, EgressMetricsSnapshot, RuntimeMetricsSnapshot};
pub use recipe::{Manual, PreConfiguredDestination, PrnsRecipe};

#[cfg(feature = "tokio-host")]
mod tokio_bind;
#[cfg(feature = "tokio-host")]
pub use crate::reactor::impls::tokio_reactor::{
    CryptoPoolConfig, PersistedStateSnapshot, PoolWorkers,
};
#[cfg(feature = "tokio-host")]
pub use tokio_bind::{
    boot_timeline_origin, AttachIntent, Attachable, AttachedInterface, AttachedSupervisor,
    DetachedFleet, Fleet, FlushError, FlushMark, FlushReport, InterfaceIfacStatus,
    InterfaceSupervisor, Prns, RatchetSeedReport, RegionFlush, ResourceReceipt,
    ResourceReceiveError, ResourceSendError, RouteSeedReport, SegmentCompression, TokioPrnsHandle,
    TunnelSeedReport,
};

#[cfg(feature = "tokio-host")]
mod byte_stream;
#[cfg(feature = "tokio-host")]
pub use byte_stream::{ByteStreamReader, ByteStreamWriter, StreamId};
#[cfg(feature = "tokio-host")]
mod interface_store;
#[cfg(feature = "tokio-host")]
pub use interface_store::{InterfaceStore, Subscription};

#[cfg(feature = "tokio-host")]
mod tokio_runner;

#[cfg(feature = "embassy-host")]
mod embassy_interface_store;
#[cfg(feature = "embassy-host")]
pub use embassy_interface_store::{EmbassyInterfaceStore, InterfaceCountSink};
#[cfg(feature = "embassy-host")]
mod embassy_bind;
#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use embassy_bind::Fleet;
#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use embassy_bind::Prns;
#[cfg(feature = "embassy-host")]
pub use embassy_bind::{
    CompletionPool, EmbassyPrnsHandle, Fleet as EmbassyFleet, MemberWire, ReactorPlumbing,
};
