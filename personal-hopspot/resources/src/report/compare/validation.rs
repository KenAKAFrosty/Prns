use std::fs;
use std::path::Path;

use super::super::model::{
    ArtifactIdentity, RamBackingUsage, RamCapacityIdentity, ResourceReport, SectionKindIdentity,
    SCHEMA_VERSION,
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
    validate_flash(path, report)?;
    validate_artifacts(path, &report.artifacts)?;
    validate_ram(path, &report.static_ram)?;
    validate_sections(path, report)
}

fn validate_flash(path: &Path, report: &ResourceReport) -> Result<(), ComparisonError> {
    let flash = &report.firmware_flash;
    let valid = flash
        .end
        .checked_sub(flash.start)
        .filter(|capacity| *capacity != 0)
        .and_then(|capacity| {
            flash
                .image_bytes
                .checked_add(flash.headroom_bytes)
                .map(|used| (capacity, used))
        })
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
    if artifacts.is_empty() {
        return Err(ComparisonError::MissingArtifactEvidence {
            path: path.to_path_buf(),
        });
    }
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

fn validate_sections(path: &Path, report: &ResourceReport) -> Result<(), ComparisonError> {
    if report.analysis.linker_map_bytes == 0 {
        return Err(ComparisonError::MissingLinkerMapEvidence {
            path: path.to_path_buf(),
        });
    }
    if report.analysis.allocated_sections.is_empty() {
        return Err(ComparisonError::MissingSectionEvidence {
            path: path.to_path_buf(),
        });
    }
    for section in &report.analysis.allocated_sections {
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
        let valid = report
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
