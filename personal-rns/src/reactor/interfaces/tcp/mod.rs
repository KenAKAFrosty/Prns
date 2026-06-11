//! A TCP interface built fresh against the reactor's
//! [`InterfaceSeam`](crate::reactor::interface_seam): RNS `TCPClientInterface` /
//! `TCPServerInterface` parity — the same RNS HDLC-style framing serial speaks, over a TCP
//! stream with the reference's socket discipline (`TCP_NODELAY`, keepalive probes). The
//! sizing brain is the host-agnostic [`core`]; the tokio body lives under [`impls`].

pub mod core;
pub mod impls;
