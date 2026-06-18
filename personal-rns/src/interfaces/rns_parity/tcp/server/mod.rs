//! The RNS `TCPServerInterface`: the listening end of a TCP pair — bind a port and serve the
//! peers that connect. The reference spawns a child interface per accepted client; the tokio body
//! here is point-to-point for now (one peer at a time, the next waits in the accept backlog), with
//! the multi-client fleet supervisor the next step. Shares the parent's framing [`core`](super::core)
//! and (on tokio) the [`tokio_socket`](super::tokio_socket) discipline.

#[cfg(feature = "tcp")]
pub mod tokio;
