pub mod architecture;
pub mod artifact;
mod context;
mod error;
mod evidence;
mod intent;
pub mod platform;
mod source;
mod toolchain;

pub use context::{default_artifact_root, BuildContext, BuildVersion};
pub use error::BuildError;
pub use evidence::{
    FirmwareEvidence, LinkOverflowEvidence, ResourceBuildEvidence, ToolchainEvidence,
};
pub use intent::{BuildIntent, LtoMode};
pub use source::{
    capture_source_custody, RepositoryCommit, SourceCaptureError, SourceCustody,
    WorkingTreeFingerprint,
};
pub use toolchain::{
    capture_stdout, embedded_cargo_command, llvm_objcopy, run_status, rust_host_triple,
};
