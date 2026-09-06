use std::io;
use std::path::{Path, PathBuf};

use personal_hopspot_builder::artifact::publish;
use personal_hopspot_builder::{BuildContext, BuildError, ToolchainEvidence};
use serde::Serialize;
use thiserror::Error;

use crate::matrix::{BuildEvidence, RecipeIdentity, Target};

use super::contract;
use super::fingerprint::fingerprint;
use super::model::{
    AnalysisState, ArchitectureIdentity, ArtifactIdentity, BuildIdentity, BuildStatus,
    ResourceReport, TargetIdentity, ToolchainIdentity, SCHEMA_VERSION,
};

const CARGO_PROFILE: &str = "release";

#[derive(Debug, Error)]
pub(crate) enum ReportError {
    #[error(transparent)]
    Contract(#[from] contract::ContractIdentityError),
    #[error("build for {target:?} did not capture resource evidence")]
    MissingResourceEvidence { target: String },
    #[error("could not inspect linker map {path}: {source}")]
    LinkerMapMetadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not serialize resource report: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("could not publish resource report: {0}")]
    Publish(#[from] BuildError),
}

pub(crate) fn write(
    target: &Target<'_>,
    context: &BuildContext<'_>,
    evidence: &BuildEvidence,
) -> Result<PathBuf, ReportError> {
    let report = build(target, context, evidence)?;
    let mut bytes = serde_json::to_vec_pretty(&report)?;
    bytes.push(b'\n');
    let path = context
        .configured_output_root()
        .join("reports")
        .join(format!("{}.json", target.id()));
    publish(&path, &bytes)?;
    Ok(path)
}

fn build(
    target: &Target<'_>,
    context: &BuildContext<'_>,
    evidence: &BuildEvidence,
) -> Result<ResourceReport, ReportError> {
    let recipe = target.recipe_identity();
    let adapter = target.adapter();
    let resource_build = evidence.firmware().resource_build().ok_or_else(|| {
        ReportError::MissingResourceEvidence {
            target: target.id().to_string(),
        }
    })?;
    let toolchain = resource_build.toolchain();
    let linker_map = resource_build.linker_map();
    let linker_map_bytes = linker_map_size(linker_map)?;

    Ok(ResourceReport {
        schema_version: SCHEMA_VERSION,
        target: TargetIdentity {
            id: target.id().to_string(),
            display_name: target.display_name().to_string(),
            memory_profile: target.profile().id.0.to_string(),
        },
        architecture: ArchitectureIdentity {
            rust_target: adapter.rust_target().to_string(),
            adapter: adapter.id().as_str().to_string(),
            linker_flavor: adapter.linker_flavor().as_str().to_string(),
        },
        build: build_identity(context, recipe)?,
        toolchain: toolchain_identity(adapter.linker_program(), toolchain)?,
        memory_contract: contract::identity(target.profile())?,
        status: BuildStatus::Success,
        artifacts: evidence
            .artifacts()
            .iter()
            .map(|artifact| ArtifactIdentity {
                path: artifact.path().to_string(),
                bytes: artifact.bytes(),
            })
            .collect(),
        analysis: AnalysisState::Pending { linker_map_bytes },
    })
}

pub(super) fn build_identity(
    context: &BuildContext<'_>,
    recipe: RecipeIdentity<'_>,
) -> Result<BuildIdentity, serde_json::Error> {
    let features = recipe
        .features
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let version = context.version();
    let recipe_kind = recipe.kind;
    let package = recipe.package;
    let binary = recipe.binary;
    let lto = context.intent().lto().as_str();
    let body = BuildFingerprint {
        firmware_version: version,
        cargo_profile: CARGO_PROFILE,
        recipe_kind,
        package,
        binary,
        features: &features,
        lto,
    };
    let fingerprint = fingerprint(&body)?;
    Ok(BuildIdentity {
        fingerprint,
        firmware_version: version.to_string(),
        cargo_profile: CARGO_PROFILE.to_string(),
        recipe_kind: recipe_kind.to_string(),
        package: package.to_string(),
        binary: binary.to_string(),
        features,
        lto: lto.to_string(),
    })
}

fn toolchain_identity(
    linker_program: &str,
    evidence: &ToolchainEvidence,
) -> Result<ToolchainIdentity, serde_json::Error> {
    let body = ToolchainFingerprint {
        rustc_version: evidence.rustc_version(),
        cargo_version: evidence.cargo_version(),
        linker_program,
        linker_version: evidence.linker_version(),
    };
    Ok(ToolchainIdentity {
        fingerprint: fingerprint(&body)?,
        rustc_version: body.rustc_version.to_string(),
        cargo_version: body.cargo_version.to_string(),
        linker_program: body.linker_program.to_string(),
        linker_version: body.linker_version.to_string(),
    })
}

fn linker_map_size(path: &Path) -> Result<u64, ReportError> {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|source| ReportError::LinkerMapMetadata {
            path: path.to_path_buf(),
            source,
        })
}

#[derive(Serialize)]
struct BuildFingerprint<'a> {
    firmware_version: &'a str,
    cargo_profile: &'a str,
    recipe_kind: &'a str,
    package: &'a str,
    binary: &'a str,
    features: &'a [String],
    lto: &'a str,
}

#[derive(Serialize)]
struct ToolchainFingerprint<'a> {
    rustc_version: &'a str,
    cargo_version: &'a str,
    linker_program: &'a str,
    linker_version: &'a str,
}
