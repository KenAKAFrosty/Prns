use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use espflash::flasher::{FlashData, FlashFrequency, FlashMode, FlashSettings, FlashSize};
use espflash::image_format::{idf::IdfBootloaderFormat, ImageFormat};
use espflash::target::{Chip, XtalFrequency};
use prns_flash_manifest::{
    sha256_hex, BoardBuild, BoardCatalog, BoardCatalogEntry, FlashManifest, FlashPart,
    FlashPartKind, ReleaseChannel, ReleaseInfo, ReleaseVersion, SigningInfo, SourceArchiveIdentity,
    TargetManifest, FLASH_MANIFEST_SCHEMA,
};

use crate::cli::ChannelArg;
use crate::error::AppError;
use crate::events::{Phase, Reporter};
use crate::release::PreparedTarget;
use crate::toolchain::{capture_stdout, configure_esp_toolchain, run_status, rust_host_triple};

const PARTITION_TABLE_OFFSET: u32 = 0x8000;
const APPLICATION_OFFSET: u32 = 0x10000;
const SOURCE_APPLICATION_HEADROOM: u64 = 1024 * 1024;
const SOURCE_EMBED_OVERHEAD_ALLOWANCE: u64 = 64 * 1024;

struct BuiltPart {
    descriptor: FlashPart,
    bytes: Vec<u8>,
}

