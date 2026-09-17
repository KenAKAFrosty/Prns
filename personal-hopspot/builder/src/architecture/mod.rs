mod linker;
mod riscv32imac;
mod thumbv7em;
mod xtensa;

#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use personal_hopspot_memory::ProcessorArchitecture;

use crate::{BuildError, BuildIntent};

const STACK_SIZE_EVIDENCE_RUSTFLAGS: [&str; 2] = ["-Z", "emit-stack-sizes=yes"];

pub use linker::{MemoryOverflow, MemoryOverflows};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AdapterId(&'static str);

impl AdapterId {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkerFlavor {
    RustLld,
    GnuLd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisassemblerFlavor {
    LlvmObjdump,
    GnuObjdump,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackFrameEvidence {
    LlvmStackSizes,
    DwarfDebugFrame,
}

impl DisassemblerFlavor {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LlvmObjdump => "llvm-objdump",
            Self::GnuObjdump => "gnu-objdump",
        }
    }
}

#[derive(Debug)]
struct DisassemblerTool {
    flavor: DisassemblerFlavor,
    program: &'static str,
    version_arguments: &'static [&'static str],
    resolve: fn() -> Result<PathBuf, BuildError>,
}

impl DisassemblerTool {
    const fn new(
        flavor: DisassemblerFlavor,
        program: &'static str,
        version_arguments: &'static [&'static str],
        resolve: fn() -> Result<PathBuf, BuildError>,
    ) -> Self {
        Self {
            flavor,
            program,
            version_arguments,
            resolve,
        }
    }
}

#[derive(Debug)]
pub struct ResolvedDisassembler {
    flavor: DisassemblerFlavor,
    program: &'static str,
    path: PathBuf,
    version_arguments: &'static [&'static str],
}

impl ResolvedDisassembler {
    #[must_use]
    pub const fn flavor(&self) -> DisassemblerFlavor {
        self.flavor
    }

    #[must_use]
    pub const fn program(&self) -> &'static str {
        self.program
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn version_arguments(&self) -> &'static [&'static str] {
        self.version_arguments
    }
}

impl LinkerFlavor {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RustLld => "rust-lld",
            Self::GnuLd => "gnu-ld",
        }
    }
}

#[derive(Debug)]
struct LinkerTool {
    flavor: LinkerFlavor,
    program: &'static str,
    version_arguments: &'static [&'static str],
    configure: fn(&mut Command) -> Result<PathBuf, BuildError>,
    map_argument: fn(&Path) -> OsString,
}

impl LinkerTool {
    const fn new(
        flavor: LinkerFlavor,
        program: &'static str,
        version_arguments: &'static [&'static str],
        configure: fn(&mut Command) -> Result<PathBuf, BuildError>,
        map_argument: fn(&Path) -> OsString,
    ) -> Self {
        Self {
            flavor,
            program,
            version_arguments,
            configure,
            map_argument,
        }
    }
}

#[derive(Debug)]
pub struct Adapter {
    id: AdapterId,
    architecture: ProcessorArchitecture,
    linker: LinkerTool,
    rustflags: &'static [&'static str],
    disassembler: DisassemblerTool,
    stack_frame_evidence: StackFrameEvidence,
}

