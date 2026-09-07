mod model;
mod render;
mod validation;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::model::{
    ArtifactIdentity, BuildIdentity, RamBackingUsage, RamCapacityIdentity, ResourceReport,
    SectionKindIdentity,
};
use model::{
    ArtifactComparison, ByteComparison, RamComparison, RamHeadroomComparison, ResourceComparison,
    SectionComparison, SettingDifference,
};

const SECTION_KINDS: [(SectionKindIdentity, &str); 5] = [
    (SectionKindIdentity::Code, "code"),
    (SectionKindIdentity::ReadOnlyData, "read-only-data"),
    (SectionKindIdentity::InitializedData, "initialized-data"),
    (SectionKindIdentity::ZeroFill, "zero-fill"),
    (SectionKindIdentity::Other, "other"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompatibilityDimension {
    Target,
    Architecture,
    BuildRecipe,
    Toolchain,
    MemoryContract,
    BuildStatus,
    FirmwareRegion,
    ArtifactSet,
    RamTopology,
}

#[derive(Debug, Error)]
pub(crate) enum ComparisonError {
    #[error("could not read resource report {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse resource report {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "resource report {path} uses schema {actual}, but this tool supports schema {supported}"
    )]
    UnsupportedSchema {
        path: PathBuf,
        actual: u32,
        supported: u32,
    },
    #[error("resource report {path} has invalid flash accounting")]
    InvalidFlashAccounting { path: PathBuf },
    #[error("resource report {path} has no artifact evidence")]
    MissingArtifactEvidence { path: PathBuf },
    #[error("resource report {path} has invalid artifact evidence for {artifact:?}")]
    InvalidArtifact { path: PathBuf, artifact: String },
    #[error("resource report {path} repeats artifact {artifact:?}")]
    DuplicateArtifact { path: PathBuf, artifact: String },
    #[error("resource report {path} has no RAM evidence")]
    MissingRamEvidence { path: PathBuf },
    #[error("resource report {path} repeats RAM backing store {backing_store:?}")]
    DuplicateRamBacking {
        path: PathBuf,
        backing_store: String,
    },
    #[error("resource report {path} has invalid RAM accounting for {backing_store:?}")]
    InvalidRamAccounting {
        path: PathBuf,
        backing_store: String,
    },
    #[error("resource report {path} has no linker-map evidence")]
    MissingLinkerMapEvidence { path: PathBuf },
    #[error("resource report {path} has no allocated-section evidence")]
    MissingSectionEvidence { path: PathBuf },
    #[error("resource report {path} has invalid section accounting for {section:?}")]
    InvalidSectionAccounting { path: PathBuf, section: String },
    #[error("resource comparison overflowed the {kind} section total")]
    SectionTotalOverflow { kind: &'static str },
    #[error("resource reports are incompatible in {dimension}")]
    Incompatible { dimension: CompatibilityDimension },
}

pub(crate) fn compare_files(before: &Path, after: &Path) -> Result<String, ComparisonError> {
    let before_report = validation::load(before)?;
    let after_report = validation::load(after)?;
    let comparison = compare_reports(&before_report, &after_report)?;
    Ok(render::render(&comparison, before, after))
}

#[cfg(test)]
pub(super) fn validate_report(path: &Path, report: &ResourceReport) -> Result<(), ComparisonError> {
    validation::validate_report(path, report)
}

#[cfg(test)]
pub(super) fn render_comparison(
    comparison: &ResourceComparison,
    before: &Path,
    after: &Path,
) -> String {
    render::render(comparison, before, after)
}

pub(super) fn compare_reports(
    before: &ResourceReport,
    after: &ResourceReport,
) -> Result<ResourceComparison, ComparisonError> {
    require(
        before.target == after.target,
        CompatibilityDimension::Target,
    )?;
    require(
        before.architecture == after.architecture,
        CompatibilityDimension::Architecture,
    )?;
    require(
        compatible_build(&before.build, &after.build),
        CompatibilityDimension::BuildRecipe,
    )?;
    require(
        before.toolchain == after.toolchain,
        CompatibilityDimension::Toolchain,
    )?;
    require(
        before.memory_contract == after.memory_contract,
        CompatibilityDimension::MemoryContract,
    )?;
    require(
        before.status == after.status,
        CompatibilityDimension::BuildStatus,
    )?;
    require(
        before.firmware_flash.region == after.firmware_flash.region
            && before.firmware_flash.start == after.firmware_flash.start
            && before.firmware_flash.end == after.firmware_flash.end,
        CompatibilityDimension::FirmwareRegion,
    )?;
    let artifacts = compare_artifacts(&before.artifacts, &after.artifacts)?;
    let ram = compare_ram(&before.static_ram, &after.static_ram)?;
    let settings = if before.build.lto == after.build.lto {
        Vec::new()
    } else {
        vec![SettingDifference::Lto {
            before: before.build.lto.clone(),
            after: after.build.lto.clone(),
        }]
    };
    Ok(ResourceComparison {
        target: before.target.id.clone(),
        settings,
        flash_image: ByteComparison::new(
            before.firmware_flash.image_bytes,
            after.firmware_flash.image_bytes,
        ),
        flash_headroom: ByteComparison::new(
            before.firmware_flash.headroom_bytes,
            after.firmware_flash.headroom_bytes,
        ),
        artifacts,
        ram,
        sections: compare_sections(before, after)?,
    })
}

fn compatible_build(before: &BuildIdentity, after: &BuildIdentity) -> bool {
    let fingerprints_match_difference = if before.lto == after.lto {
        before.fingerprint == after.fingerprint
    } else {
        before.fingerprint != after.fingerprint
    };
    fingerprints_match_difference
        && before.firmware_version == after.firmware_version
        && before.cargo_profile == after.cargo_profile
        && before.recipe_kind == after.recipe_kind
        && before.package == after.package
        && before.binary == after.binary
        && before.features == after.features
}

fn compare_artifacts(
    before: &[ArtifactIdentity],
    after: &[ArtifactIdentity],
) -> Result<Vec<ArtifactComparison>, ComparisonError> {
    require(
        before.len() == after.len(),
        CompatibilityDimension::ArtifactSet,
    )?;
    before
        .iter()
        .map(|artifact| {
            let Some(other) = after.iter().find(|other| other.path == artifact.path) else {
                return Err(ComparisonError::Incompatible {
                    dimension: CompatibilityDimension::ArtifactSet,
                });
            };
            Ok(ArtifactComparison {
                path: artifact.path.clone(),
                bytes: ByteComparison::new(artifact.bytes, other.bytes),
            })
        })
        .collect()
}

fn compare_ram(
    before: &[RamBackingUsage],
    after: &[RamBackingUsage],
) -> Result<Vec<RamComparison>, ComparisonError> {
    require(
        before.len() == after.len(),
        CompatibilityDimension::RamTopology,
    )?;
    before
        .iter()
        .map(|usage| {
            let Some(other) = after
                .iter()
                .find(|other| other.backing_store == usage.backing_store)
            else {
                return Err(ComparisonError::Incompatible {
                    dimension: CompatibilityDimension::RamTopology,
                });
            };
            require(
                compatible_ram(usage, other),
                CompatibilityDimension::RamTopology,
            )?;
            let headroom = match (&usage.capacity, &other.capacity) {
                (
                    RamCapacityIdentity::Known {
                        headroom_bytes: before,
                        ..
                    },
                    RamCapacityIdentity::Known {
                        headroom_bytes: after,
                        ..
                    },
                ) => RamHeadroomComparison::Known(ByteComparison::new(*before, *after)),
                (RamCapacityIdentity::RuntimeDetected, RamCapacityIdentity::RuntimeDetected) => {
                    RamHeadroomComparison::RuntimeDetected
                }
                _ => {
                    return Err(ComparisonError::Incompatible {
                        dimension: CompatibilityDimension::RamTopology,
                    });
                }
            };
            Ok(RamComparison {
                backing_store: usage.backing_store.clone(),
                static_sections: ByteComparison::new(
                    usage.static_section_bytes,
                    other.static_section_bytes,
                ),
                linker_padding: ByteComparison::new(
                    usage.linker_padding_bytes,
                    other.linker_padding_bytes,
                ),
                headroom,
            })
        })
        .collect()
}

fn compatible_ram(before: &RamBackingUsage, after: &RamBackingUsage) -> bool {
    before.address_spaces == after.address_spaces
        && before.additional_reservation_bytes == after.additional_reservation_bytes
        && before.included_reservation_bytes == after.included_reservation_bytes
        && before.external_reservation_bytes == after.external_reservation_bytes
        && match (&before.capacity, &after.capacity) {
            (
                RamCapacityIdentity::Known { bytes: before, .. },
                RamCapacityIdentity::Known { bytes: after, .. },
            ) => before == after,
            (RamCapacityIdentity::RuntimeDetected, RamCapacityIdentity::RuntimeDetected) => true,
            _ => false,
        }
}

fn compare_sections(
    before: &ResourceReport,
    after: &ResourceReport,
) -> Result<Vec<SectionComparison>, ComparisonError> {
    SECTION_KINDS
        .iter()
        .map(|(kind, name)| {
            let (before_run, before_load) = section_totals(before, *kind)
                .ok_or(ComparisonError::SectionTotalOverflow { kind: name })?;
            let (after_run, after_load) = section_totals(after, *kind)
                .ok_or(ComparisonError::SectionTotalOverflow { kind: name })?;
            Ok(SectionComparison {
                kind: name,
                run_bytes: ByteComparison::new(before_run, after_run),
                load_bytes: ByteComparison::new(before_load, after_load),
            })
        })
        .collect()
}

fn section_totals(report: &ResourceReport, kind: SectionKindIdentity) -> Option<(u64, u64)> {
    report
        .analysis
        .allocated_sections
        .iter()
        .filter(|section| section.kind == kind)
        .try_fold((0_u64, 0_u64), |(run, load), section| {
            Some((
                run.checked_add(section.run_bytes)?,
                load.checked_add(section.load_bytes)?,
            ))
        })
}

fn require(condition: bool, dimension: CompatibilityDimension) -> Result<(), ComparisonError> {
    if condition {
        Ok(())
    } else {
        Err(ComparisonError::Incompatible { dimension })
    }
}

impl fmt::Display for CompatibilityDimension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Target => "target identity",
            Self::Architecture => "architecture identity",
            Self::BuildRecipe => "build recipe",
            Self::Toolchain => "toolchain identity",
            Self::MemoryContract => "memory contract",
            Self::BuildStatus => "build status",
            Self::FirmwareRegion => "firmware region",
            Self::ArtifactSet => "artifact set",
            Self::RamTopology => "RAM topology",
        })
    }
}