fn embedded_cargo_command() -> Command {
    let mut command = Command::new("cargo");
    command
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTFLAGS");
    command
}

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
    let mut source_capabilities = Vec::with_capacity(catalog.boards.len());
    for board in &catalog.boards {
        let board_dir = board_output(out_root, &board.slug, &version);
        let record = board_dir.join("target.json");
        let bytes = fs::read(&record).map_err(|error| {
            AppError::developer_artifact(format!(
                "missing built target record {}: {error}",
                record.display()
            ))
        })?;
        let target = serde_json::from_slice::<TargetManifest>(&bytes).map_err(|error| {
            AppError::developer_artifact(format!(
                "invalid target record {}: {error}",
                record.display()
            ))
        })?;
        targets.push(target);
        let capability_path = board_dir.join("source-capability.json");
        let capability = fs::read(&capability_path)
            .map_err(|error| {
                AppError::developer_artifact(format!(
                    "missing source capability record {}: {error}",
                    capability_path.display()
                ))
            })
            .and_then(|bytes| {
                serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| {
                    AppError::developer_artifact(format!(
                        "invalid source capability record {}: {error}",
                        capability_path.display()
                    ))
                })
            })?;
        source_capabilities.push(capability);
    }
    let manifest = FlashManifest {
        schema: FLASH_MANIFEST_SCHEMA,
        release: ReleaseInfo {
            version: version.clone(),
            channel: match channel {
                ChannelArg::Stable => ReleaseChannel::Stable,
                ChannelArg::Preview => ReleaseChannel::Preview,
            },
            commit: commit.clone(),
        },
        signing: SigningInfo { key_id },
        targets,
    };
    manifest
        .validate(catalog)
        .map_err(|error| AppError::developer_manifest(error.to_string()))?;
    let path = out_root.join("flash-manifest.json");
    let json = serde_json::to_vec_pretty(&manifest).map_err(|error| {
        AppError::developer_manifest(format!("could not encode manifest: {error}"))
    })?;
    atomic_write(&path, &with_newline(json))?;
    let capability_document = serde_json::to_vec_pretty(&serde_json::json!({
        "schema": 1,
        "version": version,
        "commit": commit,
        "targets": source_capabilities,
    }))
    .map_err(|error| {
        AppError::developer_manifest(format!("could not encode source capabilities: {error}"))
    })?;
    let metadata_dir = out_root.join("metadata");
    fs::create_dir_all(&metadata_dir).map_err(|error| {
        AppError::developer_artifact(format!(
            "could not create candidate metadata directory: {error}"
        ))
    })?;
    atomic_write(
        &metadata_dir.join("source-capabilities.json"),
        &with_newline(capability_document),
    )?;
    let notices = repo.join("THIRD_PARTY_NOTICES.md");
    fs::copy(&notices, out_root.join("THIRD_PARTY_NOTICES.md")).map_err(|error| {
        AppError::developer_artifact(format!("could not copy release notices: {error}"))
    })?;
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
    let partition_table = crate_dir.join(&build.partition_table);
    let mut source = source_archive_identity(board, repo, version)?;
    let base_parts = build_esp_parts(board, build, &crate_dir, &partition_table, version, false)?;
    let mut capacity_downgrade = false;
    let parts = if let Some(identity) = source.as_ref() {
        let partition_bytes = factory_partition_bytes(&partition_table)?;
        let base_application_bytes = application_part_bytes(&base_parts)?;
        if !source_preflight_fits(base_application_bytes, identity.size, partition_bytes) {
            eprintln!(
                "SOURCE CAPABILITY DOWNGRADE: {} base application {} bytes plus source.zip {} bytes, {} bytes embedding allowance, and {} bytes reserve exceed its {}-byte application partition; keeping the compact build",
                board.slug,
                base_application_bytes,
                identity.size,
                SOURCE_EMBED_OVERHEAD_ALLOWANCE,
                SOURCE_APPLICATION_HEADROOM,
                partition_bytes
            );
            source = None;
            capacity_downgrade = true;
            base_parts
        } else {
            let source_parts =
                build_esp_parts(board, build, &crate_dir, &partition_table, version, true)?;
            let application_bytes = application_part_bytes(&source_parts)?;
            if application_bytes.saturating_add(SOURCE_APPLICATION_HEADROOM) > partition_bytes {
                eprintln!(
                    "SOURCE CAPABILITY DOWNGRADE: {} source-enabled application is {} bytes and cannot retain the required {}-byte reserve in its {}-byte application partition; keeping the compact build",
                    board.slug,
                    application_bytes,
                    SOURCE_APPLICATION_HEADROOM,
                    partition_bytes
                );
                source = None;
                capacity_downgrade = true;
                base_parts
            } else {
                source_parts
            }
        }
    } else {
        base_parts
    };
    let output_dir = board_output(out_root, &board.slug, version);
    fs::create_dir_all(&output_dir).map_err(|error| {
        AppError::developer_artifact(format!(
            "could not create {}: {error}",
            output_dir.display()
        ))
    })?;
    for part in &parts {
        let filename = Path::new(&part.descriptor.path)
            .file_name()
            .ok_or_else(|| AppError::developer_artifact("firmware part path has no filename"))?;
        atomic_write(&output_dir.join(filename), &part.bytes)?;
    }
    let target = target_record(
        board,
        parts.iter().map(|part| part.descriptor.clone()).collect(),
        source,
    );
    write_target_record(&output_dir, &target)?;
    write_source_capability_record(
        &output_dir,
        board,
        target.source.as_ref(),
        capacity_downgrade,
    )?;
    let source_bytes = target.source.as_ref().map_or(0, |source| source.size);
    let (version, target) = validated_prepared_target(board, version, target)?;
    report_sparse_size(board, &parts, source_bytes, reporter)?;
    let prepared = PreparedTarget::bind(
        version,
        target,
        parts.into_iter().map(|part| part.bytes).collect(),
    )
    .map_err(|error| AppError::developer_artifact(error.to_string()))?;
    let target_record = output_dir.join("target.json");
    Ok(BuildOutput {
        prepared,
        output_dir,
        target_record,
    })
}

