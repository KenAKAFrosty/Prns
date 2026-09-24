use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use personal_hopspot_memory::ProcessorArchitecture;

use super::{
    Adapter, DisassemblerFlavor, DisassemblerTool, LinkerFlavor, LinkerTool, StackFrameEvidence,
};
use crate::toolchain::{rust_tool, rust_tool_for_cargo};
use crate::BuildError;

pub(super) static ADAPTER: Adapter = adapter(
    "thumbv7em-rust-lld",
    &[
        "-C",
        "link-arg=--icf=all",
        "-C",
        "llvm-args=-enable-machine-outliner",
        "--cfg",
        "sha2_backend_soft=\"compact\"",
    ],
);

pub(super) static SERIAL_DFU_ADAPTER: Adapter = adapter("thumbv7em-serial-dfu-rust-lld", &[]);

const fn adapter(id: &'static str, rustflags: &'static [&'static str]) -> Adapter {
    Adapter::new(
        id,
        ProcessorArchitecture::ThumbV7em,
        LinkerTool::new(
            LinkerFlavor::RustLld,
            "rust-lld",
            &["-flavor", "gnu", "--version"],
            configure_linker,
            linker_map_argument,
        ),
        rustflags,
        DisassemblerTool::new(
            DisassemblerFlavor::LlvmObjdump,
            "llvm-objdump",
            &["--version"],
            resolve_disassembler,
        ),
        StackFrameEvidence::LlvmStackSizes,
    )
}

fn configure_linker(command: &mut Command) -> Result<PathBuf, BuildError> {
    rust_tool_for_cargo(command, "rust-lld")
}

fn resolve_disassembler() -> Result<PathBuf, BuildError> {
    rust_tool("llvm-objdump")
}

fn linker_map_argument(path: &Path) -> OsString {
    format!("link-arg=-Map={}", path.display()).into()
}
