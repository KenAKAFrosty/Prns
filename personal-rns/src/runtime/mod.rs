//! Runtime layer that owns an engine plus its started interfaces.

pub mod channels;
mod contract;
mod core;
pub mod host;
mod snapshot;

pub use contract::{run_contract, ContractRuntime, ContractStepOutput};
pub use host::{block_on, CycleStamp, Host};
pub use snapshot::{InterfaceView, RuntimeSnapshot};
