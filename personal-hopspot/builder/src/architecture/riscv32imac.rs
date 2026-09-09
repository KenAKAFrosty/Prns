use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use personal_hopspot_memory::ProcessorArchitecture;

use super::{
    Adapter, DisassemblerFlavor, DisassemblerTool, LinkerFlavor, LinkerTool, StackFrameEvidence,
};
use crate::toolchain::{rust_tool, rust_tool_for_cargo};
use crate::BuildError;

pub(super) static ADAPTER: Adapter = Adapter::new(
    "riscv32imac-rust-lld",
    ProcessorArchitecture::RiscV32Imac,
    LinkerTool::new(
        LinkerFlavor::RustLld,
        "rust-lld",
        &["-flavor", "gnu", "--version"],
        configure_linker,
        linker_map_argument,
    ),
    &["-C", "link-arg=-Tlinkall.x"],
    DisassemblerTool::new(
        DisassemblerFlavor::LlvmObjdump,
        "llvm-objdump",
        &["--version"],
        resolve_disassembler,
    ),
    StackFrameEvidence::DwarfDebugFrame,
);

fn configure_linker(command: &mut Command) -> Result<PathBuf, BuildError> {
    rust_tool_for_cargo(command, "rust-lld")
}

fn resolve_disassembler() -> Result<PathBuf, BuildError> {
    rust_tool("llvm-objdump")
}

fn linker_map_argument(path: &Path) -> OsString {
    format!("link-arg=-Map={}", path.display()).into()
}
