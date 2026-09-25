use std::fs;
use std::path::Path;

use espflash::flasher::{FlashData, FlashFrequency, FlashMode, FlashSettings, FlashSize};
use espflash::image_format::{idf::IdfBootloaderFormat, ImageFormat};
use espflash::target::{Chip, XtalFrequency};
use prns_flash_manifest::{
    sha256_hex, validate_esp_sparse_image, BoardCatalogEntry, EspBuild, FlashPart, FlashPartKind,
};

use crate::architecture::adapter_for_rust_target;
use crate::{embedded_cargo_command, BuildContext, BuildError, FirmwareEvidence, LtoMode};

const PARTITION_TABLE_OFFSET: u32 = 0x8000;

#[derive(Debug)]
pub struct Part {
    descriptor: FlashPart,
    bytes: Vec<u8>,
}

impl Part {
    pub const fn descriptor(&self) -> &FlashPart {
        &self.descriptor
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Debug)]
pub struct Output {
    firmware: FirmwareEvidence,
    parts: Vec<Part>,
    firmware_image_bytes: u64,
}

impl Output {
    pub const fn firmware(&self) -> &FirmwareEvidence {
        &self.firmware
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    pub const fn firmware_image_bytes(&self) -> u64 {
        self.firmware_image_bytes
    }

    pub fn into_parts(self) -> Vec<Part> {
        self.parts
    }
}

pub fn build(
    context: &BuildContext<'_>,
    board: &BoardCatalogEntry,
    recipe: &EspBuild,
) -> Result<Output, BuildError> {
    let memory = recipe
        .memory_layout()
        .map_err(|error| BuildError::Manifest(error.to_string()))?;
    let application_offset = memory.firmware_owned().start();
    let crate_dir = context
        .repository()
        .join("personal-hopspot")
        .join("embedded")
        .join("esp32");
    let partition_table = crate_dir.join(&recipe.partition_table);
    let evidence_target_directory = context.cargo_target_directory(memory.id().0);
    let target_directory = evidence_target_directory
        .clone()
        .unwrap_or_else(|| crate_dir.join("target"));
    let elf = target_directory
        .join(&recipe.rust_target)
        .join("release")
        .join(&recipe.binary);
    let mut cargo = embedded_cargo_command();
    cargo
        .arg(context.cargo_subcommand())
        .arg("--release")
        .arg("--locked")
        .arg("--package")
        .arg(&recipe.package)
        .arg("--bin")
        .arg(&recipe.binary)
        .arg("--target")
        .arg(&recipe.rust_target)
        .arg("-Zbuild-std=core,alloc")
        .env("PRNS_BUILD_VERSION", context.version())
        .current_dir(&crate_dir);
    if let Some(target_directory) = evidence_target_directory {
        cargo.arg("--target-dir").arg(target_directory);
    }
    if let Some(source_digest) = context.source_digest() {
        cargo.env("PRNS_BUILD_SOURCE_DIGEST", source_digest);
    }
    let adapter = adapter_for_rust_target(&recipe.rust_target)?;
    let capture = context.configure_firmware_cargo(
        memory.id().0,
        adapter,
        LtoMode::Configured,
        &mut cargo,
    )?;
    let firmware = context.run_firmware_build(
        &mut cargo,
        "embedded ESP cargo build",
        memory.id().0,
        adapter,
        elf.clone(),
        capture,
    )?;

    let elf_bytes = fs::read(&elf).map_err(|error| {
        BuildError::Artifact(format!("could not read {}: {error}", elf.display()))
    })?;
    let chip = recipe
        .chip
        .parse::<Chip>()
        .map_err(|error| BuildError::Build(format!("invalid chip {:?}: {error}", recipe.chip)))?;
    let flash_size_bytes = board.flash_size.ok_or_else(|| {
        BuildError::Build("ESP catalog target does not declare its flash size".into())
    })?;
    let segments = sparse_segments(&elf_bytes, &partition_table, chip, flash_size_bytes)?;
    let mut parts = Vec::new();
    for segment in segments {
        let (kind, filename) = part_identity(segment.addr, application_offset)?;
        let bytes = segment.bytes;
        let descriptor = FlashPart {
            kind,
            path: context.release_part_path(&board.slug, filename),
            offset: Some(segment.addr),
            size: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        };
        parts.push(Part { descriptor, bytes });
    }
    parts.sort_by_key(|part| part.descriptor.offset);
    validate_esp_sparse_image(board, parts.iter().map(Part::descriptor)).map_err(|error| {
        BuildError::Artifact(format!("built sparse ESP image is invalid: {error}"))
    })?;
    let firmware_image_bytes = parts
        .iter()
        .find(|part| part.descriptor.kind == FlashPartKind::Application)
        .map(|part| part.descriptor.size)
        .ok_or_else(|| BuildError::Artifact("built sparse ESP image has no application".into()))?;
    Ok(Output {
        firmware,
        parts,
        firmware_image_bytes,
    })
}

pub struct MergedImageRequest<'a> {
    pub elf: &'a [u8],
    pub partition_table: &'a Path,
    pub chip: &'a str,
    pub flash_size_bytes: u32,
}

