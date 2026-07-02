//! The seam between the node handle and the shared-instance control RPC: the read-only window the
//! RPC shim reads engine state through. The trait lives in core because the node handle implements
//! it; the shim that drives it (`SharedInstanceRpcCompat`) lives with the tokio interface impls.

use std::vec::Vec;

use crate::engine::RpcPathEntry;
use crate::wire::DestinationHash;

/// The shim's read-only window onto the engine: it issues these through the runtime handle to answer
/// a control-RPC verb with real state instead of a stub. Implemented by the node handle, which demuxes
/// each onto the command lane (see [`EngineCommand::RpcQuery`](crate::engine::RpcQuery)).
pub trait RpcQuerySource {
    /// `get_link_count` — the number of live links the node carries. The future is `Send` so the shim
    /// can answer each connection on its own task.
    fn link_count(&self) -> impl core::future::Future<Output = u32> + Send;

    /// `get_path_table` — every known destination, how it is reached, and when it was learned.
    fn path_table(&self) -> impl core::future::Future<Output = Vec<RpcPathEntry>> + Send;

    /// `get_next_hop` / `get_next_hop_if_name` — the one route to a destination, if the node holds it.
    fn route(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<Output = Option<RpcPathEntry>> + Send;
}
