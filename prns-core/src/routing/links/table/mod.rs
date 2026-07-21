//! The per-link state a node holds from LINKREQUEST to ACTIVE (RNS 1.4.0 `Link.status`):
//! - the initiator's pending establishments
//! - the responder's handshakes awaiting an RTT, and
//! - the active sessions both settle into.

pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
