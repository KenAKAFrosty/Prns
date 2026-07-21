use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use espflash::flasher::{FlashData, FlashFrequency, FlashMode, FlashSettings, FlashSize};
use espflash::image_format::{idf::IdfBootloaderFormat, ImageFormat};
use espflash::target::{Chip, XtalFrequency};
use prns_flash_manifest::{
    sha256_hex, BoardBuild, BoardCatalog, BoardCatalogEntry, FlashManifest, FlashPart,
    FlashPartKind, ReleaseChannel, ReleaseInfo, SigningInfo, TargetManifest, FLASH_MANIFEST_SCHEMA,
};

use crate::cli::ChannelArg;
use crate::error::AppError;
use crate::events::{Phase, Reporter};
use crate::release::{PreparedPart, PreparedTarget};
use crate::toolchain::{capture_stdout, configure_esp_toolchain, run_status, rust_host_triple};

const PARTITION_TABLE_OFFSET: u32 = 0x8000;
const APPLICATION_OFFSET: u32 = 0x10000;

pub(crate) struct BuildOutput {
    pub(crate) prepared: PreparedTarget,
    pub(crate) output_dir: PathBuf,
    pub(crate) target_record: PathBuf,
}

pub(crate) fn build_board(
    board: &BoardCatalogEntry,
    repo: &Path,
    out_root: &Path,
    reporter: Reporter,
) -> Result<BuildOutput, AppError> {
    let version = release_version(repo)?;
    match &board.build {
        BoardBuild::Esp(build) => build_esp(board, build, repo, out_root, &version, reporter),
        BoardBuild::Uf2(build) => build_uf2(board, build, repo, out_root, &version, reporter),
    }
}

pub(crate) fn assemble_manifest(
    catalog: &BoardCatalog,
    repo: &Path,
    out_root: &Path,
    channel: ChannelArg,
    commit: String,
    key_id: String,
) -> Result<PathBuf, AppError> {
    let version = release_version(repo)?;
    let mut targets = Vec::with_capacity(catalog.boards.len());
    for board in &catalog.boards {
        let record = board_output(out_root, &board.slug, &version).join("target.json");
        let bytes = fs::read(&record).map_err(|error| {
            AppError::developer(format!(
                "missing built target record {}: {error}",
                record.display()
            ))
        })?;
        let target = serde_json::from_slice::<TargetManifest>(&bytes).map_err(|error| {
            AppError::developer(format!(
                "invalid target record {}: {error}",
                record.display()
            ))
        })?;
        targets.push(target);
    }
    let manifest = FlashManifest {
        schema: FLASH_MANIFEST_SCHEMA,
        release: ReleaseInfo {
            version,
            channel: match channel {
                ChannelArg::Stable => ReleaseChannel::Stable,
                ChannelArg::Preview => ReleaseChannel::Preview,
            },
            commit,
        },
        signing: SigningInfo { key_id },
        targets,
    };
    manifest
        .validate(catalog)
        .map_err(|error| AppError::developer(error.to_string()))?;
    let path = out_root.join("flash-manifest.json");
    let json = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| AppError::developer(format!("could not encode manifest: {error}")))?;
    atomic_write(&path, &with_newline(json))?;
    let notices = repo.join("THIRD_PARTY_NOTICES.md");
    fs::copy(&notices, out_root.join("THIRD_PARTY_NOTICES.md"))
        .map_err(|error| AppError::developer(format!("could not copy release notices: {error}")))?;
    Ok(path)
}

