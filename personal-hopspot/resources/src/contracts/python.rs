use std::fmt::Write;
use std::path::Path;

use personal_hopspot_memory::RegionRole;
use prns_flash_manifest::{
    board_catalog, BoardBuild, CatalogError, MemoryProfileReferenceError, NrfDfuApplicationVersion,
    NrfDfuBankLayout,
};
use thiserror::Error;

use super::RenderedArtifact;

pub(super) const RELATIVE_PATH: &str = "tools/release/flasher_memory_contracts.py";

#[derive(Debug, Error)]
pub(crate) enum PythonContractError {
    #[error("the board catalog is invalid: {0}")]
    Catalog(#[from] CatalogError),
    #[error("board {board:?} has an invalid memory profile: {source}")]
    MemoryProfile {
        board: String,
        #[source]
        source: MemoryProfileReferenceError,
    },
    #[error("the generated Python contract could not be formatted")]
    Format(#[from] std::fmt::Error),
    #[error("a catalog value could not be encoded as a Python string: {0}")]
    StringEncoding(#[from] serde_json::Error),
}

pub(super) fn render(root: &Path) -> Result<RenderedArtifact, PythonContractError> {
    let catalog = board_catalog()?;
    let mut contents = String::new();
    write_esp_contracts(&mut contents, &catalog.boards)?;
    writeln!(contents)?;
    write_uf2_contracts(&mut contents, &catalog.boards)?;
    writeln!(contents)?;
    write_nrf_serial_dfu_contracts(&mut contents, &catalog.boards)?;
    Ok(RenderedArtifact {
        path: root.join(RELATIVE_PATH),
        contents,
    })
}

fn write_esp_contracts(
    output: &mut String,
    boards: &[prns_flash_manifest::BoardCatalogEntry],
) -> Result<(), PythonContractError> {
    writeln!(output, "ESP_MEMORY_CONTRACTS = {{")?;
    for board in boards {
        let BoardBuild::Esp(build) = &board.build else {
            continue;
        };
        let memory = build
            .memory_layout()
            .map_err(|source| invalid_profile(&board.slug, source))?;
        writeln!(output, "    {}: {{", quote(&board.slug)?)?;
        writeln!(
            output,
            "        \"profile\": {},",
            quote(memory.id().as_str())?
        )?;
        writeln!(output, "        \"regions\": {{")?;
        for (kind, role) in [
            ("application", RegionRole::FirmwareImage),
            ("bootloader", RegionRole::Bootloader),
            ("partition-table", RegionRole::PartitionTable),
        ] {
            let region = memory
                .region_for_role(role)
                .map_err(|source| invalid_profile(&board.slug, source))?;
            writeln!(
                output,
                "            {}: ({:#010x}, {:#010x}),",
                quote(kind)?,
                region.start(),
                region.end_exclusive()
            )?;
        }
        writeln!(output, "        }},")?;
        writeln!(output, "    }},")?;
    }
    writeln!(output, "}}")?;
    Ok(())
}

fn write_uf2_contracts(
    output: &mut String,
    boards: &[prns_flash_manifest::BoardCatalogEntry],
) -> Result<(), PythonContractError> {
    writeln!(output, "UF2_MEMORY_CONTRACTS = {{")?;
    for board in boards {
        let BoardBuild::Uf2(build) = &board.build else {
            continue;
        };
        for variant in &build.variants {
            let memory = variant
                .memory_layout()
                .map_err(|source| invalid_profile(&board.slug, source))?;
            let firmware = memory.firmware_owned();
            let transport = memory.transport_envelope();
            writeln!(
                output,
                "    ({}, {}, {}, {}, {:#010x}, {}): {{",
                quote(&board.slug)?,
                quote(&variant.softdevice_family)?,
                quote(&variant.softdevice_version)?,
                quote(&variant.fwid)?,
                transport.start(),
                quote(&variant.family_id)?
            )?;
            writeln!(
                output,
                "        \"profile\": {},",
                quote(memory.id().as_str())?
            )?;
            writeln!(
                output,
                "        \"firmware_owned\": ({:#010x}, {:#010x}),",
                firmware.start(),
                firmware.end_exclusive()
            )?;
            writeln!(
                output,
                "        \"transport_envelope\": ({:#010x}, {:#010x}),",
                transport.start(),
                transport.end_exclusive()
            )?;
            writeln!(output, "    }},")?;
        }
    }
    writeln!(output, "}}")?;
    Ok(())
}

fn write_nrf_serial_dfu_contracts(
    output: &mut String,
    boards: &[prns_flash_manifest::BoardCatalogEntry],
) -> Result<(), PythonContractError> {
    writeln!(output, "NRF_SERIAL_DFU_MEMORY_CONTRACTS = {{")?;
    for board in boards {
        let BoardBuild::NrfSerialDfu(build) = &board.build else {
            continue;
        };
        let memory = build
            .memory_layout()
            .map_err(|source| invalid_profile(&board.slug, source))?;
        let compatibility = build
            .manifest_compatibility()
            .map_err(|source| invalid_profile(&board.slug, source))?;
        let firmware = memory.firmware_owned();
        let transport = memory.transport_envelope();
        writeln!(output, "    {}: {{", quote(&board.slug)?)?;
        writeln!(
            output,
            "        \"profile\": {},",
            quote(memory.id().as_str())?
        )?;
        writeln!(output, "        \"compatibility\": {{")?;
        writeln!(
            output,
            "            \"softdevice_family\": {},",
            quote(&compatibility.softdevice_family)?
        )?;
        writeln!(
            output,
            "            \"softdevice_version\": {},",
            quote(&compatibility.softdevice_version)?
        )?;
        writeln!(
            output,
            "            \"fwid\": {},",
            quote(&compatibility.fwid)?
        )?;
        writeln!(
            output,
            "            \"device_type\": {},",
            quote(&compatibility.device_type)?
        )?;
        writeln!(
            output,
            "            \"device_revision\": {},",
            compatibility.device_revision
        )?;
        writeln!(
            output,
            "            \"application_version\": {},",
            quote(application_version(compatibility.application_version))?
        )?;
        writeln!(
            output,
            "            \"application_base\": {},",
            quote(&compatibility.application_base)?
        )?;
        writeln!(
            output,
            "            \"application_end_exclusive\": {},",
            quote(&compatibility.application_end_exclusive)?
        )?;
        writeln!(
            output,
            "            \"bank_layout\": {},",
            quote(bank_layout(compatibility.bank_layout))?
        )?;
        writeln!(output, "        }},")?;
        writeln!(output, "        \"recovery\": {{")?;
        writeln!(
            output,
            "            \"mount_label\": {},",
            quote(&build.recovery.mount_label)?
        )?;
        writeln!(
            output,
            "            \"board_id_prefix\": {},",
            quote(&build.recovery.board_identity.value)?
        )?;
        writeln!(
            output,
            "            \"family_id\": {},",
            quote(&build.recovery.family_id)?
        )?;
        writeln!(output, "        }},")?;
        writeln!(
            output,
            "        \"firmware_owned\": ({:#010x}, {:#010x}),",
            firmware.start(),
            firmware.end_exclusive()
        )?;
        writeln!(
            output,
            "        \"transport_envelope\": ({:#010x}, {:#010x}),",
            transport.start(),
            transport.end_exclusive()
        )?;
        writeln!(output, "    }},")?;
    }
    writeln!(output, "}}")?;
    Ok(())
}

fn quote(value: &str) -> Result<String, serde_json::Error> {
    serde_json::to_string(value)
}

fn invalid_profile(board: &str, source: MemoryProfileReferenceError) -> PythonContractError {
    PythonContractError::MemoryProfile {
        board: board.to_string(),
        source,
    }
}

const fn application_version(value: NrfDfuApplicationVersion) -> &'static str {
    match value {
        NrfDfuApplicationVersion::NotEnforced => "not-enforced",
    }
}

const fn bank_layout(value: NrfDfuBankLayout) -> &'static str {
    match value {
        NrfDfuBankLayout::Single => "single",
        NrfDfuBankLayout::Dual => "dual",
    }
}
