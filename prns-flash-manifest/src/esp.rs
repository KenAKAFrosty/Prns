use std::collections::{BTreeMap, BTreeSet};

use personal_hopspot_memory::RegionRole;
use thiserror::Error;

use crate::{
    ApplicationAddressRange, BoardBuild, BoardCatalogEntry, DomainValueError, FlashPart,
    FlashPartKind, ImmutableArtifactPath, MemoryProfileReferenceError, Sha256Digest, CONFIG_OFFSET,
    CONFIG_SIZE, ESP_FLASH_SECTOR_SIZE,
};

const REQUIRED_PARTS: [FlashPartKind; 3] = [
    FlashPartKind::Bootloader,
    FlashPartKind::PartitionTable,
    FlashPartKind::Application,
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EspSparseImageError {
    #[error("board {board:?} does not have an ESP build")]
    Build { board: String },
    #[error("ESP board {board:?} has no physical flash size")]
    MissingFlashSize { board: String },
    #[error(transparent)]
    MemoryProfile(#[from] MemoryProfileReferenceError),
    #[error("ESP build has no sparse parts")]
    MissingParts,
    #[error("ESP sparse parts must be ordered {REQUIRED_PARTS:?}, found {actual:?}")]
    PartOrder { actual: Vec<FlashPartKind> },
    #[error("ESP part {path:?}: {violation}")]
    Part {
        path: String,
        violation: EspPartViolation,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EspPartViolation {
    #[error("size must be nonzero")]
    Empty,
    #[error("path is not immutable and relative: {0}")]
    Path(DomainValueError),
    #[error("SHA-256 is invalid: {0}")]
    Digest(DomainValueError),
    #[error("artifact path is duplicated")]
    DuplicatePath,
    #[error("part kind {0:?} is not valid for an ESP image")]
    Kind(FlashPartKind),
    #[error("offset is missing")]
    MissingOffset,
    #[error("size {0} cannot be represented in the ESP address space")]
    Size(u64),
    #[error("offset 0x{0:x} is not aligned to the 4 KiB flash erase sector")]
    Alignment(u32),
    #[error("sector-rounded erase footprint overflows the ESP address space")]
    EraseOverflow,
    #[error("sector-rounded erase end 0x{erase_end:x} exceeds physical flash 0x{flash_size:x}")]
    PhysicalFlash { erase_end: u32, flash_size: u32 },
    #[error(
        "sector-rounded range 0x{offset:x}..0x{erase_end:x} is not the assigned profile region"
    )]
    Region { offset: u32, erase_end: u32 },
    #[error("sector-rounded erase footprint overlaps the provisioning slot")]
    Provisioning,
    #[error("sector-rounded erase footprint overlaps {other:?}")]
    Overlap { other: String },
}

pub fn validate_esp_sparse_image<'a>(
    board: &BoardCatalogEntry,
    parts: impl IntoIterator<Item = &'a FlashPart>,
) -> Result<(), EspSparseImageError> {
    let BoardBuild::Esp(build) = &board.build else {
        return Err(EspSparseImageError::Build {
            board: board.slug.clone(),
        });
    };
    let flash_size = board
        .flash_size
        .ok_or_else(|| EspSparseImageError::MissingFlashSize {
            board: board.slug.clone(),
        })?;
    let memory = build.memory_layout()?;
    let parts = parts.into_iter().collect::<Vec<_>>();
    if parts.is_empty() {
        return Err(EspSparseImageError::MissingParts);
    }
    let actual = parts.iter().map(|part| part.kind).collect::<Vec<_>>();
    if actual != REQUIRED_PARTS {
        return Err(EspSparseImageError::PartOrder { actual });
    }
    let mut ranges = BTreeMap::<u32, (u32, &str)>::new();
    let mut paths = BTreeSet::new();
    for part in parts {
        validate_part(part, memory, flash_size, &mut paths, &mut ranges)?;
    }
    Ok(())
}

fn validate_part<'a>(
    part: &'a FlashPart,
    memory: crate::ResolvedMemoryProfile,
    flash_size: u32,
    paths: &mut BTreeSet<&'a str>,
    ranges: &mut BTreeMap<u32, (u32, &'a str)>,
) -> Result<(), EspSparseImageError> {
    if part.size == 0 {
        return Err(part_error(part, EspPartViolation::Empty));
    }
    ImmutableArtifactPath::parse(part.path.clone())
        .map_err(|error| part_error(part, EspPartViolation::Path(error)))?;
    Sha256Digest::parse(part.sha256.clone())
        .map_err(|error| part_error(part, EspPartViolation::Digest(error)))?;
    if !paths.insert(&part.path) {
        return Err(part_error(part, EspPartViolation::DuplicatePath));
    }
    let assigned_region = match part.kind {
        FlashPartKind::Bootloader => memory.region_for_role(RegionRole::Bootloader),
        FlashPartKind::PartitionTable => memory.region_for_role(RegionRole::PartitionTable),
        FlashPartKind::Application => Ok(memory.firmware_owned()),
        kind @ (FlashPartKind::Uf2
        | FlashPartKind::DfuApplication
        | FlashPartKind::DfuInitPacket) => {
            return Err(part_error(part, EspPartViolation::Kind(kind)));
        }
    }?;
    validate_range(part, assigned_region, flash_size, ranges)
}

fn validate_range<'a>(
    part: &'a FlashPart,
    assigned_region: ApplicationAddressRange,
    flash_size: u32,
    ranges: &mut BTreeMap<u32, (u32, &'a str)>,
) -> Result<(), EspSparseImageError> {
    let offset = part
        .offset
        .ok_or_else(|| part_error(part, EspPartViolation::MissingOffset))?;
    let size = u32::try_from(part.size)
        .map_err(|_| part_error(part, EspPartViolation::Size(part.size)))?;
    if !offset.is_multiple_of(ESP_FLASH_SECTOR_SIZE) {
        return Err(part_error(part, EspPartViolation::Alignment(offset)));
    }
    let erase_size = size
        .checked_add(ESP_FLASH_SECTOR_SIZE - 1)
        .map(|rounded| rounded / ESP_FLASH_SECTOR_SIZE * ESP_FLASH_SECTOR_SIZE)
        .ok_or_else(|| part_error(part, EspPartViolation::EraseOverflow))?;
    let erase_end = offset
        .checked_add(erase_size)
        .ok_or_else(|| part_error(part, EspPartViolation::EraseOverflow))?;
    if erase_end > flash_size {
        return Err(part_error(
            part,
            EspPartViolation::PhysicalFlash {
                erase_end,
                flash_size,
            },
        ));
    }
    if offset != assigned_region.start() || !assigned_region.contains(offset, erase_end) {
        return Err(part_error(
            part,
            EspPartViolation::Region { offset, erase_end },
        ));
    }
    let provisioning_end = CONFIG_OFFSET + CONFIG_SIZE as u32;
    if offset < provisioning_end && CONFIG_OFFSET < erase_end {
        return Err(part_error(part, EspPartViolation::Provisioning));
    }
    if let Some((_, (previous_end, previous_path))) = ranges.range(..=offset).next_back() {
        if *previous_end > offset {
            return Err(part_error(
                part,
                EspPartViolation::Overlap {
                    other: (*previous_path).to_string(),
                },
            ));
        }
    }
    if let Some((next_offset, (_, next_path))) = ranges.range(offset..).next() {
        if erase_end > *next_offset {
            return Err(part_error(
                part,
                EspPartViolation::Overlap {
                    other: (*next_path).to_string(),
                },
            ));
        }
    }
    ranges.insert(offset, (erase_end, &part.path));
    Ok(())
}

fn part_error(part: &FlashPart, violation: EspPartViolation) -> EspSparseImageError {
    EspSparseImageError::Part {
        path: part.path.clone(),
        violation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_artifacts_are_checked_against_the_esp_memory_contract(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let catalog = crate::board_catalog()?;
        let board = catalog.board("heltec-v4").ok_or("missing Heltec V4")?;
        let crate::BoardBuild::Esp(build) = &board.build else {
            return Err("Heltec V4 does not use an ESP build".into());
        };
        let memory = build.memory_layout()?;
        let mut parts = [
            part(
                FlashPartKind::Bootloader,
                memory.region_for_role(RegionRole::Bootloader)?.start(),
                "bootloader.bin",
            ),
            part(
                FlashPartKind::PartitionTable,
                memory.region_for_role(RegionRole::PartitionTable)?.start(),
                "partition-table.bin",
            ),
            part(
                FlashPartKind::Application,
                memory.firmware_owned().start(),
                "application.bin",
            ),
        ];
        assert_eq!(validate_esp_sparse_image(board, &parts), Ok(()));

        parts[2].size = u64::from(memory.firmware_owned().byte_len()) + 1;
        assert!(matches!(
            validate_esp_sparse_image(board, &parts),
            Err(EspSparseImageError::Part {
                violation: EspPartViolation::Region { .. },
                ..
            })
        ));
        Ok(())
    }

    fn part(kind: FlashPartKind, offset: u32, path: &str) -> FlashPart {
        FlashPart {
            kind,
            path: path.to_string(),
            offset: Some(offset),
            size: 1,
            sha256: "a".repeat(64),
        }
    }
}