fn build_esp_parts(
    board: &BoardCatalogEntry,
    build: &prns_flash_manifest::EspBuild,
    crate_dir: &Path,
    partition_table: &Path,
    version: &str,
    source_enabled: bool,
) -> Result<Vec<BuiltPart>, AppError> {
    let elf = crate_dir
        .join("target")
        .join(&build.rust_target)
        .join("release")
        .join(&build.binary);
    let mut cargo = embedded_cargo_command();
    cargo
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
        .current_dir(crate_dir);
    if source_enabled {
        cargo.arg("--features").arg("source-archive");
    }
    if build.rust_target.starts_with("xtensa-") {
        configure_esp_toolchain(&mut cargo)?;
    }
    run_status(&mut cargo, "embedded ESP cargo build")?;

    let elf_bytes = fs::read(&elf).map_err(|error| {
        AppError::developer_artifact(format!("could not read {}: {error}", elf.display()))
    })?;
    let chip = build.chip.parse::<Chip>().map_err(|error| {
        AppError::developer_build(format!("invalid chip {:?}: {error}", build.chip))
    })?;
    let flash_size = match board.flash_size {
        Some(4_194_304) => FlashSize::_4Mb,
        Some(8_388_608) => FlashSize::_8Mb,
        other => {
            return Err(AppError::developer_build(format!(
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
        Some(partition_table),
        None,
        Some(PARTITION_TABLE_OFFSET),
        Some("factory"),
    )
    .map_err(|error| {
        AppError::developer_build(format!("could not construct sparse ESP image: {error}"))
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
                return Err(AppError::developer_build(format!(
                    "unexpected sparse ESP segment at 0x{address:x}"
                )));
            }
        };
        let bytes = segment.data.into_owned();
        let descriptor = FlashPart {
            kind,
            path: release_part_path(&board.slug, version, filename),
            offset: Some(segment.addr),
            size: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        };
        parts.push(BuiltPart { descriptor, bytes });
    }
    parts.sort_by_key(|part| part.descriptor.offset);
    Ok(parts)
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
    let mut cargo = embedded_cargo_command();
    cargo
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
        AppError::developer_artifact(format!("could not create work directory: {error}"))
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
        AppError::developer_artifact(format!(
            "could not create {}: {error}",
            output_dir.display()
        ))
    })?;
    let uf2 = output_dir.join("t-echo.uf2");
    run_status(
        Command::new("python3")
            .arg(repo.join("tools").join("device").join("bin2uf2.py"))
            .arg(&binary)
            .arg(&uf2)
            .arg(&build.base_address)
            .arg(&build.family_id),
        "bin2uf2.py",
    )?;
    let bytes = fs::read(&uf2)
        .map_err(|error| AppError::developer_artifact(format!("could not read UF2: {error}")))?;
    let descriptor = FlashPart {
        kind: FlashPartKind::Uf2,
        path: release_part_path(&board.slug, version, "t-echo.uf2"),
        offset: None,
        size: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
    };
    let target = target_record(board, vec![descriptor.clone()], None);
    write_target_record(&output_dir, &target)?;
    write_source_capability_record(&output_dir, board, None, false)?;
    let (version, target) = validated_prepared_target(board, version, target)?;
    reporter.phase(
        Phase::ArtifactReady,
        Some(&board.slug),
        &format!("UF2 ready: {} bytes", bytes.len()),
    );
    let prepared = PreparedTarget::bind(version, target, vec![bytes])
        .map_err(|error| AppError::developer_artifact(error.to_string()))?;
    let target_record = output_dir.join("target.json");
    Ok(BuildOutput {
        prepared,
        output_dir,
        target_record,
    })
}

fn source_archive_identity(
    board: &BoardCatalogEntry,
    repo: &Path,
    version: &str,
) -> Result<Option<SourceArchiveIdentity>, AppError> {
    if !board.source_archive_capable {
        return Ok(None);
    }
    let Some(path) = std::env::var_os("PRNS_SOURCE_ARCHIVE") else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(AppError::developer_build(
            "PRNS_SOURCE_ARCHIVE must be an absolute path",
        ));
    }
    let bytes = fs::read(&path).map_err(|error| {
        AppError::developer_artifact(format!(
            "could not read PRNS_SOURCE_ARCHIVE {}: {error}",
            path.display()
        ))
    })?;
    let source_version = required_source_environment("PRNS_SOURCE_VERSION")?;
    if source_version != version {
        return Err(AppError::developer_build(format!(
            "PRNS_SOURCE_VERSION {source_version:?} disagrees with repository VERSION {version:?}"
        )));
    }
    let source_commit = required_source_environment("PRNS_SOURCE_COMMIT")?;
    if source_commit.len() != 40
        || !source_commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::developer_build(
            "PRNS_SOURCE_COMMIT must be a lowercase full Git commit",
        ));
    }
    let repository_commit = capture_stdout(
        Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(repo),
        "git rev-parse HEAD",
    )?;
    if repository_commit.trim() != source_commit {
        return Err(AppError::developer_build(format!(
            "PRNS_SOURCE_COMMIT {source_commit} disagrees with repository HEAD {}",
            repository_commit.trim()
        )));
    }
    let expected_size = required_source_environment("PRNS_SOURCE_SIZE")?
        .parse::<u64>()
        .map_err(|_| AppError::developer_build("PRNS_SOURCE_SIZE must be an integer"))?;
    let expected_sha256 = required_source_environment("PRNS_SOURCE_SHA256")?;
    if !is_sha256(&expected_sha256)
        || u64::try_from(bytes.len()).ok() != Some(expected_size)
        || sha256_hex(&bytes) != expected_sha256
    {
        return Err(AppError::developer_artifact(
            "PRNS_SOURCE_ARCHIVE bytes disagree with canonical source metadata",
        ));
    }
    Ok(Some(SourceArchiveIdentity {
        route: "/file/source.zip".to_string(),
        checksum_route: "/file/source.zip.sha256".to_string(),
        size: expected_size,
        sha256: expected_sha256,
    }))
}

