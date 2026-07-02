//! A TCP interface built fresh against the reactor's
//! [`InterfaceSeam`](crate::reactor::interface_seam): RNS `TCPClientInterface` /
//! `TCPServerInterface` parity — the same RNS HDLC-style framing serial speaks, over a TCP
//! stream with the reference's socket discipline (`TCP_NODELAY`, keepalive probes). The
//! sizing brain is the host-agnostic [`core`]; the initiating and listening ends live in the
//! runtime interface crates (`prns-interfaces-tokio`'s `tcp::client` / `tcp::server` plus its
//! shared `tokio_socket` discipline, `prns-interfaces-embassy`'s `tcp::client`), split by role
//! the way the reference's `TCPClientInterface` and `TCPServerInterface` are.

pub mod core;
