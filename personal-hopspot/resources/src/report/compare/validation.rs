use std::fs;
use std::path::Path;

use super::super::model::{
    ArtifactIdentity, AsyncMemoryIdentity, AttributionCategoryIdentity, BuildStatus, Evidence,
    ExecutableArchitectureIdentity, ExecutableIdentity, FirmwareFlashUsage,
    FlashAttributionIdentity, FunctionBoundaryIdentity, FunctionNormalizationIdentity,
    LoadPermissionIdentity, MemoryOverflowIdentity, RamBackingUsage, RamCapacityIdentity,
    ResourceReport, ScenarioFutureSizesIdentity, SectionKindIdentity, SectionUsage,
    StackAnalysisGapKindIdentity, StackFrameSourceIdentity, StackLimitIdentity,
    StartupAnchorRoleIdentity, TaskPoolAccountingIdentity, SCHEMA_VERSION,
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
    let executable = report.analysis.executable.complete().ok_or_else(|| {
        ComparisonError::MissingExecutableEvidence {
            path: path.to_path_buf(),
        }
    })?;
    let async_memory = report.analysis.async_memory.complete().ok_or_else(|| {
        ComparisonError::MissingAsyncMemoryEvidence {
            path: path.to_path_buf(),
        }
    })?;
    validate_flash(path, flash)?;
    validate_artifacts(path, artifacts)?;
    validate_ram(path, ram)?;
    validate_sections(path, sections)?;
    validate_attribution(path, attribution)?;
    validate_executable(path, report, executable)?;
    validate_async_memory(path, sections, async_memory)
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
    if let Evidence::Complete(executable) | Evidence::Partial(executable) =
        &report.analysis.executable
    {
        validate_executable(path, report, executable)?;
    }
    if let Evidence::Complete(async_memory) | Evidence::Partial(async_memory) =
        &report.analysis.async_memory
    {
        let sections = match &report.analysis.allocated_sections {
            Evidence::Complete(sections) | Evidence::Partial(sections) => sections.as_slice(),
            Evidence::Unavailable => &[],
        };
        validate_async_memory(path, sections, async_memory)?;
    }
    Ok(())
}

fn validate_executable(
    path: &Path,
    report: &ResourceReport,
    executable: &ExecutableIdentity,
) -> Result<(), ComparisonError> {
    executable_valid(
        path,
        executable.rust_target == report.architecture.rust_target,
        "Rust target does not match report architecture",
    )?;
    executable_valid(
        path,
        matches!(
            executable.byte_order,
            super::super::model::ByteOrderIdentity::Little
        ),
        "ELF identity is incomplete or unsupported",
    )?;
    executable_valid(
        path,
        !executable.load_segments.is_empty(),
        "load segment inventory is empty",
    )?;
    for segment in &executable.load_segments {
        let ranges_valid = segment.run_end.checked_sub(segment.run_address)
            == Some(segment.memory_bytes)
            && segment.load_end.checked_sub(segment.load_address) == Some(segment.memory_bytes);
        let permissions_valid = !segment.permissions.is_empty()
            && segment
                .permissions
                .iter()
                .enumerate()
                .all(|(index, permission)| !segment.permissions[..index].contains(permission));
        executable_valid(
            path,
            ranges_valid
                && segment.memory_bytes != 0
                && segment.file_bytes <= segment.memory_bytes
                && segment.alignment.is_power_of_two()
                && permissions_valid,
            "load segment accounting is invalid",
        )?;
    }
    executable_valid(
        path,
        executable.load_segments.iter().any(|segment| {
            segment
                .permissions
                .contains(&LoadPermissionIdentity::Execute)
                && segment.run_address <= executable.entry_point
                && executable.entry_point < segment.run_end
        }),
        "entry point is outside executable load segments",
    )?;
    executable_valid(
        path,
        !executable.executable_sections.is_empty(),
        "executable section inventory is empty",
    )?;
    for (index, section) in executable.executable_sections.iter().enumerate() {
        executable_valid(
            path,
            !section.name.is_empty()
                && section.end.checked_sub(section.address) == Some(section.bytes)
                && section.bytes != 0
                && section.alignment.is_power_of_two()
                && !executable.executable_sections[..index]
                    .iter()
                    .any(|prior| prior.address < section.end && section.address < prior.end),
            "executable section accounting is invalid",
        )?;
    }
    executable_valid(
        path,
        !executable.startup.entry_section.is_empty()
            && !executable.startup.entry_symbol.is_empty()
            && executable
                .executable_sections
                .iter()
                .any(|section| section.name == executable.startup.entry_section),
        "startup entry identity is invalid",
    )?;
    let entry_anchors = executable
        .startup
        .anchors
        .iter()
        .filter(|anchor| anchor.role == StartupAnchorRoleIdentity::EntryPoint)
        .collect::<Vec<_>>();
    executable_valid(
        path,
        entry_anchors.len() == 1
            && entry_anchors[0].address == executable.entry_point
            && entry_anchors[0].section == executable.startup.entry_section
            && executable
                .startup
                .anchors
                .iter()
                .enumerate()
                .all(|(index, anchor)| {
                    !anchor.section.is_empty()
                        && !executable.startup.anchors[..index]
                            .iter()
                            .any(|prior| prior.role == anchor.role)
                }),
        "startup anchors are invalid",
    )?;
    validate_functions(path, &report.target.id, executable)?;
    let executable_bytes = executable
        .executable_sections
        .iter()
        .try_fold(0_u64, |total, section| total.checked_add(section.bytes));
    executable_valid(
        path,
        executable_bytes == Some(executable.disassembly.executable_bytes)
            && executable
                .disassembly
                .decoded_bytes
                .checked_add(executable.disassembly.undecoded_bytes)
                == Some(executable.disassembly.executable_bytes)
            && executable.disassembly.decoded_bytes != 0
            && executable.disassembly.instruction_count != 0
            && executable.disassembly.adapter == executable.architecture
            && !executable.disassembly.program.is_empty()
            && !executable.disassembly.version.is_empty(),
        "disassembly accounting is invalid",
    )?;
    validate_stack(path, report, executable)
}

