use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use personal_hopspot_memory::ProcessorArchitecture;

use super::{Adapter, LinkerFlavor};
use crate::toolchain::rust_tool_for_cargo;
use crate::BuildError;

pub(super) static ADAPTER: Adapter = Adapter::new(
    "thumbv7em-rust-lld",
    ProcessorArchitecture::ThumbV7em,
    LinkerFlavor::RustLld,
    "rust-lld",
    configure_linker,
    linker_map_argument,
);

fn configure_linker(command: &mut Command) -> Result<PathBuf, BuildError> {
    rust_tool_for_cargo(command, "rust-lld")
}

fn linker_map_argument(path: &Path) -> OsString {
    format!("link-arg=-Map={}", path.display()).into()
}
