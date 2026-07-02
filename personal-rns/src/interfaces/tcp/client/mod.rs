//! The RNS `TCPClientInterface`: the initiating end of a TCP pair — dial a fixed Reticulum TCP node
//! and serve the one connection, reconnecting when it drops. Point-to-point: one engine interface,
//! one peer. The host bodies share the parent's framing [`core`](super::core) and (on tokio) the
//! [`tokio_socket`](super::tokio_socket) discipline.

#[cfg(feature = "tcp")]
pub mod tokio;

#[cfg(feature = "embassy-wifi")]
pub mod embassy;
