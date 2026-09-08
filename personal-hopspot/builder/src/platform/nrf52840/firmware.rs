use personal_hopspot_memory::MemoryProfile;

use super::binary;
use crate::architecture::adapter_for_rust_target;
use crate::{embedded_cargo_command, BuildContext, BuildError, FirmwareEvidence};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recipe<'a> {
    pub package: &'a str,
    pub binary: &'a str,
    pub rust_target: &'a str,
    pub cargo_features: &'a str,
}

#[derive(Debug)]
pub struct Output {
    firmware: FirmwareEvidence,
    firmware_image_bytes: u64,
}

impl Output {
    pub const fn firmware(&self) -> &FirmwareEvidence {
        &self.firmware
    }

    pub const fn firmware_image_bytes(&self) -> u64 {
        self.firmware_image_bytes
    }
}

pub fn build(
    context: &BuildContext<'_>,
    profile: &MemoryProfile,
    recipe: Recipe<'_>,
) -> Result<Output, BuildError> {
    let target_id = profile.id.as_str();
    let crate_dir = context
        .repository()
        .join("personal-hopspot")
        .join("embedded")
        .join("nrf52840");
    let target_directory = context
        .cargo_target_directory(target_id)
        .unwrap_or_else(|| context.work_output(target_id).join("cargo"));
    let elf = target_directory
        .join(recipe.rust_target)
        .join("release")
        .join(recipe.binary);
    let mut cargo = embedded_cargo_command();
    cargo
        .arg(context.cargo_subcommand())
        .arg("--release")
        .arg("--locked")
        .arg("--no-default-features")
        .arg("--features")
        .arg(recipe.cargo_features)
        .arg("--package")
        .arg(recipe.package)
        .arg("--bin")
        .arg(recipe.binary)
        .arg("--target")
        .arg(recipe.rust_target)
        .arg("--target-dir")
        .arg(&target_directory)
        .env("PRNS_BUILD_VERSION", context.version())
        .current_dir(crate_dir);
    let adapter = adapter_for_rust_target(recipe.rust_target)?;
    let capture = context.configure_firmware_cargo(target_id, adapter, &mut cargo)?;
    let firmware = context.run_firmware_build(
        &mut cargo,
        &format!("{target_id} cargo build"),
        target_id,
        adapter,
        elf.clone(),
        capture,
    )?;
    let binary = context.work_output(target_id).join("firmware.bin");
    let firmware_image_bytes = binary::extract(&elf, &binary)?;
    Ok(Output {
        firmware,
        firmware_image_bytes,
    })
}
