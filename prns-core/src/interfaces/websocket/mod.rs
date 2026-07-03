//! A host-only WebSocket interface: one binary WebSocket message carries one RNS wire frame.
//! Not an upstream RNS parity interface, but it deliberately follows the same reactor seam and
//! lifecycle shape as TCP (an outbound connector, a listener supervisor, one per-connection
//! peer interface), all in `prns-interfaces-tokio`'s `websocket` module above this
//! host-agnostic [`core`]. Gives browser code a native integration path. TLS is intentionally
//! outside this transport body; deploy `wss://` by putting a TLS terminator or reverse proxy
//! in front of the plain `ws://` listener.

pub mod core;
