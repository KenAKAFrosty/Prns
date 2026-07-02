mod election;
pub mod rpc_compat;
pub mod server;

pub use election::{
    join_shared_instance, InstancePorts, JoinError, OnExisting, Role, SharedInstanceIntent,
};
