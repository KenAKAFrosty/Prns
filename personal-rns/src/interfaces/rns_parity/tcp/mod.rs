//! A TCP interface built fresh against the reactor's
//! [`InterfaceSeam`](crate::reactor::interface_seam): RNS `TCPClientInterface` /
//! `TCPServerInterface` parity — the same RNS HDLC-style framing serial speaks, over a TCP
//! stream with the reference's socket discipline (`TCP_NODELAY`, keepalive probes). The
//! sizing brain is the host-agnostic [`core`]; the initiating and listening ends live under
//! [`client`] and [`server`], split by role the way the reference's `TCPClientInterface` and
//! `TCPServerInterface` are — and sharing the [`core`] framing and (on tokio) the
//! [`tokio_socket`] discipline both ends apply.

pub mod client;
pub mod core;
pub mod server;

#[cfg(feature = "tcp")]
pub mod tokio_socket;