fn build_esp(
    board: &BoardCatalogEntry,
    build: &prns_flash_manifest::EspBuild,
    repo: &Path,
    out_root: &Path,
    version: &str,
    reporter: Reporter,
) -> Result<BuildOutput, AppError> {
    prepare_embedded_site_bundle(build, repo, reporter)?;
    reporter.phase(
        Phase::Building,
        Some(&board.slug),
        &format!("Building {} developer firmware…", board.display_name),
    );
    let crate_dir = repo.join("personal-hopspot").join("embedded").join("esp32");
    let elf = crate_dir
        .join("target")
        .join(&build.rust_target)
        .join("release")
        .join(&build.binary);
    let partition_table = crate_dir.join(&build.partition_table);
    let mut cargo = Command::new("cargo");
    cargo
        .env_remove("RUSTUP_TOOLCHAIN")
        .arg("build")
        .arg("--release")
        .arg("--locked")
        .arg("--package")
        .arg(&build.package)
        .arg("--bin")
        .arg(&build.binary)
        .arg("--target")
        .arg(&build.rust_target)
        .arg("-Zbuild-std=core,alloc")
        .current_dir(&crate_dir);
    if build.rust_target.starts_with("xtensa-") {
        configure_esp_toolchain(&mut cargo)?;
    }
    run_status(&mut cargo, "embedded ESP cargo build")?;

    let elf_bytes = fs::read(&elf).map_err(|error| {
        AppError::developer(format!("could not read {}: {error}", elf.display()))
    })?;
    let chip = build
        .chip
        .parse::<Chip>()
        .map_err(|error| AppError::developer(format!("invalid chip {:?}: {error}", build.chip)))?;
    let flash_size = match board.flash_size {
        Some(4_194_304) => FlashSize::_4Mb,
        Some(8_388_608) => FlashSize::_8Mb,
        other => {
            return Err(AppError::developer(format!(
                "unsupported catalog flash size {other:?}"
            )));
        }
    };
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
    .map_err(|error| {
        AppError::developer(format!("could not construct sparse ESP image: {error}"))
    })?;
    let output_dir = board_output(out_root, &board.slug, version);
    fs::create_dir_all(&output_dir).map_err(|error| {
        AppError::developer(format!(
            "could not create {}: {error}",
            output_dir.display()
        ))
    })?;
    let mut parts = Vec::new();
    for segment in ImageFormat::from(image).flash_segments() {
        let (kind, filename) = match segment.addr {
            PARTITION_TABLE_OFFSET => (FlashPartKind::PartitionTable, "partition-table.bin"),
            APPLICATION_OFFSET => (FlashPartKind::Application, "application.bin"),
            _ if segment.addr < PARTITION_TABLE_OFFSET => {
                (FlashPartKind::Bootloader, "bootloader.bin")
            }
            address => {
                return Err(AppError::developer(format!(
                    "unexpected sparse ESP segment at 0x{address:x}"
                )));
            }
        };
        let bytes = segment.data.into_owned();
        let path = output_dir.join(filename);
        atomic_write(&path, &bytes)?;
        let descriptor = FlashPart {
            kind,
            path: release_part_path(&board.slug, version, filename),
            offset: Some(segment.addr),
            size: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        };
        parts.push(PreparedPart { descriptor, bytes });
    }
    parts.sort_by_key(|part| part.descriptor.offset);
    let target = target_record(
        board,
        parts.iter().map(|part| part.descriptor.clone()).collect(),
    );
    write_target_record(&output_dir, &target)?;
    report_sparse_size(board, &parts, reporter)?;
    let target_record = output_dir.join("target.json");
    Ok(BuildOutput {
        prepared: PreparedTarget {
            version: version.to_string(),
            target,
            parts,
        },
        output_dir,
        target_record,
    })
}

