pub mod blackhole_compat;
mod election;
pub mod rpc_compat;
pub mod server;

pub use blackhole_compat::RnsLocalBlackholeFile;
pub use election::{
    join_shared_instance, InstancePorts, JoinError, OnExisting, Role, SharedInstanceEndpoint,
    SharedInstanceIntent, SharedInstanceTransport,
};
pub use rpc_compat::SharedInstanceCredentials;