fn validate_stack(
    path: &Path,
    report: &ResourceReport,
    executable: &ExecutableIdentity,
) -> Result<(), ComparisonError> {
    let (stack, partial) = match &executable.stack {
        Evidence::Complete(stack) => (stack, false),
        Evidence::Partial(stack) => (stack, true),
        Evidence::Unavailable => {
            return executable_valid(path, false, "stack evidence is unavailable");
        }
    };
    let gaps_valid = stack.gaps.iter().enumerate().all(|(index, gap)| {
        gap.occurrences != 0
            && !stack.gaps[..index]
                .iter()
                .any(|prior| prior.kind == gap.kind)
    });
    executable_valid(
        path,
        partial == !stack.gaps.is_empty() && gaps_valid,
        "stack evidence availability does not match its gaps",
    )?;
    executable_valid(
        path,
        stack.frame_source == expected_stack_frame_source(executable.architecture)
            && stack.source_bytes != 0
            && stack.frame_count != 0
            && stack.functions_without_frames <= executable.functions.boundary_count
            && stack.largest_frames.len() <= 20
            && !stack.largest_frames.is_empty()
            && u64::try_from(stack.largest_frames.len())
                .is_ok_and(|count| count <= stack.frame_count)
            && stack
                .largest_frames
                .iter()
                .enumerate()
                .all(|(index, frame)| {
                    !frame.name.is_empty()
                        && executable_address(executable, frame.address)
                        && stack.largest_frames.get(index + 1).is_none_or(|next| {
                            frame.bytes > next.bytes
                                || (frame.bytes == next.bytes && frame.name < next.name)
                                || (frame.bytes == next.bytes
                                    && frame.name == next.name
                                    && frame.address <= next.address)
                        })
                }),
        "stack frame accounting is invalid",
    )?;
    executable_valid(
        path,
        !stack.roots.is_empty()
            && stack.roots.iter().enumerate().all(|(index, root)| {
                !root.name.is_empty()
                    && executable_address(executable, root.address)
                    && !stack.roots[..index].iter().any(|prior| prior == root)
            }),
        "stack roots are invalid",
    )?;
    let path_bytes = stack
        .largest_known_path
        .frames
        .iter()
        .try_fold(0_u64, |total, frame| total.checked_add(frame.frame_bytes));
    executable_valid(
        path,
        !stack.largest_known_path.frames.is_empty()
            && path_bytes == Some(stack.largest_known_path.bytes)
            && stack.largest_known_path.frames.iter().all(|frame| {
                !frame.name.is_empty() && executable_address(executable, frame.address)
            }),
        "known stack path accounting is invalid",
    )?;
    let undeclared_gap = stack
        .gaps
        .iter()
        .any(|gap| gap.kind == StackAnalysisGapKindIdentity::StackLimitUndeclared);
    let limit_valid = match &stack.limit {
        StackLimitIdentity::Declared {
            reservation,
            bytes,
            headroom_bytes,
        } => {
            !reservation.is_empty()
                && !undeclared_gap
                && stack.largest_known_path.bytes.checked_add(*headroom_bytes) == Some(*bytes)
                && report
                    .memory_contract
                    .runtime_reservations
                    .iter()
                    .any(|candidate| candidate.id == *reservation && candidate.bytes == *bytes)
        }
        StackLimitIdentity::Undeclared => undeclared_gap,
    };
    executable_valid(path, limit_valid, "stack limit accounting is invalid")?;
    executable_valid(
        path,
        stack.artifact.path == format!("work/{}/stack-evidence.json", report.target.id)
            && stack.artifact.bytes != 0,
        "stack evidence artifact is invalid",
    )
}

