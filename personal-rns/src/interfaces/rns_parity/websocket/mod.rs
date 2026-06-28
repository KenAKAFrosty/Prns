//! A host-only WebSocket interface for Prns: one binary WebSocket message carries one RNS wire
//! frame. It is not an upstream RNS parity interface, but it deliberately follows the same reactor
//! seam and lifecycle shape as TCP: an outbound connector, a listener supervisor, and one
//! per-connection peer interface.
//!
//! The WebSocket layer gives browser code a native integration path: a page can open a `WebSocket`,
//! send binary frames, and participate through a local or public Prns endpoint without raw TCP. TLS
//! is intentionally outside this first transport body; deploy `wss://` by putting a TLS terminator or
//! reverse proxy in front of the plain `ws://` listener.

#[cfg(feature = "websocket-core")]
pub mod core;

#[cfg(feature = "websocket")]
pub mod client;
#[cfg(feature = "websocket")]
pub mod server;
#[cfg(feature = "websocket")]
mod tokio_wire;
