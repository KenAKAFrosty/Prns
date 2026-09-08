use std::fs;
use std::path::Path;

use super::super::model::{
    ArtifactIdentity, AttributionCategoryIdentity, BuildStatus, Evidence, FirmwareFlashUsage,
    FlashAttributionIdentity, MemoryOverflowIdentity, RamBackingUsage, RamCapacityIdentity,
    ResourceReport, SectionKindIdentity, SectionUsage, SCHEMA_VERSION,
};
use super::{ComparisonError, SECTION_KINDS};

pub(super) fn load(path: &Path) -> Result<ResourceReport, ComparisonError> {
    let bytes = fs::read(path).map_err(|source| ComparisonError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let report = serde_json::from_slice::<ResourceReport>(&bytes).map_err(|source| {
        ComparisonError::Parse {
            path: path.to_path_buf(),
            source,
        }
    })?;
    validate_report(path, &report)?;
    Ok(report)
}

pub(super) fn validate_report(path: &Path, report: &ResourceReport) -> Result<(), ComparisonError> {
    if report.schema_version != SCHEMA_VERSION {
        return Err(ComparisonError::UnsupportedSchema {
            path: path.to_path_buf(),
            actual: report.schema_version,
            supported: SCHEMA_VERSION,
        });
    }
    validate_toolchain(path, report)?;
    validate_linker_map(path, report)?;
    match &report.status {
        BuildStatus::Success => validate_success(path, report),
        BuildStatus::MemoryOverflow { regions } => {
            validate_overflows(path, regions)?;
            validate_available(path, report)
        }
    }
}

fn validate_toolchain(path: &Path, report: &ResourceReport) -> Result<(), ComparisonError> {
    let toolchain = &report.toolchain;
    let valid = !toolchain.rustc_version.is_empty()
        && !toolchain.cargo_version.is_empty()
        && !toolchain.linker_program.is_empty()
        && !toolchain.linker_version.is_empty()
        && super::super::build::toolchain_fingerprint(
            &toolchain.rustc_version,
            &toolchain.cargo_version,
            &toolchain.linker_program,
            &toolchain.linker_version,
        )
        .is_ok_and(|fingerprint| fingerprint == toolchain.fingerprint);
    if valid {
        Ok(())
    } else {
        Err(ComparisonError::InvalidToolchainIdentity {
            path: path.to_path_buf(),
        })
    }
}

fn validate_success(path: &Path, report: &ResourceReport) -> Result<(), ComparisonError> {
    let flash =
        report
            .firmware_flash
            .complete()
            .ok_or_else(|| ComparisonError::MissingFlashEvidence {
                path: path.to_path_buf(),
            })?;
    let artifacts =
        report
            .artifacts
            .complete()
            .ok_or_else(|| ComparisonError::MissingArtifactEvidence {
                path: path.to_path_buf(),
            })?;
    let ram = report
        .static_ram
        .complete()
        .ok_or_else(|| ComparisonError::MissingRamEvidence {
            path: path.to_path_buf(),
        })?;
    let sections = report
        .analysis
        .allocated_sections
        .complete()
        .ok_or_else(|| ComparisonError::MissingSectionEvidence {
            path: path.to_path_buf(),
        })?;
    let attribution = report
        .analysis
        .flash_attribution
        .complete()
        .ok_or_else(|| ComparisonError::MissingAttributionEvidence {
            path: path.to_path_buf(),
        })?;
    validate_flash(path, flash)?;
    validate_artifacts(path, artifacts)?;
    validate_ram(path, ram)?;
    validate_sections(path, sections)?;
    validate_attribution(path, attribution)
}

fn validate_available(path: &Path, report: &ResourceReport) -> Result<(), ComparisonError> {
    if let Evidence::Complete(flash) | Evidence::Partial(flash) = &report.firmware_flash {
        validate_flash(path, flash)?;
    }
    if let Evidence::Complete(artifacts) | Evidence::Partial(artifacts) = &report.artifacts {
        validate_artifacts(path, artifacts)?;
    }
    if let Evidence::Complete(ram) | Evidence::Partial(ram) = &report.static_ram {
        validate_ram(path, ram)?;
    }
    if let Evidence::Complete(sections) | Evidence::Partial(sections) =
        &report.analysis.allocated_sections
    {
        validate_sections(path, sections)?;
    }
    if let Evidence::Complete(attribution) | Evidence::Partial(attribution) =
        &report.analysis.flash_attribution
    {
        validate_attribution(path, attribution)?;
    }
    Ok(())
}

fn validate_overflows(
    path: &Path,
    overflows: &[MemoryOverflowIdentity],
) -> Result<(), ComparisonError> {
    if overflows.is_empty() {
        return Err(ComparisonError::MissingOverflowEvidence {
            path: path.to_path_buf(),
        });
    }
    for (index, overflow) in overflows.iter().enumerate() {
        let duplicate = overflows[..index]
            .iter()
            .any(|prior| prior.linker_region == overflow.linker_region);
        if overflow.linker_region.is_empty() || overflow.overflow_bytes == 0 || duplicate {
            return Err(ComparisonError::InvalidOverflowEvidence {
                path: path.to_path_buf(),
                linker_region: overflow.linker_region.clone(),
            });
        }
    }
    Ok(())
}

fn validate_linker_map(path: &Path, report: &ResourceReport) -> Result<(), ComparisonError> {
    if report.analysis.linker_map_bytes == 0 {
        Err(ComparisonError::MissingLinkerMapEvidence {
            path: path.to_path_buf(),
        })
    } else {
        Ok(())
    }
}

fn validate_flash(path: &Path, flash: &FirmwareFlashUsage) -> Result<(), ComparisonError> {
    let valid = flash
        .end
        .checked_sub(flash.start)
        .filter(|capacity| *capacity != 0)
        .zip(flash.image_bytes.checked_add(flash.headroom_bytes))
        .is_some_and(|(capacity, used)| flash.image_bytes != 0 && capacity == used);
    if valid {
        Ok(())
    } else {
        Err(ComparisonError::InvalidFlashAccounting {
            path: path.to_path_buf(),
        })
    }
}

fn validate_artifacts(path: &Path, artifacts: &[ArtifactIdentity]) -> Result<(), ComparisonError> {
    for (index, artifact) in artifacts.iter().enumerate() {
        if artifact.path.is_empty() || artifact.bytes == 0 {
            return Err(ComparisonError::InvalidArtifact {
                path: path.to_path_buf(),
                artifact: artifact.path.clone(),
            });
        }
        if artifacts[..index]
            .iter()
            .any(|prior| prior.path == artifact.path)
        {
            return Err(ComparisonError::DuplicateArtifact {
                path: path.to_path_buf(),
                artifact: artifact.path.clone(),
            });
        }
    }
    Ok(())
}

fn validate_ram(path: &Path, ram: &[RamBackingUsage]) -> Result<(), ComparisonError> {
    if ram.is_empty() {
        return Err(ComparisonError::MissingRamEvidence {
            path: path.to_path_buf(),
        });
    }
    for (index, usage) in ram.iter().enumerate() {
        if ram[..index]
            .iter()
            .any(|prior| prior.backing_store == usage.backing_store)
        {
            return Err(ComparisonError::DuplicateRamBacking {
                path: path.to_path_buf(),
                backing_store: usage.backing_store.clone(),
            });
        }
        let reservations_valid = usage.included_reservation_bytes <= usage.static_section_bytes;
        let accounting_valid = match usage.capacity {
            RamCapacityIdentity::Known {
                bytes,
                headroom_bytes,
            } => {
                bytes != 0
                    && usage
                        .static_section_bytes
                        .checked_add(usage.linker_padding_bytes)
                        .and_then(|bytes| bytes.checked_add(usage.additional_reservation_bytes))
                        .and_then(|bytes| bytes.checked_add(usage.external_reservation_bytes))
                        .and_then(|bytes| bytes.checked_add(headroom_bytes))
                        == Some(bytes)
            }
            RamCapacityIdentity::RuntimeDetected => true,
        };
        let address_spaces_valid = !usage.address_spaces.is_empty()
            && usage
                .address_spaces
                .iter()
                .enumerate()
                .all(|(index, space)| {
                    !space.is_empty() && !usage.address_spaces[..index].contains(space)
                });
        if usage.backing_store.is_empty()
            || !address_spaces_valid
            || !reservations_valid
            || !accounting_valid
        {
            return Err(ComparisonError::InvalidRamAccounting {
                path: path.to_path_buf(),
                backing_store: usage.backing_store.clone(),
            });
        }
    }
    Ok(())
}

fn validate_sections(path: &Path, sections: &[SectionUsage]) -> Result<(), ComparisonError> {
    if sections.is_empty() {
        return Err(ComparisonError::MissingSectionEvidence {
            path: path.to_path_buf(),
        });
    }
    for section in sections {
        let valid = !section.name.is_empty()
            && section.run_end.checked_sub(section.run_address) == Some(section.run_bytes)
            && section.run_bytes != 0
            && section.alignment.is_power_of_two()
            && section.load_bytes <= section.run_bytes
            && (!matches!(section.kind, SectionKindIdentity::ZeroFill) || section.load_bytes == 0);
        if !valid {
            return Err(ComparisonError::InvalidSectionAccounting {
                path: path.to_path_buf(),
                section: section.name.clone(),
            });
        }
    }
    for (kind, name) in SECTION_KINDS {
        let valid = sections
            .iter()
            .filter(|section| section.kind == kind)
            .try_fold((0_u64, 0_u64), |(run, load), section| {
                Some((
                    run.checked_add(section.run_bytes)?,
                    load.checked_add(section.load_bytes)?,
                ))
            })
            .is_some();
        if !valid {
            return Err(ComparisonError::InvalidSectionAccounting {
                path: path.to_path_buf(),
                section: name.to_string(),
            });
        }
    }
    Ok(())
}

fn validate_attribution(
    path: &Path,
    attribution: &FlashAttributionIdentity,
) -> Result<(), ComparisonError> {
    validate_attribution_category(path, "crate", &attribution.crates)?;
    validate_attribution_category(path, "symbol", &attribution.symbols)?;
    let valid = attribution.crates.coverage.analyzed_bytes
        == attribution.symbols.coverage.analyzed_bytes
        && attribution.crates.coverage.attributed_bytes
            <= attribution.symbols.coverage.attributed_bytes;
    if valid {
        Ok(())
    } else {
        Err(ComparisonError::InvalidAttribution {
            path: path.to_path_buf(),
            category: "cross-category",
        })
    }
}

fn validate_attribution_category(
    path: &Path,
    category: &'static str,
    attribution: &AttributionCategoryIdentity,
) -> Result<(), ComparisonError> {
    let coverage = &attribution.coverage;
    let accounting_valid = coverage.analyzed_bytes > 0
        && coverage
            .attributed_bytes
            .checked_add(coverage.unclassified_bytes)
            == Some(coverage.analyzed_bytes);
    let entries_valid = attribution
        .largest
        .iter()
        .enumerate()
        .all(|(index, entry)| {
            let unique = !attribution.largest[..index]
                .iter()
                .any(|prior| prior.name == entry.name);
            let ranked = attribution.largest.get(index + 1).is_none_or(|next| {
                entry.bytes > next.bytes || (entry.bytes == next.bytes && entry.name < next.name)
            });
            !entry.name.is_empty()
                && entry.bytes > 0
                && entry.bytes <= coverage.attributed_bytes
                && unique
                && ranked
        });
    let ranked_bytes = attribution
        .largest
        .iter()
        .try_fold(0_u64, |sum, entry| sum.checked_add(entry.bytes));
    let presence_valid = (coverage.attributed_bytes == 0) == attribution.largest.is_empty();
    if accounting_valid
        && entries_valid
        && ranked_bytes.is_some_and(|bytes| bytes <= coverage.attributed_bytes)
        && presence_valid
    {
        Ok(())
    } else {
        Err(ComparisonError::InvalidAttribution {
            path: path.to_path_buf(),
            category,
        })
    }
}
