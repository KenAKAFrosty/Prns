//! The USB auto interface: a Prns-native, plug-and-play link over USB CDC.
//!
//! Unlike the serial interface, which targets one operator-named port and is
//! wire-exact against a stock RNS `SerialInterface`, this interface is *ours on
//! both ends* and *self-discovering*: a host enumerates the USB bus (and/or
//! listens for hotplug notifications), probes each
//! CDC port with our handshake, and attaches the ones that answer as a Personal
//! node. The operator plugs a cable in and it works without a port argument or configuration.

pub mod core;
