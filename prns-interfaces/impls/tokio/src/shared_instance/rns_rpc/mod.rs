mod authentication;
mod client;
mod framing;
mod legacy;
mod persistence;
mod server;
mod telemetry;

pub use authentication::SharedInstanceCredentials;
pub use client::{
    SharedInstanceBlackholeOutcome, SharedInstancePacketPhyStats, SharedInstanceRpcClient,
    SharedInstanceRpcClientError, SharedInstanceRpcClientPhase, SharedInstanceRpcEndpoint,
    SharedInstanceUnblackholeOutcome,
};
pub use persistence::{load_or_seed_rns_rpc_key, reticulum_storage_dir, RnsRpcKeyStorageError};
pub use prns_core::interfaces::shared_instance::rns_rpc::RpcAuthenticationKey;
pub use server::{SharedInstanceRpcBindError, SharedInstanceRpcListener, SharedInstanceRpcServer};
pub use telemetry::{RpcTelemetry, RpcTelemetrySnapshot};

#[cfg(test)]
mod tests;
