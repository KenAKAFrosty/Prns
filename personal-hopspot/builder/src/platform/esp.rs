use std::fs;

use espflash::flasher::{FlashData, FlashFrequency, FlashMode, FlashSettings, FlashSize};
use espflash::image_format::{idf::IdfBootloaderFormat, ImageFormat};
use espflash::target::{Chip, XtalFrequency};
use prns_flash_manifest::{sha256_hex, BoardCatalogEntry, EspBuild, FlashPart, FlashPartKind};

use crate::architecture::adapter_for_rust_target;
use crate::{embedded_cargo_command, run_status, BuildContext, BuildError, FirmwareEvidence};

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
}

impl Output {
    pub const fn firmware(&self) -> &FirmwareEvidence {
        &self.firmware
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
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
    let application_offset = memory.transport_envelope().start();
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
    let capture = context.configure_firmware_cargo(memory.id().0, adapter, &mut cargo)?;
    run_status(&mut cargo, "embedded ESP cargo build")?;
    let firmware = context.finish_firmware_build(elf.clone(), capture)?;

    let elf_bytes = fs::read(&elf).map_err(|error| {
        BuildError::Artifact(format!("could not read {}: {error}", elf.display()))
    })?;
    let chip = recipe
        .chip
        .parse::<Chip>()
        .map_err(|error| BuildError::Build(format!("invalid chip {:?}: {error}", recipe.chip)))?;
    let flash_size = flash_size(board.flash_size)?;
    let flash_data = FlashData::new(
        FlashSettings::new(
            Some(FlashMode::Dio),
            Some(flash_size),
            Some(FlashFrequency::_40Mhz),
        ),
        0,
        None,
        chip,
        XtalFrequency::_40Mhz,
    );
    let image = IdfBootloaderFormat::new(
        &elf_bytes,
        &flash_data,
        Some(&partition_table),
        None,
        Some(PARTITION_TABLE_OFFSET),
        Some("factory"),
    )
    .map_err(|error| BuildError::Build(format!("could not construct sparse ESP image: {error}")))?;
    let mut parts = Vec::new();
    for segment in ImageFormat::from(image).flash_segments() {
        let (kind, filename) = part_identity(segment.addr, application_offset)?;
        let bytes = segment.data.into_owned();
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
    Ok(Output { firmware, parts })
}

fn flash_size(bytes: Option<u32>) -> Result<FlashSize, BuildError> {
    match bytes {
        Some(4_194_304) => Ok(FlashSize::_4Mb),
        Some(8_388_608) => Ok(FlashSize::_8Mb),
        Some(16_777_216) => Ok(FlashSize::_16Mb),
        other => Err(BuildError::Build(format!(
            "unsupported catalog flash size {other:?}"
        ))),
    }
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
    fn catalog_flash_sizes_map_to_supported_image_sizes() -> Result<(), BuildError> {
        assert_eq!(flash_size(Some(4_194_304))?, FlashSize::_4Mb);
        assert_eq!(flash_size(Some(8_388_608))?, FlashSize::_8Mb);
        assert_eq!(flash_size(Some(16_777_216))?, FlashSize::_16Mb);
        assert!(flash_size(None).is_err());
        assert!(flash_size(Some(2_097_152)).is_err());
        Ok(())
    }
}