pub fn merged_flash_image(request: MergedImageRequest<'_>) -> Result<Vec<u8>, BuildError> {
    let chip = request
        .chip
        .parse::<Chip>()
        .map_err(|error| BuildError::Build(format!("invalid chip {:?}: {error}", request.chip)))?;
    let segments = sparse_segments(
        request.elf,
        request.partition_table,
        chip,
        request.flash_size_bytes,
    )?;
    merge_segments(request.flash_size_bytes, segments)
}

#[derive(Debug)]
struct SparseSegment {
    addr: u32,
    bytes: Vec<u8>,
}

fn sparse_segments(
    elf: &[u8],
    partition_table: &Path,
    chip: Chip,
    flash_size_bytes: u32,
) -> Result<Vec<SparseSegment>, BuildError> {
    let flash_data = FlashData::new(
        FlashSettings::new(
            Some(FlashMode::Dio),
            Some(flash_size(flash_size_bytes)?),
            Some(FlashFrequency::_40Mhz),
        ),
        0,
        None,
        chip,
        XtalFrequency::_40Mhz,
    );
    let app_partition = app_partition_name(partition_table)?;
    let image = IdfBootloaderFormat::new(
        elf,
        &flash_data,
        Some(partition_table),
        None,
        Some(PARTITION_TABLE_OFFSET),
        Some(&app_partition),
    )
    .map_err(|error| BuildError::Build(format!("could not construct sparse ESP image: {error}")))?;
    Ok(ImageFormat::from(image)
        .flash_segments()
        .into_iter()
        .map(|segment| SparseSegment {
            addr: segment.addr,
            bytes: segment.data.into_owned(),
        })
        .collect())
}

fn merge_segments(
    flash_size_bytes: u32,
    mut segments: Vec<SparseSegment>,
) -> Result<Vec<u8>, BuildError> {
    segments.sort_by_key(|segment| segment.addr);
    let mut previous_end = 0usize;
    let mut image = vec![0xff; flash_size_bytes as usize];
    for segment in segments {
        let start = segment.addr as usize;
        let end = start
            .checked_add(segment.bytes.len())
            .ok_or_else(|| BuildError::Artifact("ESP flash segment address overflow".into()))?;
        if start < previous_end {
            return Err(BuildError::Artifact(format!(
                "ESP flash segment at 0x{start:x} overlaps its predecessor"
            )));
        }
        let destination = image.get_mut(start..end).ok_or_else(|| {
            BuildError::Artifact(format!(
                "ESP flash segment 0x{start:x}..0x{end:x} exceeds {flash_size_bytes} bytes"
            ))
        })?;
        destination.copy_from_slice(&segment.bytes);
        previous_end = end;
    }
    Ok(image)
}

fn flash_size(bytes: u32) -> Result<FlashSize, BuildError> {
    match bytes {
        4_194_304 => Ok(FlashSize::_4Mb),
        8_388_608 => Ok(FlashSize::_8Mb),
        16_777_216 => Ok(FlashSize::_16Mb),
        other => Err(BuildError::Build(format!(
            "unsupported ESP flash size {other}"
        ))),
    }
}