fn required_source_environment(name: &str) -> Result<String, AppError> {
    std::env::var(name)
        .map_err(|_| AppError::developer_build(format!("{name} is required for source serving")))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn application_part_bytes(parts: &[BuiltPart]) -> Result<u64, AppError> {
    parts
        .iter()
        .find(|part| part.descriptor.kind == FlashPartKind::Application)
        .map(|part| part.descriptor.size)
        .ok_or_else(|| AppError::developer_artifact("ESP image has no application part"))
}

fn source_preflight_fits(
    base_application_bytes: u64,
    source_archive_bytes: u64,
    partition_bytes: u64,
) -> bool {
    base_application_bytes
        .checked_add(source_archive_bytes)
        .and_then(|bytes| bytes.checked_add(SOURCE_EMBED_OVERHEAD_ALLOWANCE))
        .and_then(|bytes| bytes.checked_add(SOURCE_APPLICATION_HEADROOM))
        .is_some_and(|required| required <= partition_bytes)
}

fn factory_partition_bytes(partition_table: &Path) -> Result<u64, AppError> {
    let csv = fs::read_to_string(partition_table).map_err(|error| {
        AppError::developer_artifact(format!(
            "could not read partition table {}: {error}",
            partition_table.display()
        ))
    })?;
    for line in csv.lines() {
        let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
        if fields.first().copied() == Some("factory") {
            let size = fields.get(4).copied().unwrap_or_default();
            return if let Some(hex) = size.strip_prefix("0x") {
                u64::from_str_radix(hex, 16)
            } else {
                size.parse::<u64>()
            }
            .map_err(|_| AppError::developer_build("factory partition size is invalid"));
        }
    }
    Err(AppError::developer_build(
        "partition table has no factory application partition",
    ))
}

fn write_source_capability_record(
    output_dir: &Path,
    board: &BoardCatalogEntry,
    source: Option<&SourceArchiveIdentity>,
    capacity_downgrade: bool,
) -> Result<(), AppError> {
    let status = if source.is_some() {
        "serving"
    } else if capacity_downgrade {
        "capacity-downgrade"
    } else {
        "absent"
    };
    let json = serde_json::to_vec_pretty(&serde_json::json!({
        "schema": 1,
        "board_slug": board.slug,
        "nominally_capable": board.source_archive_capable,
        "status": status,
        "source": source,
        "reserve_bytes": source.map(|_| SOURCE_APPLICATION_HEADROOM),
    }))
    .map_err(|error| {
        AppError::developer_manifest(format!("could not encode source capability: {error}"))
    })?;
    atomic_write(
        &output_dir.join("source-capability.json"),
        &with_newline(json),
    )
}

fn target_record(
    board: &BoardCatalogEntry,
    parts: Vec<FlashPart>,
    source: Option<SourceArchiveIdentity>,
) -> TargetManifest {
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
        source,
    }
}

