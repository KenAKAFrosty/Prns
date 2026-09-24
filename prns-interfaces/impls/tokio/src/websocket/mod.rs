#[cfg(feature = "websocket")]
mod client;
pub(crate) mod framing;
mod server;

#[cfg(feature = "websocket")]
pub use client::WebSocketClientInterface;
pub use server::{WebSocketServer, WebSocketServerConnection, WebSocketServerStatus};