fn build_uf2(
    board: &BoardCatalogEntry,
    build: &prns_flash_manifest::Uf2Build,
    repo: &Path,
    out_root: &Path,
    version: &str,
    reporter: Reporter,
) -> Result<BuildOutput, AppError> {
    reporter.phase(
        Phase::Building,
        Some(&board.slug),
        &format!("Building {} developer firmware…", board.display_name),
    );
    let crate_dir = repo
        .join("personal-hopspot")
        .join("embedded")
        .join("nrf52840");
    let mut cargo = Command::new("cargo");
    cargo
        .env_remove("RUSTUP_TOOLCHAIN")
        .arg("build")
        .arg("--release")
        .arg("--locked")
        .current_dir(&crate_dir);
    run_status(&mut cargo, "T-Echo cargo build")?;

    let host_triple = rust_host_triple()?;
    let sysroot = capture_stdout(Command::new("rustc").arg("--print").arg("sysroot"), "rustc")?;
    let objcopy = Path::new(sysroot.trim())
        .join("lib")
        .join("rustlib")
        .join(host_triple.trim())
        .join("bin")
        .join("llvm-objcopy");
    let elf = crate_dir
        .join("target")
        .join(&build.rust_target)
        .join("release")
        .join(&build.package);
    let work_dir = repo
        .join("target")
        .join("flash-artifacts")
        .join("work")
        .join(&board.slug);
    fs::create_dir_all(&work_dir).map_err(|error| {
        AppError::developer(format!("could not create work directory: {error}"))
    })?;
    let binary = work_dir.join("firmware.bin");
    run_status(
        Command::new(&objcopy)
            .arg("-O")
            .arg("binary")
            .arg(&elf)
            .arg(&binary),
        "llvm-objcopy",
    )?;
    let output_dir = board_output(out_root, &board.slug, version);
    fs::create_dir_all(&output_dir).map_err(|error| {
        AppError::developer(format!(
            "could not create {}: {error}",
            output_dir.display()
        ))
    })?;
    let uf2 = output_dir.join("t-echo.uf2");
    run_status(
        Command::new("python3")
            .arg(repo.join("scripts").join("bin2uf2.py"))
            .arg(&binary)
            .arg(&uf2)
            .arg(&build.base_address)
            .arg(&build.family_id),
        "bin2uf2.py",
    )?;
    let bytes = fs::read(&uf2)
        .map_err(|error| AppError::developer(format!("could not read UF2: {error}")))?;
    let descriptor = FlashPart {
        kind: FlashPartKind::Uf2,
        path: release_part_path(&board.slug, version, "t-echo.uf2"),
        offset: None,
        size: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
    };
    let target = target_record(board, vec![descriptor.clone()]);
    write_target_record(&output_dir, &target)?;
    reporter.phase(
        Phase::ArtifactReady,
        Some(&board.slug),
        &format!("UF2 ready: {} bytes", bytes.len()),
    );
    let target_record = output_dir.join("target.json");
    Ok(BuildOutput {
        prepared: PreparedTarget {
            version: version.to_string(),
            target,
            parts: vec![PreparedPart { descriptor, bytes }],
        },
        output_dir,
        target_record,
    })
}

fn target_record(board: &BoardCatalogEntry, parts: Vec<FlashPart>) -> TargetManifest {
    let esp = match &board.build {
        BoardBuild::Esp(build) => Some(build),
        BoardBuild::Uf2(_) => None,
    };
    TargetManifest {
        board_slug: board.slug.clone(),
        display_name: board.display_name.clone(),
        silicon: board.silicon.clone(),
        interfaces: board.interfaces.clone(),
        transport: board.transport,
        expected_chip: board.expected_chip.clone(),
        flash_size: board.flash_size,
        flash_mode: esp.map(|build| build.flash_mode.clone()),
        flash_frequency: esp.map(|build| build.flash_frequency.clone()),
        before_reset: esp.map(|build| build.before_reset.clone()),
        after_reset: esp.map(|build| build.after_reset.clone()),
        preparation_profile: board.preparation_profile.clone(),
        parts,
        provisioning: board.provisioning.clone(),
    }
}

fn write_target_record(output_dir: &Path, target: &TargetManifest) -> Result<(), AppError> {
    let json = serde_json::to_vec_pretty(target)
        .map_err(|error| AppError::developer(format!("could not encode target record: {error}")))?;
    atomic_write(&output_dir.join("target.json"), &with_newline(json))
}

fn prepare_embedded_site_bundle(
    build: &prns_flash_manifest::EspBuild,
    repo: &Path,
    reporter: Reporter,
) -> Result<(), AppError> {
    if !build.rust_target.starts_with("xtensa-") {
        return Ok(());
    }
    let site_dir = repo.join("docs").join("website");
    let output_dir = site_dir
        .join("target")
        .join("dx")
        .join("reticulum-site")
        .join("release")
        .join("web")
        .join("public");
    if std::env::var_os("PRNS_EMBEDDED_SITE_READY").is_some() {
        if output_dir.join("index.html").is_file() {
            return Ok(());
        }
        return Err(AppError::developer(
            "PRNS_EMBEDDED_SITE_READY was set but the embedded site output is missing",
        ));
    }
    reporter.phase(
        Phase::BuildingEmbeddedSite,
        None,
        "Building the hosted-JavaScript-free SoftAP site bundle…",
    );
    if output_dir.exists() {
        fs::remove_dir_all(&output_dir).map_err(|error| {
            AppError::developer(format!(
                "could not clear generated Dioxus output {}: {error}",
                output_dir.display()
            ))
        })?;
    }
    let mut dx = Command::new("dx");
    dx.env("PRNS_EMBEDDED_SITE", "1")
        .env_remove("PRNS_FLASH_ARTIFACT_ROOT")
        .arg("build")
        .arg("--platform")
        .arg("web")
        .arg("--debug-symbols")
        .arg("false")
        .arg("--release")
        .arg("--features")
        .arg("embedded-site")
        .current_dir(&site_dir);
    run_status(&mut dx, "embedded site build")?;
    if !output_dir.join("index.html").is_file() {
        return Err(AppError::developer(
            "embedded docs bundle is missing index.html",
        ));
    }
    Ok(())
}

