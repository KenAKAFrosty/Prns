//! The interface contract — every interface is an autonomous mini-program.
//!
//! An [`InterfaceWorker`] owns its byte I/O, peers, fan-out, and discovery
//! opaquely and runs on its own (thread, task, or — via [`RuntimeDriven`] —
//! cooperative polling). It meets the system through a narrow seam: the runtime
//! hands it packets to send ([`InterfaceWorker::submit`]) and drains the inbound
//! packets it stamps into the shared mailbox; nothing below the interface (peer
//! addresses, sockets, sockets' platform) ever surfaces. [`InterfaceStats`] is
//! the worker's self-reported meta surface. Medium-capability refinements like
//! [`TrackedPeerMulticastInterface`] extend this contract for interfaces that
//! can honestly expose more.

mod core;
mod multicast;
mod runtime_driven;
mod stats;

pub use core::{InterfaceWorker, QueueFull};
pub use multicast::TrackedPeerMulticastInterface;
pub use runtime_driven::RuntimeDriven;
pub use stats::{InterfaceStats, LinkState};