const fn expected_stack_frame_source(
    architecture: ExecutableArchitectureIdentity,
) -> StackFrameSourceIdentity {
    match architecture {
        ExecutableArchitectureIdentity::Thumbv7em => StackFrameSourceIdentity::LlvmStackSizes,
        ExecutableArchitectureIdentity::Riscv32imac
        | ExecutableArchitectureIdentity::XtensaEsp32s3 => {
            StackFrameSourceIdentity::DwarfDebugFrame
        }
    }
}

fn executable_address(executable: &ExecutableIdentity, address: u64) -> bool {
    executable
        .executable_sections
        .iter()
        .any(|section| section.address <= address && address < section.end)
}

fn validate_async_memory(
    path: &Path,
    sections: &[SectionUsage],
    async_memory: &AsyncMemoryIdentity,
) -> Result<(), ComparisonError> {
    let pools_valid = !async_memory.task_pools.is_empty()
        && async_memory
            .task_pools
            .iter()
            .enumerate()
            .all(|(index, pool)| {
                let end = pool.address.checked_add(pool.bytes);
                !pool.task.is_empty()
                    && pool.bytes != 0
                    && !pool.section.is_empty()
                    && !async_memory.task_pools[..index]
                        .iter()
                        .any(|prior| prior.task == pool.task || prior.address == pool.address)
                    && sections.iter().any(|section| {
                        section.name == pool.section
                            && section.kind == SectionKindIdentity::ZeroFill
                            && section.run_address <= pool.address
                            && end.is_some_and(|end| end <= section.run_end)
                    })
            });
    let total = async_memory
        .task_pools
        .iter()
        .try_fold(0_u64, |total, pool| total.checked_add(pool.bytes));
    let futures_valid = match &async_memory.scenario_futures {
        ScenarioFutureSizesIdentity::Measured { futures } => {
            !futures.is_empty()
                && futures.iter().enumerate().all(|(index, future)| {
                    !future.scenario.is_empty()
                        && future.bytes != 0
                        && !futures[..index]
                            .iter()
                            .any(|prior| prior.scenario == future.scenario)
                })
        }
        ScenarioFutureSizesIdentity::Unavailable { .. } => true,
    };
    if async_memory.task_pool_accounting == TaskPoolAccountingIdentity::IncludedInStaticRam
        && pools_valid
        && total == Some(async_memory.task_pool_bytes)
        && futures_valid
    {
        Ok(())
    } else {
        Err(ComparisonError::InvalidAsyncMemoryEvidence {
            path: path.to_path_buf(),
        })
    }
}

fn validate_functions(
    path: &Path,
    target_id: &str,
    executable: &ExecutableIdentity,
) -> Result<(), ComparisonError> {
    let functions = &executable.functions;
    let executable_bytes = executable
        .executable_sections
        .iter()
        .try_fold(0_u64, |total, section| total.checked_add(section.bytes));
    executable_valid(
        path,
        functions.normalization == FunctionNormalizationIdentity::LinkedFunctionBodySha256V1
            && functions.boundary_count != 0
            && functions.boundaries_artifact.path
                == format!("work/{target_id}/function-boundaries.json")
            && functions.boundaries_artifact.bytes != 0
            && functions
                .classified_bytes
                .checked_add(functions.unclassified_bytes)
                == executable_bytes,
        "function coverage is invalid",
    )?;
    executable_valid(
        path,
        functions.largest.len() <= 20
            && !functions.largest.is_empty()
            && u64::try_from(functions.largest.len())
                .is_ok_and(|count| count <= functions.boundary_count)
            && functions
                .largest
                .iter()
                .enumerate()
                .all(|(index, boundary)| {
                    let ranked = functions.largest.get(index + 1).is_none_or(|next| {
                        boundary.bytes > next.bytes
                            || (boundary.bytes == next.bytes && boundary.name < next.name)
                    });
                    ranked && valid_function_boundary(executable, boundary)
                }),
        "largest-function ranking is invalid",
    )
}

fn valid_function_boundary(
    executable: &ExecutableIdentity,
    boundary: &FunctionBoundaryIdentity,
) -> bool {
    !boundary.name.is_empty()
        && boundary.end.checked_sub(boundary.address) == Some(boundary.bytes)
        && boundary.bytes != 0
        && executable
            .executable_sections
            .iter()
            .any(|section| section.address <= boundary.address && boundary.end <= section.end)
}

fn executable_valid(path: &Path, valid: bool, reason: &'static str) -> Result<(), ComparisonError> {
    if valid {
        Ok(())
    } else {
        Err(ComparisonError::InvalidExecutableEvidence {
            path: path.to_path_buf(),
            reason,
        })
    }
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
