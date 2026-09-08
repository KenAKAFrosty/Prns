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

use crate::BuildError;

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
}

impl LinkerTool {
    const fn new(
        flavor: LinkerFlavor,
        program: &'static str,
        version_arguments: &'static [&'static str],
    ) -> Self {
        Self {
            flavor,
            program,
            version_arguments,
        }
    }
}

#[derive(Debug)]
pub struct Adapter {
    id: AdapterId,
    architecture: ProcessorArchitecture,
    linker: LinkerTool,
    rustflags: &'static [&'static str],
    configure_linker: fn(&mut Command) -> Result<PathBuf, BuildError>,
    linker_map_argument: fn(&Path) -> OsString,
}

impl Adapter {
    const fn new(
        id: &'static str,
        architecture: ProcessorArchitecture,
        linker: LinkerTool,
        rustflags: &'static [&'static str],
        configure_linker: fn(&mut Command) -> Result<PathBuf, BuildError>,
        linker_map_argument: fn(&Path) -> OsString,
    ) -> Self {
        Self {
            id: AdapterId(id),
            architecture,
            linker,
            rustflags,
            configure_linker,
            linker_map_argument,
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
    pub const fn rustflags(&self) -> &'static [&'static str] {
        self.rustflags
    }

    pub fn configure_cargo(&self, command: &mut Command) -> Result<PathBuf, BuildError> {
        self.configure_rustflags(command);
        let linker = (self.configure_linker)(command)?;
        command.env(cargo_linker_environment(self.rust_target()), &linker);
        Ok(linker)
    }

    fn configure_rustflags(&self, command: &mut Command) {
        command.env_remove(cargo_rustflags_environment(self.rust_target()));
        if !self.rustflags.is_empty() {
            command.env("RUSTFLAGS", self.rustflags.join(" "));
        }
    }

    pub(crate) fn linker_map_argument(&self, path: &Path) -> OsString {
        (self.linker_map_argument)(path)
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
