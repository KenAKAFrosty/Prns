//! The plug-and-play USB-auto interface against the reactor's
//! [`InterfaceSeam`](crate::reactor::interface_seam). The framing brain is the
//! host-agnostic [`core`]; each host supplies only its async byte streams and discovery
//! in its runtime's interface crate — the tokio hub that multiplexes many CDC ports,
//! and the embassy device link.

pub mod core;
