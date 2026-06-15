//! A UDP interface built fresh against the reactor's
//! [`InterfaceSeam`](crate::reactor::interface_seam): RNS `UDPInterface` parity — one raw
//! wire packet per datagram, no framing at all, and fixed peer addressing (the reference
//! forwards every datagram to a configured `ip:port`, never to an arriving datagram's
//! source). Connectionless: no accept, no reconnect, no socket discipline — bound is up.
//! The sizing brain is the host-agnostic [`core`]; the tokio body lives under [`impls`].

pub mod core;
pub mod impls;