/// Single-slot tables name the application `factory`. Dual-slot tables name the first
/// slot `ota_0`, which is where an empty boot-selection region starts.
fn app_partition_name(partition_table: &Path) -> Result<String, BuildError> {
    let text = fs::read_to_string(partition_table).map_err(|error| {
        BuildError::Build(format!(
            "could not read partition table {}: {error}",
            partition_table.display()
        ))
    })?;
    app_partition_name_from_csv(&text)
}

fn app_partition_name_from_csv(text: &str) -> Result<String, BuildError> {
    let mut names = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split(',');
        let name = fields.next().unwrap_or("").trim();
        let kind = fields.next().unwrap_or("").trim();
        if kind == "app" && !name.is_empty() {
            names.push(name.to_string());
        }
    }
    if names.iter().any(|name| name == "factory") {
        return Ok("factory".to_string());
    }
    if names.iter().any(|name| name == "ota_0") {
        return Ok("ota_0".to_string());
    }
    names
        .into_iter()
        .next()
        .ok_or_else(|| BuildError::Build("partition table has no app partition".to_string()))
}

fn part_identity(
    address: u32,
    application_offset: u32,
) -> Result<(FlashPartKind, &'static str), BuildError> {
    match address {
        PARTITION_TABLE_OFFSET => Ok((FlashPartKind::PartitionTable, "partition-table.bin")),
        address if address == application_offset => {
            Ok((FlashPartKind::Application, "application.bin"))
        }
        address if address < PARTITION_TABLE_OFFSET => {
            Ok((FlashPartKind::Bootloader, "bootloader.bin"))
        }
        address => Err(BuildError::Build(format!(
            "unexpected sparse ESP segment at 0x{address:x}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_segments_have_one_canonical_identity() -> Result<(), BuildError> {
        assert_eq!(
            part_identity(0, 0x10_000)?,
            (FlashPartKind::Bootloader, "bootloader.bin")
        );
        assert_eq!(
            part_identity(PARTITION_TABLE_OFFSET, 0x10_000)?,
            (FlashPartKind::PartitionTable, "partition-table.bin")
        );
        assert_eq!(
            part_identity(0x10_000, 0x10_000)?,
            (FlashPartKind::Application, "application.bin")
        );
        assert!(part_identity(0x20_000, 0x10_000).is_err());
        Ok(())
    }

    #[test]
    fn app_partition_name_prefers_factory_then_the_first_ota_slot() {
        assert_eq!(
            app_partition_name_from_csv("factory,app,factory,0x10000,0xe6d000,\n").unwrap(),
            "factory"
        );
        assert_eq!(
            app_partition_name_from_csv(
                "ota_0,app,ota_0,0x10000,0x730000,\nota_1,app,ota_1,0x740000,0x730000,\n"
            )
            .unwrap(),
            "ota_0"
        );
        assert!(app_partition_name_from_csv("nvs,data,nvs,0x9000,0x3000,\n").is_err());
    }

    #[test]
    fn catalog_flash_sizes_map_to_supported_image_sizes() -> Result<(), BuildError> {
        assert_eq!(flash_size(4_194_304)?, FlashSize::_4Mb);
        assert_eq!(flash_size(8_388_608)?, FlashSize::_8Mb);
        assert_eq!(flash_size(16_777_216)?, FlashSize::_16Mb);
        assert!(flash_size(2_097_152).is_err());
        Ok(())
    }

    #[test]
    fn sparse_segments_merge_with_erased_gaps() -> Result<(), BuildError> {
        let image = merge_segments(
            8,
            vec![
                SparseSegment {
                    addr: 4,
                    bytes: vec![3, 4],
                },
                SparseSegment {
                    addr: 1,
                    bytes: vec![1, 2],
                },
            ],
        )?;

        assert_eq!(image, vec![0xff, 1, 2, 0xff, 3, 4, 0xff, 0xff]);
        assert!(merge_segments(
            4,
            vec![SparseSegment {
                addr: 3,
                bytes: vec![1, 2],
            }],
        )
        .is_err());
        assert!(merge_segments(
            4,
            vec![
                SparseSegment {
                    addr: 1,
                    bytes: vec![1, 2],
                },
                SparseSegment {
                    addr: 2,
                    bytes: vec![3],
                },
            ],
        )
        .is_err());
        Ok(())
    }
}
