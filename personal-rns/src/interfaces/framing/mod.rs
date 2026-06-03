//! Wire framing codecs interfaces build on, but which are not interfaces
//! themselves.
//!
//! [`rns_serial_framing`] is the RNS reference-compatible byte-stuffed framing
//! used by serial-style transports (RS-232, UARTs, etc.) to mark frame
//! boundaries in a raw byte stream. A concrete interface over such a medium
//! composes this codec; the codec itself knows nothing about routing or the
//! engine.

pub mod rns_serial_framing;
