//! The RNS shared-instance control RPC, answered for stock clients: the RNS-compatibility control channel, distinct from Prns's own command API. Stock apps (Sideband, NomadNet, MeshChat) lean on it in ways that fault hard if nobody answers: LXMF turns on `link.track_phy_stats`, whose unguarded `Reticulum.get_packet_rssi` raises `ConnectionRefused` through `Link.receive` and hangs an attachment; NomadNet's TextUI calls `get_interface_stats` at startup and crashes if the reply is missing or the wrong shape.
//!
//! Wire protocol is `multiprocessing.connection`: a signed 4-byte big-endian length, or `-1` followed by an unsigned 8-byte length for wide frames; mutual HMAC challenge/response keyed on the shared `rpc_key`; and one request and reply per connection. Negotiated per peer: the HMAC digest exactly as CPython does (`{sha256}` tag vs legacy unprefixed HMAC-MD5), and the payload codec (RNS 1.3.5 msgpack vs legacy pickle), detected from the request's first byte and answered in kind.

mod authentication;
mod client;
mod framing;
mod server;
mod storage;
mod telemetry;

pub use authentication::SharedInstanceCredentials;
pub use client::{
    SharedInstanceRpcClient, SharedInstanceRpcClientError, SharedInstanceRpcClientPhase,
    SharedInstanceRpcEndpoint,
};
pub use server::{SharedInstanceRpcBindError, SharedInstanceRpcCompat, SharedInstanceRpcListener};
pub use storage::{load_or_seed_rns_rpc_key, reticulum_storage_dir, RnsRpcKeyStorageError};
pub use telemetry::{RpcTelemetry, RpcTelemetrySnapshot};

#[cfg(test)]
mod tests;
