mod command;
mod event;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
mod interface_set;
mod recipe;
pub mod request_router;

pub use command::{PrnsApi, SendError};
pub use event::{Diagnostic, Message, PrnsEvent};
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub use interface_set::{InterfaceAttach, InterfaceSet};
pub use recipe::{PreConfiguredDestination, PrnsRecipe};

#[cfg(feature = "tokio-host")]
mod tokio_bind;
#[cfg(feature = "tokio-host")]
pub use crate::reactor::impls::tokio_reactor::{CryptoPoolConfig, PoolWorkers};
#[cfg(all(test, feature = "tokio-host"))]
pub(crate) use tokio_bind::FleetTestGuard;
#[cfg(feature = "tokio-host")]
pub use tokio_bind::{
    AttachedInterface, AttachedSupervisor, Fleet, InterfaceSupervisor, Prns, ResourceReceipt,
    ResourceReceiveError, ResourceSendError, TokioPrnsHandle,
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

#[cfg(feature = "local")]
mod local_instance;
#[cfg(feature = "local")]
pub use local_instance::{InstancePorts, JoinError, LocalInstance, OnExisting, Role};

#[cfg(feature = "embassy-contract")]
mod embassy_interface_store;
#[cfg(feature = "embassy-contract")]
pub use embassy_interface_store::{EmbassyInterfaceStore, InterfaceCountSink};
#[cfg(feature = "embassy-contract")]
mod embassy_bind;
#[cfg(all(feature = "embassy-contract", not(feature = "tokio-host")))]
pub use embassy_bind::Fleet;
#[cfg(all(feature = "embassy-contract", not(feature = "tokio-host")))]
pub use embassy_bind::Prns;
#[cfg(feature = "embassy-contract")]
pub use embassy_bind::{
    CompletionPool, EmbassyPrnsHandle, Fleet as EmbassyFleet, MemberWire, ReactorPlumbing,
};
