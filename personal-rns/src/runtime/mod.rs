mod command;
mod event;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
mod interface_set;
mod recipe;
pub mod request_router;

pub use command::{Commands, SendError};
pub use event::{Diagnostic, Message, PrnsEvent};
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub use interface_set::{InterfaceAttach, InterfaceSet};
pub use recipe::{PreConfiguredDestination, PrnsRecipe};

#[cfg(feature = "tokio-host")]
mod tokio_bind;
#[cfg(feature = "tokio-host")]
pub use tokio_bind::{
    AttachedInterface, AttachedSupervisor, Fleet, InterfaceSupervisor, Prns, PrnsHandle,
};

#[cfg(feature = "tokio-host")]
mod byte_stream;
#[cfg(feature = "tokio-host")]
pub use byte_stream::{ByteStreamReader, ByteStreamWriter, StreamId};

#[cfg(feature = "tokio-host")]
mod tokio_runner;

#[cfg(feature = "embassy-contract")]
mod embassy_bind;
#[cfg(all(feature = "embassy-contract", not(feature = "tokio-host")))]
pub use embassy_bind::Prns;
#[cfg(feature = "embassy-contract")]
pub use embassy_bind::{CompletionPool, EmbassyCommands, Fleet, MemberWire, ReactorPlumbing};