impl Adapter {
    const fn new(
        id: &'static str,
        architecture: ProcessorArchitecture,
        linker: LinkerTool,
        rustflags: &'static [&'static str],
        disassembler: DisassemblerTool,
        stack_frame_evidence: StackFrameEvidence,
    ) -> Self {
        Self {
            id: AdapterId(id),
            architecture,
            linker,
            rustflags,
            disassembler,
            stack_frame_evidence,
        }
    }

    #[must_use]
    pub const fn id(&self) -> AdapterId {
        self.id
    }

    #[must_use]
    pub const fn architecture(&self) -> ProcessorArchitecture {
        self.architecture
    }

    #[must_use]
    pub const fn rust_target(&self) -> &'static str {
        self.architecture.rust_target()
    }

    #[must_use]
    pub const fn linker_flavor(&self) -> LinkerFlavor {
        self.linker.flavor
    }

    #[must_use]
    pub const fn linker_program(&self) -> &'static str {
        self.linker.program
    }

    pub(crate) const fn linker_version_arguments(&self) -> &'static [&'static str] {
        self.linker.version_arguments
    }

    #[must_use]
    pub const fn firmware_rustflags(&self) -> &'static [&'static str] {
        self.rustflags
    }

    #[must_use]
    pub fn rustflags(&self, intent: BuildIntent) -> Vec<&'static str> {
        let mut rustflags = self.rustflags.to_vec();
        if intent.is_resource_report()
            && self.stack_frame_evidence == StackFrameEvidence::LlvmStackSizes
        {
            rustflags.extend(STACK_SIZE_EVIDENCE_RUSTFLAGS);
        }
        rustflags
    }

    #[must_use]
    pub const fn stack_frame_evidence(&self) -> StackFrameEvidence {
        self.stack_frame_evidence
    }

    pub fn configure_cargo(
        &self,
        command: &mut Command,
        intent: BuildIntent,
    ) -> Result<PathBuf, BuildError> {
        self.configure_rustflags(command, intent);
        let linker = (self.linker.configure)(command)?;
        command.env(cargo_linker_environment(self.rust_target()), &linker);
        Ok(linker)
    }

    fn configure_rustflags(&self, command: &mut Command, intent: BuildIntent) {
        command
            .env_remove(cargo_rustflags_environment(self.rust_target()))
            .env_remove("RUSTC_BOOTSTRAP");
        if intent.is_resource_report()
            && self.stack_frame_evidence == StackFrameEvidence::LlvmStackSizes
        {
            command.env("RUSTC_BOOTSTRAP", "1");
        }
        let rustflags = self.rustflags(intent);
        if !rustflags.is_empty() {
            command.env("RUSTFLAGS", rustflags.join(" "));
        }
    }

    pub(crate) fn linker_map_argument(&self, path: &Path) -> OsString {
        (self.linker.map_argument)(path)
    }

    pub fn resolve_disassembler(&self) -> Result<ResolvedDisassembler, BuildError> {
        Ok(ResolvedDisassembler {
            flavor: self.disassembler.flavor,
            program: self.disassembler.program,
            path: (self.disassembler.resolve)()?,
            version_arguments: self.disassembler.version_arguments,
        })
    }

    #[must_use]
    pub const fn disassembler_flavor(&self) -> DisassemblerFlavor {
        self.disassembler.flavor
    }

    #[must_use]
    pub const fn disassembler_program(&self) -> &'static str {
        self.disassembler.program
    }

    pub(crate) fn detect_memory_overflow(&self, diagnostics: &str) -> Option<MemoryOverflows> {
        match self.linker.flavor {
            LinkerFlavor::RustLld => linker::rust_lld::detect(diagnostics),
            LinkerFlavor::GnuLd => linker::gnu_ld::detect(diagnostics),
        }
    }
}

pub const ADAPTERS: [&Adapter; 3] = [&thumbv7em::ADAPTER, &riscv32imac::ADAPTER, &xtensa::ADAPTER];

#[must_use]
pub const fn adapter_for(architecture: ProcessorArchitecture) -> &'static Adapter {
    match architecture {
        ProcessorArchitecture::ThumbV7em => &thumbv7em::ADAPTER,
        ProcessorArchitecture::RiscV32Imac => &riscv32imac::ADAPTER,
        ProcessorArchitecture::XtensaEsp32S3 => &xtensa::ADAPTER,
    }
}

pub fn adapter_for_rust_target(rust_target: &str) -> Result<&'static Adapter, BuildError> {
    ADAPTERS
        .iter()
        .copied()
        .find(|adapter| adapter.rust_target() == rust_target)
        .ok_or_else(|| BuildError::Toolchain(format!("unsupported Rust target {rust_target:?}")))
}

fn cargo_linker_environment(rust_target: &str) -> String {
    format!(
        "CARGO_TARGET_{}_LINKER",
        rust_target.replace('-', "_").to_ascii_uppercase()
    )
}

fn cargo_rustflags_environment(rust_target: &str) -> String {
    format!(
        "CARGO_TARGET_{}_RUSTFLAGS",
        rust_target.replace('-', "_").to_ascii_uppercase()
    )
}
