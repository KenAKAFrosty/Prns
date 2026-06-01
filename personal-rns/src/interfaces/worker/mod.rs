//! The interface-worker contract — the sibling to [`Interface`](super::Interface)
//! for interfaces that are autonomous mini-programs.
//!
//! Where an `Interface` exposes inline byte I/O the engine drives, an
//! [`InterfaceWorker`] owns its byte I/O, peers, fan-out, and discovery
//! opaquely and runs on its own (thread, task, or — via [`RuntimeDriven`] —
//! cooperative polling). It meets the system through a narrow seam: the runtime
//! hands it packets to send ([`InterfaceWorker::submit`]) and drains the inbound
//! packets it stamps into the shared mailbox; nothing below the interface (peer
//! addresses, sockets, sockets' platform) ever surfaces. [`InterfaceStats`] is
//! the worker's self-reported meta surface.

mod core;
mod runtime_driven;
mod stats;

pub use core::{InterfaceWorker, QueueFull};
pub use runtime_driven::RuntimeDriven;
pub use stats::{InterfaceStats, LinkState};
