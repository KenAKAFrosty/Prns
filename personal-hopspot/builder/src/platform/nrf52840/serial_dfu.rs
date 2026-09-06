use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use prns_flash_manifest::{
    sha256_hex, BoardCatalogEntry, FlashPart, FlashPartKind, NrfDfuApplicationVersion,
    NrfSerialDfuBuild, NrfSerialDfuBuildCompatibility, NrfSerialDfuManifest,
    NrfSerialDfuRecoveryManifest,
};
use prns_nrf_dfu::{
    ApplicationInitPacket, ApplicationInitPacketSpec, ApplicationVersion, DfuDeviceRevision,
    DfuDeviceType, DfuImage, SoftdeviceFirmwareId, SoftdeviceRequirements,
};

use crate::artifact::publish;
use crate::{embedded_cargo_command, llvm_objcopy, run_status, BuildContext, BuildError};

#[derive(Debug)]
pub struct Output {
    elf: PathBuf,
    manifest: NrfSerialDfuManifest,
    application: Vec<u8>,
    init_packet: Vec<u8>,
    recovery: Vec<u8>,
}

impl Output {
    pub fn elf(&self) -> &Path {
        &self.elf
    }

    pub const fn manifest(&self) -> &NrfSerialDfuManifest {
        &self.manifest
    }

    pub fn application(&self) -> &[u8] {
        &self.application
    }

    pub fn init_packet(&self) -> &[u8] {
        &self.init_packet
    }

    pub fn recovery(&self) -> &[u8] {
        &self.recovery
    }

    pub fn into_transfer_artifacts(self) -> (Vec<u8>, Vec<u8>) {
        (self.application, self.init_packet)
    }
}

pub fn build(
    context: &BuildContext<'_>,
    board: &BoardCatalogEntry,
    recipe: &NrfSerialDfuBuild,
) -> Result<Output, BuildError> {
    let crate_dir = context
        .repository()
        .join("personal-hopspot")
        .join("embedded")
        .join("nrf52840");
    let target_directory = crate_dir.join(&recipe.target_directory);
    let mut cargo = embedded_cargo_command();
    cargo
        .arg("build")
        .arg("--release")
        .arg("--locked")
        .arg("--no-default-features")
        .arg("--features")
        .arg(&recipe.cargo_feature)
        .arg("--package")
        .arg(&recipe.package)
        .arg("--bin")
        .arg(&recipe.binary)
        .arg("--target")
        .arg(&recipe.rust_target)
        .arg("--target-dir")
        .arg(&target_directory)
        .env("PRNS_BUILD_VERSION", context.version())
        .current_dir(&crate_dir);
    run_status(&mut cargo, "Nordic serial DFU cargo build")?;

    let elf = target_directory
        .join(&recipe.rust_target)
        .join("release")
        .join(&recipe.binary);
    let work_dir = context.work_output(&board.slug);
    fs::create_dir_all(&work_dir).map_err(|error| {
        BuildError::Artifact(format!("could not create work directory: {error}"))
    })?;
    let application_path = work_dir.join(&recipe.application_filename);
    run_status(
        Command::new(llvm_objcopy()?.as_os_str())
            .arg("-O")
            .arg("binary")
            .arg(&elf)
            .arg(&application_path),
        "llvm-objcopy",
    )?;
    let application = fs::read(&application_path).map_err(|error| {
        BuildError::Artifact(format!(
            "could not read {}: {error}",
            application_path.display()
        ))
    })?;
    let memory = recipe
        .memory_layout()
        .map_err(|error| BuildError::Manifest(error.to_string()))?;
    let maximum_application_bytes = u64::from(memory.firmware_owned().byte_len());
    let application_bytes = application.len() as u64;
    if application_bytes > maximum_application_bytes {
        return Err(BuildError::FirmwareOverflow {
            target: board.display_name.clone(),
            actual: application_bytes,
            maximum: maximum_application_bytes,
        });
    }

    let init_packet_spec = init_packet_spec(&recipe.compatibility)?;
    let init_packet = ApplicationInitPacket::build(&application, &init_packet_spec)
        .map_err(|error| BuildError::Artifact(error.to_string()))?
        .bytes()
        .to_vec();
    let output_dir = context.board_output(&board.slug);
    publish(&output_dir.join(&recipe.application_filename), &application)?;
    publish(&output_dir.join(&recipe.init_packet_filename), &init_packet)?;

    let recovery_path = output_dir.join(&recipe.recovery.filename);
    let application_base = format!("0x{:08x}", memory.transport_envelope().start());
    run_status(
        Command::new(if cfg!(windows) { "python" } else { "python3" })
            .arg(
                context
                    .repository()
                    .join("tools")
                    .join("device")
                    .join("bin2uf2.py"),
            )
            .arg(&application_path)
            .arg(&recovery_path)
            .arg(&application_base)
            .arg(&recipe.recovery.family_id),
        "bin2uf2.py",
    )?;
    let recovery = fs::read(&recovery_path)
        .map_err(|error| BuildError::Artifact(format!("could not read recovery UF2: {error}")))?;
    let compatibility = recipe
        .manifest_compatibility()
        .map_err(|error| BuildError::Manifest(error.to_string()))?;
    let manifest = NrfSerialDfuManifest {
        serial: recipe.serial.clone(),
        compatibility,
        application: release_artifact(
            board,
            context,
            &recipe.application_filename,
            FlashPartKind::DfuApplication,
            &application,
        ),
        init_packet: release_artifact(
            board,
            context,
            &recipe.init_packet_filename,
            FlashPartKind::DfuInitPacket,
            &init_packet,
        ),
        recovery: NrfSerialDfuRecoveryManifest {
            mount_label: recipe.recovery.mount_label.clone(),
            board_id_prefix: recipe.recovery.board_identity.value.clone(),
            family_id: recipe.recovery.family_id.clone(),
            artifact: release_artifact(
                board,
                context,
                &recipe.recovery.filename,
                FlashPartKind::Uf2,
                &recovery,
            ),
        },
    };
    DfuImage::from_artifacts(&application, &init_packet, &init_packet_spec)
        .map_err(|error| BuildError::Artifact(error.to_string()))?;
    Ok(Output {
        elf,
        manifest,
        application,
        init_packet,
        recovery,
    })
}

