//! Interfaces built fresh against the reactor's [`InterfaceSeam`](super::interface_seam):
//! async-native, each its own small reactor over one medium. They reuse the shared codecs
//! (e.g. `rns_serial_framing`) from `crate::interfaces` but none of the old poll-loop seam.

pub mod serial;
