//! The runtime over the passive engine.
//!
//! - `core` holds the substrate-neutral [`Runtime`] — it owns the engine and its
//!   registered workers as siblings, turns one drive cycle into intake → step →
//!   exhaust (touching no clock, RNG, sockets, or `.await`), and exposes the
//!   single generic [`run`] loop over a [`Host`](super::Host).
//! - `snapshot` holds the app-facing view ([`RuntimeSnapshot`]) the runtime
//!   surfaces each cycle — the one seam an app reads engine state through.
//! - [`impls`] holds the per-platform inbound-mailbox + snapshot-channel types a
//!   host wires its workers through (mpsc on std, an embassy `Channel`/`Watch` on
//!   embedded). Each is gated by its platform feature; the neutral core carries
//!   no gate.

mod core;
mod snapshot;
pub use core::{run, Runtime};
pub use snapshot::{InterfaceView, RuntimeSnapshot};

#[cfg(any(feature = "embassy-host", feature = "std-host"))]
pub mod impls;
