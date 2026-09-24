use std::fs;
use std::process::Command;

use prns_flash_manifest::{
    sha256_hex, validate_uf2_build_artifact, BoardCatalogEntry, Uf2ApplicationLink, Uf2Build,
    Uf2BuildVariant, Uf2VariantManifest,
};

use super::binary;
use crate::architecture::adapter_for_rust_target;
use crate::{
    embedded_cargo_command, run_status, BuildContext, BuildError, FirmwareEvidence, LtoMode,
};

#[derive(Debug)]
pub struct Output {
    firmware: FirmwareEvidence,
    descriptor: Uf2VariantManifest,
    bytes: Vec<u8>,
    firmware_image_bytes: u64,
}

impl Output {
    pub const fn firmware(&self) -> &FirmwareEvidence {
        &self.firmware
    }

    pub const fn descriptor(&self) -> &Uf2VariantManifest {
        &self.descriptor
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn firmware_image_bytes(&self) -> u64 {
        self.firmware_image_bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

pub fn build(
    context: &BuildContext<'_>,
    board: &BoardCatalogEntry,
    recipe: &Uf2Build,
    variant: &Uf2BuildVariant,
) -> Result<Output, BuildError> {
    let memory = variant
        .memory_layout()
        .map_err(|error| BuildError::Manifest(error.to_string()))?;
    let application = memory.firmware_owned();
    let application_base = format!("0x{:08x}", application.start());
    let crate_dir = context
        .repository()
        .join("personal-hopspot")
        .join("embedded")
        .join("nrf52840");
    let target_directory = context
        .cargo_target_directory(memory.id().0)
        .unwrap_or_else(|| crate_dir.join(&variant.target_directory));
    let elf = target_directory
        .join(&recipe.rust_target)
        .join("release")
        .join(&recipe.binary);
    let features = cargo_features(&recipe.board_feature, variant.application_link);
    let mut cargo = embedded_cargo_command();
    cargo
        .arg(context.cargo_subcommand())
        .arg("--release")
        .arg("--locked")
        .arg("--no-default-features")
        .arg("--bin")
        .arg(&recipe.binary)
        .arg("--features")
        .arg(features)
        .arg("--target-dir")
        .arg(&target_directory)
        .current_dir(&crate_dir);
    let adapter = adapter_for_rust_target(&recipe.rust_target)?;
    let capture = context.configure_firmware_cargo(
        memory.id().0,
        adapter,
        LtoMode::Configured,
        &mut cargo,
    )?;
    let firmware = context.run_firmware_build(
        &mut cargo,
        &format!("{} cargo build", board.display_name),
        memory.id().0,
        adapter,
        elf.clone(),
        capture,
    )?;

    let work_dir = context.work_output(&board.slug);
    let binary = work_dir.join(format!("{}.bin", variant.softdevice_version));
    let firmware_image_bytes = binary::extract(&elf, &binary)?;

    let output_dir = context.board_output(&board.slug);
    fs::create_dir_all(&output_dir).map_err(|error| {
        BuildError::Artifact(format!(
            "could not create {}: {error}",
            output_dir.display()
        ))
    })?;
    let uf2 = output_dir.join(&variant.filename);
    run_status(
        Command::new(if cfg!(windows) { "python" } else { "python3" })
            .arg(
                context
                    .repository()
                    .join("tools")
                    .join("device")
                    .join("bin2uf2.py"),
            )
            .arg(&binary)
            .arg(&uf2)
            .arg(&application_base)
            .arg(&variant.family_id),
        "bin2uf2.py",
    )?;
    let bytes = fs::read(&uf2)
        .map_err(|error| BuildError::Artifact(format!("could not read UF2: {error}")))?;
    validate_uf2_build_artifact(variant, &bytes).map_err(|error| {
        BuildError::Artifact(format!("built UF2 {} is invalid: {error}", uf2.display()))
    })?;
    let descriptor = Uf2VariantManifest {
        softdevice_family: variant.softdevice_family.clone(),
        softdevice_version: variant.softdevice_version.clone(),
        fwid: variant.fwid.clone(),
        application_base,
        family_id: variant.family_id.clone(),
        path: context.release_part_path(&board.slug, &variant.filename),
        size: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
    };
    Ok(Output {
        firmware,
        descriptor,
        bytes,
        firmware_image_bytes,
    })
}

fn cargo_features(board_feature: &str, application_link: Uf2ApplicationLink) -> String {
    application_link.cargo_feature().map_or_else(
        || board_feature.to_string(),
        |link_feature| format!("{board_feature},{link_feature}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_softdevice_is_added_to_board_features() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = prns_flash_manifest::board_catalog()?;
        let board = catalog.board("t-echo").ok_or("missing T-Echo")?;
        let prns_flash_manifest::BoardBuild::Uf2(recipe) = &board.build else {
            return Err("T-Echo does not use UF2".into());
        };
        let [v6, v7] = recipe.variants.as_slice() else {
            return Err("T-Echo does not have exactly two build variants".into());
        };

        assert_eq!(
            cargo_features(&recipe.board_feature, v6.application_link),
            "board-t-echo,softdevice-s140-v6"
        );
        assert_eq!(
            cargo_features(&recipe.board_feature, v7.application_link),
            "board-t-echo,softdevice-s140-v7"
        );
        Ok(())
    }
}
