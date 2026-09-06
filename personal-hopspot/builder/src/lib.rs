pub mod architecture;
pub mod artifact;
mod context;
mod error;
mod evidence;
mod intent;
pub mod platform;
mod toolchain;

pub use context::{default_artifact_root, BuildContext, BuildVersion};
pub use error::BuildError;
pub use evidence::{FirmwareEvidence, ResourceBuildEvidence, ToolchainEvidence};
pub use intent::{BuildIntent, LtoMode};
pub use toolchain::{
    capture_stdout, embedded_cargo_command, llvm_objcopy, run_status, rust_host_triple,
};
