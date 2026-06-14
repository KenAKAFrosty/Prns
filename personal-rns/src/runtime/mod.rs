//! The high-level Prns runtime: declare a [`Recipe`] (identity, transport role, starting
//! destinations, and a platform [`Bind`]) and `Prns::run` stands the node up on the reactor.
//! This is the consumer-facing layer every app sits on — Hopspot, the benchmarks, future apps —
//! revived on top of the current reactor + `StorageLayout`. The platform-neutral assembly lives
//! in `prns.rs`; the one place embassy and tokio diverge is the [`Bind`] seam.

mod bind;
mod event;
mod prns;
mod recipe;
mod request_router;

pub use bind::Bind;
pub use event::{Diagnostic, Message, PrnsEvent};
pub use prns::Prns;
pub use recipe::{Recipe, StartingDestination};
pub use request_router::{
    InboundRequest, RequestCx, RequestRoute, Responder, Response, ResponseSink, RoutePolicy,
    RouteSet, Router,
};

#[cfg(feature = "tokio-host")]
mod tokio_bind;
#[cfg(feature = "tokio-host")]
pub use tokio_bind::{TokioBind, TokioCommands};

#[cfg(feature = "tokio-host")]
mod tokio_runner;

#[cfg(feature = "embassy-contract")]
mod embassy_bind;
#[cfg(feature = "embassy-contract")]
pub use embassy_bind::EmbassyBind;
