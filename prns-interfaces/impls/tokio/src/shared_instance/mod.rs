pub mod blackhole_compat;
mod election;
pub mod rpc_compat;
pub mod server;

pub use blackhole_compat::RnsBlackholeFiles;
pub use election::{
    connect_existing_shared_instance, join_shared_instance, ExistingSharedInstanceUnavailable,
    InstancePorts, JoinError, OnExisting, Role, SharedInstanceBusEndpoint,
    SharedInstanceClientIntent, SharedInstanceEndpoint, SharedInstanceIntent,
    SharedInstanceTransport,
};
pub use rpc_compat::{
    SharedInstanceCredentials, SharedInstanceRpcClient, SharedInstanceRpcClientError,
    SharedInstanceRpcClientPhase, SharedInstanceRpcEndpoint,
};