fn report_sparse_size(
    board: &BoardCatalogEntry,
    parts: &[PreparedPart],
    reporter: Reporter,
) -> Result<(), AppError> {
    let total = parts
        .iter()
        .map(|part| part.bytes.len() as u64)
        .sum::<u64>();
    if let Some((baseline, maximum)) = sparse_size_gate(&board.slug) {
        if total > maximum {
            return Err(AppError::developer(format!(
                "sparse artifact is {total} bytes versus the {baseline}-byte merged baseline and misses the 60% reduction gate (maximum {maximum})"
            )));
        }
    }
    reporter.phase(
        Phase::ArtifactReady,
        Some(&board.slug),
        &format!(
            "Sparse artifact ready: {total} bytes across {} parts",
            parts.len()
        ),
    );
    Ok(())
}

fn sparse_size_gate(board_slug: &str) -> Option<(u64, u64)> {
    match board_slug {
        "heltec-v4" => Some((7_643_152, 3_057_260)),
        "t-beam-supreme" => Some((7_639_296, 3_055_718)),
        _ => None,
    }
}

fn release_version(repo: &Path) -> Result<String, AppError> {
    fs::read_to_string(repo.join("VERSION"))
        .map(|value| value.trim().to_string())
        .map_err(|error| AppError::developer(format!("could not read VERSION: {error}")))
        .and_then(|version| {
            if version.is_empty() || version.eq_ignore_ascii_case("next") {
                Err(AppError::developer("VERSION is not publishable"))
            } else {
                Ok(version)
            }
        })
}

fn release_part_path(board: &str, version: &str, filename: &str) -> String {
    format!("firmware/hopspot/{board}/{version}/{filename}")
}

fn board_output(out_root: &Path, board: &str, version: &str) -> PathBuf {
    out_root
        .join("firmware")
        .join("hopspot")
        .join(board)
        .join(version)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::developer(format!("path has no parent: {}", path.display())))?;
    fs::create_dir_all(parent).map_err(|error| {
        AppError::developer(format!("could not create {}: {error}", parent.display()))
    })?;
    let temporary = path.with_extension(format!("part-{}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| {
        AppError::developer(format!("could not write {}: {error}", temporary.display()))
    })?;
    fs::rename(&temporary, path).map_err(|error| {
        AppError::developer(format!("could not publish {}: {error}", path.display()))
    })
}

fn with_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.push(b'\n');
    bytes
}

pub(crate) fn default_artifact_root(repo: &Path) -> PathBuf {
    repo.join("target").join("flash-artifacts")
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_flash_manifest::Transport;

    #[test]
    fn release_paths_are_versioned() {
        assert_eq!(
            release_part_path("heltec-v4", "0.2.6", "application.bin"),
            "firmware/hopspot/heltec-v4/0.2.6/application.bin"
        );
    }

    #[test]
    fn all_catalog_boards_have_a_build_recipe() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = prns_flash_manifest::board_catalog()?;
        assert_eq!(catalog.boards.len(), 4);
        assert!(catalog.boards.iter().all(|board| {
            matches!(
                (&board.transport, &board.build),
                (Transport::EspSerial, BoardBuild::Esp(_))
                    | (Transport::Uf2MassStorage, BoardBuild::Uf2(_))
            )
        }));
        Ok(())
    }

    #[test]
    fn s3_size_gates_are_board_specific_and_at_least_sixty_percent() {
        assert_eq!(sparse_size_gate("heltec-v4"), Some((7_643_152, 3_057_260)));
        assert_eq!(
            sparse_size_gate("t-beam-supreme"),
            Some((7_639_296, 3_055_718))
        );
        assert_eq!(sparse_size_gate("xiao-esp32-c6"), None);
    }
}
