use std::io;
use std::path::{Path, PathBuf};

use personal_hopspot_builder::artifact::publish;
use personal_hopspot_builder::{BuildContext, BuildError, LinkOverflowEvidence, ToolchainEvidence};
use personal_hopspot_memory::MemoryProfile;
use serde::Serialize;
use thiserror::Error;

use crate::analysis::{self, AnalysisError, RamAnalysisError, RamCapacity, SectionKind};
use crate::matrix::{BuildEvidence, RecipeIdentity, Target};

use super::contract;
use super::fingerprint::fingerprint;
use super::model::{
    AnalysisEvidence, ArchitectureIdentity, ArtifactIdentity, BuildIdentity, BuildStatus, Evidence,
    FirmwareFlashUsage, MemoryOverflowIdentity, RamBackingUsage, RamCapacityIdentity,
    ResourceReport, SectionKindIdentity, SectionUsage, TargetIdentity, ToolchainIdentity,
    SCHEMA_VERSION,
};

const CARGO_PROFILE: &str = "release";

#[derive(Debug, Error)]
pub(crate) enum ReportError {
    #[error(transparent)]
    Contract(#[from] contract::ContractIdentityError),
    #[error("build for {target:?} did not capture resource evidence")]
    MissingResourceEvidence { target: String },
    #[error("overflow evidence for {actual:?} does not belong to target {expected:?}")]
    MismatchedOverflowTarget { expected: String, actual: String },
    #[error("memory profile {profile:?} has no firmware-owned region {region:?}")]
    MissingFirmwareRegion { profile: String, region: String },
    #[error("firmware image for {target:?} uses {actual} bytes but its region holds {maximum}")]
    FirmwareOverflow {
        target: String,
        actual: u64,
        maximum: u64,
    },
    #[error("could not inspect linker map {path}: {source}")]
    LinkerMapMetadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Analysis(#[from] AnalysisError),
    #[error(transparent)]
    RamAnalysis(#[from] RamAnalysisError),
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
    publish_report(context, target, &report)
}

pub(crate) fn write_overflow(
    target: &Target<'_>,
    context: &BuildContext<'_>,
    evidence: &LinkOverflowEvidence,
) -> Result<PathBuf, ReportError> {
    if evidence.target() != target.id() {
        return Err(ReportError::MismatchedOverflowTarget {
            expected: target.id().to_string(),
            actual: evidence.target().to_string(),
        });
    }
    let adapter = target.adapter();
    let linker_map_bytes = linker_map_size(evidence.linker_map())?;
    let report = ResourceReport {
        schema_version: SCHEMA_VERSION,
        target: target_identity(target),
        architecture: architecture_identity(target),
        build: build_identity(context, target.recipe_identity())?,
        toolchain: toolchain_identity(adapter.linker_program(), evidence.toolchain())?,
        memory_contract: contract::identity(target.profile())?,
        status: BuildStatus::MemoryOverflow {
            regions: evidence
                .overflows()
                .iter()
                .map(|overflow| MemoryOverflowIdentity {
                    linker_region: overflow.region().to_string(),
                    overflow_bytes: overflow.bytes(),
                })
                .collect(),
        },
        firmware_flash: Evidence::Unavailable,
        static_ram: Evidence::Unavailable,
        artifacts: Evidence::Unavailable,
        analysis: AnalysisEvidence {
            linker_map_bytes,
            allocated_sections: Evidence::Unavailable,
        },
    };
    publish_report(context, target, &report)
}

fn publish_report(
    context: &BuildContext<'_>,
    target: &Target<'_>,
    report: &ResourceReport,
) -> Result<PathBuf, ReportError> {
    let mut bytes = serde_json::to_vec_pretty(report)?;
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
    let allocated_sections = analysis::read_allocated_sections(evidence.elf())?;
    let static_ram = analysis::analyze_ram(target.profile(), &allocated_sections)?
        .into_iter()
        .map(|usage| RamBackingUsage {
            backing_store: usage.backing_store.0.to_string(),
            address_spaces: usage
                .address_spaces
                .into_iter()
                .map(|address_space| address_space.0.to_string())
                .collect(),
            capacity: match usage.capacity {
                RamCapacity::Known {
                    bytes,
                    headroom_bytes,
                } => RamCapacityIdentity::Known {
                    bytes,
                    headroom_bytes,
                },
                RamCapacity::RuntimeDetected => RamCapacityIdentity::RuntimeDetected,
            },
            static_section_bytes: usage.static_section_bytes,
            linker_padding_bytes: usage.linker_padding_bytes,
            additional_reservation_bytes: usage.additional_reservation_bytes,
            included_reservation_bytes: usage.included_reservation_bytes,
            external_reservation_bytes: usage.external_reservation_bytes,
        })
        .collect();
    let allocated_sections = allocated_sections
        .into_iter()
        .map(|section| {
            let run_range = section.run_range();
            SectionUsage {
                name: section.name().to_string(),
                kind: section_kind_identity(section.kind()),
                run_address: run_range.start(),
                run_end: run_range.end(),
                run_bytes: run_range.byte_len(),
                load_bytes: section.load_bytes(),
                alignment: section.alignment(),
            }
        })
        .collect();

    Ok(ResourceReport {
        schema_version: SCHEMA_VERSION,
        target: target_identity(target),
        architecture: architecture_identity(target),
        build: build_identity(context, recipe)?,
        toolchain: toolchain_identity(adapter.linker_program(), toolchain)?,
        memory_contract: contract::identity(target.profile())?,
        status: BuildStatus::Success,
        firmware_flash: Evidence::Complete(firmware_flash_usage(
            target.id(),
            target.profile(),
            evidence.firmware_image_bytes(),
        )?),
        static_ram: Evidence::Complete(static_ram),
        artifacts: Evidence::Complete(
            evidence
                .artifacts()
                .iter()
                .map(|artifact| ArtifactIdentity {
                    path: artifact.path().to_string(),
                    bytes: artifact.bytes(),
                })
                .collect(),
        ),
        analysis: AnalysisEvidence {
            linker_map_bytes,
            allocated_sections: Evidence::Complete(allocated_sections),
        },
    })
}

fn target_identity(target: &Target<'_>) -> TargetIdentity {
    TargetIdentity {
        id: target.id().to_string(),
        display_name: target.display_name().to_string(),
        memory_profile: target.profile().id.0.to_string(),
    }
}

fn architecture_identity(target: &Target<'_>) -> ArchitectureIdentity {
    let adapter = target.adapter();
    ArchitectureIdentity {
        rust_target: adapter.rust_target().to_string(),
        adapter: adapter.id().as_str().to_string(),
        linker_flavor: adapter.linker_flavor().as_str().to_string(),
    }
}

const fn section_kind_identity(kind: SectionKind) -> SectionKindIdentity {
    match kind {
        SectionKind::Code => SectionKindIdentity::Code,
        SectionKind::ReadOnlyData => SectionKindIdentity::ReadOnlyData,
        SectionKind::InitializedData => SectionKindIdentity::InitializedData,
        SectionKind::ZeroFill => SectionKindIdentity::ZeroFill,
        SectionKind::Other => SectionKindIdentity::Other,
    }
}

pub(super) fn firmware_flash_usage(
    target_id: &str,
    profile: &MemoryProfile,
    image_bytes: u64,
) -> Result<FirmwareFlashUsage, ReportError> {
    let region_id = profile.firmware.firmware_owned_region;
    let region = profile
        .region(region_id)
        .ok_or_else(|| ReportError::MissingFirmwareRegion {
            profile: profile.id.as_str().to_string(),
            region: region_id.0.to_string(),
        })?;
    let maximum = region.range.byte_len();
    let headroom_bytes =
        maximum
            .checked_sub(image_bytes)
            .ok_or_else(|| ReportError::FirmwareOverflow {
                target: target_id.to_string(),
                actual: image_bytes,
                maximum,
            })?;
    Ok(FirmwareFlashUsage {
        region: region.id.0.to_string(),
        start: region.range.start(),
        end: region.range.end(),
        image_bytes,
        headroom_bytes,
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