fn validated_prepared_target(
    board: &BoardCatalogEntry,
    version: &str,
    target: TargetManifest,
) -> Result<(ReleaseVersion, prns_flash_manifest::ReleaseTarget), AppError> {
    let version = ReleaseVersion::parse(version.to_string()).map_err(|error| {
        AppError::developer_repository(format!("invalid repository VERSION: {error}"))
    })?;
    let target = target
        .into_validated(board, &version)
        .map_err(|error| AppError::developer_manifest(format!("invalid built target: {error}")))?;
    Ok((version, target))
}

fn write_target_record(output_dir: &Path, target: &TargetManifest) -> Result<(), AppError> {
    let json = serde_json::to_vec_pretty(target).map_err(|error| {
        AppError::developer_manifest(format!("could not encode target record: {error}"))
    })?;
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
        return Err(AppError::developer_artifact(
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
            AppError::developer_artifact(format!(
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
        return Err(AppError::developer_artifact(
            "embedded docs bundle is missing index.html",
        ));
    }
    Ok(())
}

fn report_sparse_size(
    board: &BoardCatalogEntry,
    parts: &[BuiltPart],
    source_bytes: u64,
    reporter: Reporter,
) -> Result<(), AppError> {
    let total = parts
        .iter()
        .map(|part| part.bytes.len() as u64)
        .sum::<u64>();
    let code_total = total.saturating_sub(source_bytes);
    if let Some((baseline, maximum)) = sparse_size_gate(&board.slug) {
        if code_total > maximum {
            return Err(AppError::developer_artifact(format!(
                "sparse code payload is {code_total} bytes after excluding {source_bytes} embedded source bytes, versus the {baseline}-byte merged baseline, and misses the 60% reduction gate (maximum {maximum})"
            )));
        }
    }
    reporter.phase(
        Phase::ArtifactReady,
        Some(&board.slug),
        &format!(
            "Sparse artifact ready: {total} bytes across {} parts ({source_bytes} embedded source bytes)",
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
        .map_err(|error| AppError::developer_repository(format!("could not read VERSION: {error}")))
        .and_then(|version| {
            if version.is_empty() || version.eq_ignore_ascii_case("next") {
                Err(AppError::developer_repository("VERSION is not publishable"))
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
    let parent = path.parent().ok_or_else(|| {
        AppError::developer_artifact(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        AppError::developer_artifact(format!("could not create {}: {error}", parent.display()))
    })?;
    let temporary = path.with_extension(format!("part-{}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| {
        AppError::developer_artifact(format!("could not write {}: {error}", temporary.display()))
    })?;
    fs::rename(&temporary, path).map_err(|error| {
        AppError::developer_artifact(format!("could not publish {}: {error}", path.display()))
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
    use std::collections::BTreeMap;
    use std::ffi::OsStr;

    #[test]
    fn embedded_cargo_removes_inherited_host_configuration() {
        let command = embedded_cargo_command();
        let environments = command.get_envs().collect::<BTreeMap<_, _>>();
        assert_eq!(
            environments,
            BTreeMap::from([
                (OsStr::new("RUSTFLAGS"), None),
                (OsStr::new("RUSTUP_TOOLCHAIN"), None),
            ])
        );
    }

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

    #[test]
    fn source_preflight_includes_archive_overhead_and_required_reserve() {
        let exact =
            2_100_000 + 4_960_000 + SOURCE_EMBED_OVERHEAD_ALLOWANCE + SOURCE_APPLICATION_HEADROOM;
        assert!(source_preflight_fits(2_100_000, 4_960_000, exact));
        assert!(!source_preflight_fits(2_100_000, 4_960_000, exact - 1));
        assert!(!source_preflight_fits(u64::MAX, 1, u64::MAX));
    }
}
