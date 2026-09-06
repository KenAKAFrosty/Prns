pub mod architecture;
pub mod artifact;
mod configuration;
mod context;
mod error;
mod evidence;
pub mod platform;
mod toolchain;

pub use configuration::{BuildConfiguration, LtoMode};
pub use context::{default_artifact_root, BuildContext, BuildVersion};
pub use error::BuildError;
pub use evidence::FirmwareEvidence;
pub use toolchain::{
    capture_stdout, embedded_cargo_command, llvm_objcopy, run_status, rust_host_triple,
};
