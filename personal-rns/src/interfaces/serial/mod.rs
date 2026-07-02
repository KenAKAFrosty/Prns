//! A serial interface built fresh against the reactor's
//! [`InterfaceSeam`](crate::reactor::interface_seam): the RNS HDLC-style framing over a
//! direct peer. The framing brain is the host-agnostic [`core`]; each host supplies only the
//! async byte stream and its select primitive in `prns-interfaces-tokio`.

pub mod core;