fn init_packet_spec(
    compatibility: &NrfSerialDfuBuildCompatibility,
) -> Result<ApplicationInitPacketSpec, BuildError> {
    let fwid = SoftdeviceFirmwareId::new(parse_hex_u16("FWID", &compatibility.fwid)?)
        .map_err(|error| BuildError::Manifest(error.to_string()))?;
    Ok(ApplicationInitPacketSpec {
        device_type: DfuDeviceType::new(parse_hex_u16("device type", &compatibility.device_type)?),
        device_revision: DfuDeviceRevision::new(compatibility.device_revision),
        application_version: match compatibility.application_version {
            NrfDfuApplicationVersion::NotEnforced => ApplicationVersion::NotEnforced,
        },
        softdevices: SoftdeviceRequirements::new(fwid, std::iter::empty())
            .map_err(|error| BuildError::Manifest(error.to_string()))?,
    })
}

fn parse_hex_u16(label: &str, value: &str) -> Result<u16, BuildError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or_else(|| BuildError::Manifest(format!("invalid Nordic DFU {label} {value:?}")))?;
    u16::from_str_radix(digits, 16).map_err(|error| {
        BuildError::Manifest(format!("invalid Nordic DFU {label} {value:?}: {error}"))
    })
}

fn release_artifact(
    board: &BoardCatalogEntry,
    context: &BuildContext<'_>,
    filename: &str,
    kind: FlashPartKind,
    bytes: &[u8],
) -> FlashPart {
    FlashPart {
        kind,
        path: context.release_part_path(&board.slug, filename),
        offset: None,
        size: bytes.len() as u64,
        sha256: sha256_hex(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_hex_values_are_strictly_prefixed_and_bounded() -> Result<(), BuildError> {
        assert_eq!(parse_hex_u16("FWID", "0x00b6")?, 0x00b6);
        assert!(parse_hex_u16("FWID", "00b6").is_err());
        assert!(parse_hex_u16("FWID", "0x10000").is_err());
        Ok(())
    }
}
