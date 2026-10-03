mod parser;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use personal_hopspot_builder::{capture_stdout, embedded_cargo_command, BuildContext, BuildError};
use thiserror::Error;

use crate::matrix::{BuildEvidence, Target, TargetPlatform};

const PRINT_TYPE_SIZES: &str = "print-type-sizes";
const MEASUREMENT_CONFIGURATION: &str = "prns_future_size_measurement";
const DIAGNOSTIC_LIMIT: usize = 4_096;

static MEASUREMENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NamedFutureSize {
    pub scenario: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Measurements {
    futures: Vec<NamedFutureSize>,
}

#[derive(Default)]
pub(crate) struct MeasurementCache {
    entries: Vec<CacheEntry>,
}

struct CacheEntry {
    platform: TargetPlatform,
    rust_target: &'static str,
    rustc_version: String,
    measurements: Measurements,
}

#[derive(Debug, Error)]
pub(crate) enum MeasurementError {
    #[error("build for {target:?} did not capture resource evidence for future-size measurement")]
    MissingResourceEvidence { target: String },
    #[error("future-size compiler for {architecture:?} is {actual:?}, expected the production compiler {expected:?}")]
    CompilerMismatch {
        architecture: String,
        expected: String,
        actual: String,
    },
    #[error("could not run the future-size compiler for {architecture:?}: {source}")]
    Compiler {
        architecture: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not prepare future-size workspace {path}: {source}")]
    Workspace {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not clean the future-size workspace for {architecture:?}: {source}")]
    Cleanup {
        architecture: String,
        #[source]
        source: std::io::Error,
    },
    #[error("future-size compilation for {architecture:?} exited with {status}: {diagnostics}")]
    Compilation {
        architecture: String,
        status: String,
        diagnostics: String,
    },
    #[error(transparent)]
    Evidence(#[from] parser::EvidenceError),
    #[error("could not identify the future-size compiler for {architecture:?}: {source}")]
    Toolchain {
        architecture: String,
        #[source]
        source: BuildError,
    },
}

impl Measurements {
    pub(crate) fn iter(&self) -> impl Iterator<Item = &NamedFutureSize> {
        self.futures.iter()
    }
}

impl MeasurementCache {
    pub(crate) fn for_target(
        &mut self,
        target: &Target<'_>,
        context: &BuildContext<'_>,
        evidence: &BuildEvidence,
    ) -> Result<&Measurements, MeasurementError> {
        let toolchain = evidence
            .firmware()
            .resource_build()
            .ok_or_else(|| MeasurementError::MissingResourceEvidence {
                target: target.id().to_string(),
            })?
            .toolchain();
        let platform = target.platform();
        let rust_target = target.adapter().rust_target();
        let rustc_version = toolchain.rustc_version();
        if let Some(index) = self.entries.iter().position(|entry| {
            entry.platform == platform
                && entry.rust_target == rust_target
                && entry.rustc_version == rustc_version
        }) {
            return Ok(&self.entries[index].measurements);
        }

        let measurements = measure(target, context, rustc_version)?;
        self.entries.push(CacheEntry {
            platform,
            rust_target,
            rustc_version: rustc_version.to_string(),
            measurements,
        });
        let index = self.entries.len() - 1;
        Ok(&self.entries[index].measurements)
    }
}

fn measure(
    target: &Target<'_>,
    context: &BuildContext<'_>,
    expected_rustc: &str,
) -> Result<Measurements, MeasurementError> {
    let architecture = target.adapter().architecture();
    let compiler_directory = compiler_directory(context.repository(), target.platform());
    let actual_rustc =
        capture_rustc(&compiler_directory).map_err(|source| MeasurementError::Toolchain {
            architecture: architecture.id().to_string(),
            source,
        })?;
    if actual_rustc != expected_rustc {
        return Err(MeasurementError::CompilerMismatch {
            architecture: architecture.id().to_string(),
            expected: expected_rustc.to_string(),
            actual: actual_rustc,
        });
    }

    let manifest = context
        .repository()
        .join("personal-hopspot")
        .join("assurance-kernel")
        .join("Cargo.toml");
    let work = context.output_root().join("work");
    std::fs::create_dir_all(&work).map_err(|source| MeasurementError::Workspace {
        path: work.clone(),
        source,
    })?;
    let prefix = format!(
        "semantic-futures-{}-{}-",
        target.platform().id(),
        architecture.id()
    );
    let target_directory = tempfile::Builder::new()
        .prefix(&prefix)
        .tempdir_in(&work)
        .map_err(|source| MeasurementError::Workspace { path: work, source })?;
    let mut cargo = embedded_cargo_command();
    cargo
        .arg("rustc")
        .arg("--release")
        .arg("--locked")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--lib")
        .arg("--target")
        .arg(architecture.rust_target())
        .arg("--target-dir")
        .arg(target_directory.path())
        .env("RUSTC_BOOTSTRAP", "1")
        .current_dir(compiler_directory);
    if target.platform() == TargetPlatform::Esp {
        cargo.arg("-Zbuild-std=core,alloc");
    }
    cargo
        .arg("--")
        .arg(format!("-Z{PRINT_TYPE_SIZES}"))
        .arg("--emit=metadata")
        .arg("--cfg")
        .arg(format!(
            "{MEASUREMENT_CONFIGURATION}=\"{}\"",
            measurement_nonce()
        ))
        .arg("--check-cfg")
        .arg(format!("cfg({MEASUREMENT_CONFIGURATION}, values(any()))"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cargo
        .output()
        .map_err(|source| MeasurementError::Compiler {
            architecture: architecture.id().to_string(),
            source,
        })?;
    target_directory
        .close()
        .map_err(|source| MeasurementError::Cleanup {
            architecture: architecture.id().to_string(),
            source,
        })?;
    if !output.status.success() {
        return Err(MeasurementError::Compilation {
            architecture: architecture.id().to_string(),
            status: output.status.to_string(),
            diagnostics: diagnostics(&output.stdout, &output.stderr),
        });
    }
    let futures = parser::parse(&output.stdout, &output.stderr)?;
    Ok(Measurements { futures })
}

fn capture_rustc(directory: &Path) -> Result<String, BuildError> {
    let mut rustc = Command::new("rustc");
    rustc
        .arg("-vV")
        .env_remove("RUSTUP_TOOLCHAIN")
        .current_dir(directory);
    Ok(capture_stdout(&mut rustc, "future-size rustc -vV")?
        .trim()
        .to_string())
}

fn compiler_directory(repository: &Path, platform: TargetPlatform) -> PathBuf {
    let platform = match platform {
        TargetPlatform::Esp => "esp32",
        TargetPlatform::Nrf52840 => "nrf52840",
    };
    repository
        .join("personal-hopspot")
        .join("embedded")
        .join(platform)
}

fn measurement_nonce() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = MEASUREMENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{sequence}", std::process::id())
}

fn diagnostics(stdout: &[u8], stderr: &[u8]) -> String {
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let mut start = combined.len().saturating_sub(DIAGNOSTIC_LIMIT);
    while !combined.is_char_boundary(start) {
        start += 1;
    }
    combined[start..].trim().to_string()
}

impl TargetPlatform {
    const fn id(self) -> &'static str {
        match self {
            Self::Esp => "esp",
            Self::Nrf52840 => "nrf52840",
        }
    }
}
