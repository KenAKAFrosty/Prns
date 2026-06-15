mod bind;
mod command;
mod event;
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
mod interface_set;
mod prns;
mod recipe;
pub mod request_router;

pub use bind::Bind;
pub use command::{Commands, SendError};
pub use event::{Diagnostic, Message, PrnsEvent};
#[cfg(any(feature = "tokio-host", feature = "embassy-contract"))]
pub use interface_set::{InterfaceAttach, InterfaceSet};
pub use prns::Prns;
pub use recipe::{Recipe, StartingDestination};

#[cfg(feature = "tokio-host")]
mod tokio_bind;
#[cfg(feature = "tokio-host")]
pub use tokio_bind::{TokioBind, TokioCommands};

#[cfg(feature = "tokio-host")]
mod tokio_runner;

#[cfg(feature = "embassy-contract")]
mod embassy_bind;
#[cfg(feature = "embassy-contract")]
pub use embassy_bind::{CompletionPool, EmbassyBind, EmbassyCommands};

#[cfg(feature = "embassy-contract")]
mod embassy_runner;
