//! The RNS `BackboneInterface` / `BackboneClientInterface`: a TCP backbone link for transport nodes.
//! On the wire it is *identical* to the [`tcp`](super::tcp) interface — the same RNS HDLC framing
//! (`FLAG=0x7E`, `ESC=0x7D`) over a TCP stream, the same socket discipline (`TCP_NODELAY`, keepalive
//! probes), the same reconnect cadence. So this module is deliberately thin: it reuses
//! [`tcp::core`](super::tcp::core)'s sizing and descriptor, [`framed_stream::serve`] with
//! [`HdlcFraming`], and the [`tcp::tokio_socket`](super::tcp::tokio_socket) discipline; only the
//! interface *kinds* and the bitrate guesses are its own (see [`core`]).
//!
//! What sets `BackboneInterface` apart upstream is its *I/O backend*, not its wire: RNS drives every
//! backbone socket from a single `select.epoll()` loop for high client counts, which is why upstream
//! hard-errors (`OSError`) off Linux/Android — `epoll` is Linux-only. We do **not** replicate that
//! platform gate: tokio's reactor is the cross-platform `epoll` equivalent, so each connection is just
//! a task and Backbone runs anywhere tokio does. The parity that matters — the frames on the wire and
//! the per-connection lifecycle — is preserved; the arbitrary OS restriction is not.
//!
//! [`server`] is the listener (`BackboneInterface`) standing up a [`server::BackboneServerConnection`]
//! per accepted client, the way the reference spawns a child interface per connection; [`client`] is
//! the outbound connector (`BackboneClientInterface`). Both share [`core`].

pub mod core;

#[cfg(feature = "tcp")]
pub mod client;
#[cfg(feature = "tcp")]
pub mod server;
