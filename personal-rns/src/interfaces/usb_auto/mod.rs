//! The plug-and-play USB-auto interface built fresh against the reactor's
//! [`InterfaceSeam`](crate::reactor::interface_seam): the same `Prns`-magic handshake the legacy
//! interface speaks, so a reactor host and an unmigrated device still talk. The framing brain is
//! the host-agnostic [`core`] (reused wholesale from `crate::interfaces::impls::usb_auto`); each
//! host supplies only its async byte streams and discovery under [`impls`] — a tokio hub that
//! multiplexes many CDC ports today, an embassy device link to follow.

pub mod core;
pub mod impls;
