use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use personal_hopspot_builder::platform::esp as esp_builder;
use personal_hopspot_builder::platform::nrf52840::uf2 as uf2_builder;
use personal_hopspot_builder::{
    embedded_cargo_command, llvm_objcopy, run_status, BuildContext, BuildVersion,
};
use prns_flash_manifest::{
    sha256_hex, validate_nrf_serial_dfu_recovery_artifact, validate_uf2_artifact, BoardBuild,
    BoardCatalog, BoardCatalogEntry, FlashManifest, FlashPart, FlashPartKind,
    ManifestTargetSetPolicy, NrfDfuApplicationVersion, NrfSerialDfuManifest,
    NrfSerialDfuRecoveryManifest, OfflineKeySigningInfo, ReleaseChannel, ReleaseInfo,
    ReleaseTarget, ReleaseVersion, SoftdeviceIdentity, TargetManifest, Uf2VariantManifest,
    FLASH_MANIFEST_SCHEMA,
};
use prns_nrf_dfu::{
    ApplicationInitPacket, ApplicationInitPacketSpec, ApplicationVersion, DfuDeviceRevision,
    DfuDeviceType, DfuImage, SoftdeviceFirmwareId, SoftdeviceRequirements,
};

use crate::cli::ChannelArg;
use crate::error::AppError;
use crate::events::{Phase, Reporter};
use crate::release::PreparedTarget;

pub(crate) struct BuildOutput {
    prepared: Option<PreparedTarget>,
    pub(crate) output_dir: PathBuf,
    pub(crate) target_record: PathBuf,
}

impl BuildOutput {
    pub(crate) fn into_prepared(self) -> Result<PreparedTarget, AppError> {
        self.prepared.ok_or_else(|| {
            AppError::developer_artifact("build did not select one flash compatibility variant")
        })
    }
}

pub(crate) enum ManifestTargetProfile<'a> {
    Production,
    LocalDevelopment {
        version: &'a str,
        board_slugs: &'a [String],
    },
}

enum Uf2BuildSelection<'a> {
    AllVariants,
    Compatible(&'a SoftdeviceIdentity),
}

pub(crate) fn build_board(
    board: &BoardCatalogEntry,
    repo: &Path,
    out_root: &Path,
    build_version: BuildVersion<'_>,
    reporter: Reporter,
) -> Result<BuildOutput, AppError> {
    let context = BuildContext::new(repo, out_root, build_version)?;
    match &board.build {
        BoardBuild::Esp(build) => build_esp(board, build, &context, reporter),
        BoardBuild::Uf2(build) => build_uf2(
            board,
            build,
            &context,
            Uf2BuildSelection::AllVariants,
            reporter,
        ),
        BoardBuild::NrfSerialDfu(build) => build_nrf_serial_dfu(board, build, &context, reporter),
    }
}

pub(crate) fn build_board_for_flash(
    board: &BoardCatalogEntry,
    repo: &Path,
    out_root: &Path,
    build_version: BuildVersion<'_>,
    softdevice: &SoftdeviceIdentity,
    reporter: Reporter,
) -> Result<BuildOutput, AppError> {
    let context = BuildContext::new(repo, out_root, build_version)?;
    match &board.build {
        BoardBuild::Esp(build) => build_esp(board, build, &context, reporter),
        BoardBuild::Uf2(build) => build_uf2(
            board,
            build,
            &context,
            Uf2BuildSelection::Compatible(softdevice),
            reporter,
        ),
        BoardBuild::NrfSerialDfu(_) => Err(AppError::developer_build(
            "Nordic serial DFU build does not accept a UF2 SoftDevice selection",
        )),
    }
}

pub(crate) fn assemble_manifest(
    catalog: &BoardCatalog,
    repo: &Path,
    out_root: &Path,
    channel: ChannelArg,
    commit: String,
    key_id: String,
    target_profile: ManifestTargetProfile<'_>,
) -> Result<PathBuf, AppError> {
    let (build_version, boards, policy) = match target_profile {
        ManifestTargetProfile::Production => (
            BuildVersion::Repository,
            catalog.shipping_boards().collect::<Vec<_>>(),
            ManifestTargetSetPolicy::all_shipping_targets(catalog),
        ),
        ManifestTargetProfile::LocalDevelopment {
            version,
            board_slugs,
        } => {
            let slugs = board_slugs.iter().map(String::as_str).collect::<Vec<_>>();
            let policy = ManifestTargetSetPolicy::local_development(catalog, &slugs)
                .map_err(|error| AppError::developer_manifest(error.to_string()))?;
            let boards = slugs
                .iter()
                .map(|slug| {
                    catalog.board(slug).ok_or_else(|| {
                        AppError::developer_manifest(format!("unknown board {slug:?}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            (BuildVersion::Developer(version), boards, policy)
        }
    };
    let context = BuildContext::new(repo, out_root, build_version)?;
    let version = context.version().to_string();
    let mut targets = Vec::with_capacity(boards.len());
    let mut source_capabilities = Vec::with_capacity(boards.len());
    for board in boards {
        let board_dir = context.board_output(&board.slug);
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
        schema_version: FLASH_MANIFEST_SCHEMA,
        release: ReleaseInfo {
            version: version.clone(),
            channel: match channel {
                ChannelArg::Stable => ReleaseChannel::Stable,
                ChannelArg::Preview => ReleaseChannel::Preview,
            },
            commit: commit.clone(),
        },
        signing: OfflineKeySigningInfo { key_id },
        targets,
    };
    manifest
        .validate_with_target_set(catalog, &policy)
        .map_err(|error| AppError::developer_manifest(error.to_string()))?;
    let path = context.output_root().join("flash-manifest.json");
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
    let metadata_dir = context.output_root().join("metadata");
    fs::create_dir_all(&metadata_dir).map_err(|error| {
        AppError::developer_artifact(format!(
            "could not create candidate metadata directory: {error}"
        ))
    })?;
    atomic_write(
        &metadata_dir.join("source-capabilities.json"),
        &with_newline(capability_document),
    )?;
    let notices = context.repository().join("THIRD_PARTY_NOTICES.md");
    fs::copy(
        &notices,
        context.output_root().join("THIRD_PARTY_NOTICES.md"),
    )
    .map_err(|error| {
        AppError::developer_artifact(format!("could not copy release notices: {error}"))
    })?;
    Ok(path)
}

fn build_esp(
    board: &BoardCatalogEntry,
    build: &prns_flash_manifest::EspBuild,
    context: &BuildContext<'_>,
    reporter: Reporter,
) -> Result<BuildOutput, AppError> {
    reporter.phase(
        Phase::Building,
        Some(&board.slug),
        &format!("Building {} developer firmware…", board.display_name),
    );
    let built = esp_builder::build(context, board, build)?;
    let output_dir = context.board_output(&board.slug);
    fs::create_dir_all(&output_dir).map_err(|error| {
        AppError::developer_artifact(format!(
            "could not create {}: {error}",
            output_dir.display()
        ))
    })?;
    for part in built.parts() {
        let filename = Path::new(&part.descriptor().path)
            .file_name()
            .ok_or_else(|| AppError::developer_artifact("firmware part path has no filename"))?;
        atomic_write(&output_dir.join(filename), part.bytes())?;
    }
    let target = target_record(
        board,
        BuiltTargetArtifacts::Esp(
            built
                .parts()
                .iter()
                .map(|part| part.descriptor().clone())
                .collect(),
        ),
    );
    write_target_record(&output_dir, &target)?;
    write_source_capability_record(&output_dir, board)?;
    let (version, target) = validated_prepared_target(board, context.version(), target)?;
    report_sparse_size(board, built.parts(), reporter)?;
    let prepared = PreparedTarget::bind(
        version,
        target,
        built
            .into_parts()
            .into_iter()
            .map(esp_builder::Part::into_bytes)
            .collect(),
    )
    .map_err(|error| AppError::developer_artifact(error.to_string()))?;
    let target_record = output_dir.join("target.json");
    Ok(BuildOutput {
        prepared: Some(prepared),
        output_dir,
        target_record,
    })
}

fn build_uf2(
    board: &BoardCatalogEntry,
    build: &prns_flash_manifest::Uf2Build,
    context: &BuildContext<'_>,
    selection: Uf2BuildSelection<'_>,
    reporter: Reporter,
) -> Result<BuildOutput, AppError> {
    reporter.phase(
        Phase::Building,
        Some(&board.slug),
        &format!("Building {} developer firmware…", board.display_name),
    );
    let selected_softdevice = match selection {
        Uf2BuildSelection::AllVariants => None,
        Uf2BuildSelection::Compatible(softdevice) => Some(softdevice),
    };
    let variants = compatible_uf2_build_variants(build, selected_softdevice);
    if variants.is_empty() {
        return Err(AppError::developer_artifact(format!(
            "no build variant matches {selected_softdevice:?}"
        )));
    }
    let built_variants = variants
        .into_iter()
        .map(|variant| uf2_builder::build(context, board, build, variant))
        .collect::<Result<Vec<_>, _>>()?;
    let descriptors = built_variants
        .iter()
        .map(|variant| variant.descriptor().clone())
        .collect();
    let target = target_record(board, BuiltTargetArtifacts::Uf2(descriptors));
    let output_dir = context.board_output(&board.slug);
    write_target_record(&output_dir, &target)?;
    write_source_capability_record(&output_dir, board)?;
    let (version, target) = match selected_softdevice {
        Some(softdevice) => {
            validated_prepared_uf2_variant(board, context.version(), target, softdevice)?
        }
        None => validated_prepared_target(board, context.version(), target)?,
    };
    let ReleaseTarget::Uf2(validated_uf2) = &target else {
        return Err(AppError::developer_manifest(format!(
            "built {} target did not validate as UF2",
            board.display_name
        )));
    };
    if validated_uf2.variants().len() != built_variants.len() {
        return Err(AppError::developer_artifact(
            "built UF2 descriptor and payload counts disagree",
        ));
    }
    for (variant, built) in validated_uf2.variants().iter().zip(&built_variants) {
        validate_uf2_artifact(variant, built.bytes()).map_err(|error| {
            AppError::developer_artifact(format!(
                "built UF2 {} is invalid: {error}",
                variant.part().path()
            ))
        })?;
    }
    reporter.phase(
        Phase::ArtifactReady,
        Some(&board.slug),
        &format!(
            "UF2 variants ready: {} bytes",
            built_variants
                .iter()
                .map(|variant| variant.bytes().len())
                .sum::<usize>()
        ),
    );
    let prepared = selected_softdevice
        .map(|softdevice| {
            let bytes = built_variants
                .into_iter()
                .next()
                .map(uf2_builder::Output::into_bytes)
                .ok_or_else(|| {
                    AppError::developer_artifact(format!(
                        "no built UF2 variant matches {softdevice}"
                    ))
                })?;
            PreparedTarget::bind_uf2(version.clone(), target.clone(), softdevice, bytes)
                .map_err(|error| AppError::developer_artifact(error.to_string()))
        })
        .transpose()?;
    let target_record = output_dir.join("target.json");
    Ok(BuildOutput {
        prepared,
        output_dir,
        target_record,
    })
}

fn build_nrf_serial_dfu(
    board: &BoardCatalogEntry,
    build: &prns_flash_manifest::NrfSerialDfuBuild,
    context: &BuildContext<'_>,
    reporter: Reporter,
) -> Result<BuildOutput, AppError> {
    reporter.phase(
        Phase::Building,
        Some(&board.slug),
        &format!("Building {} developer firmware…", board.display_name),
    );
    let crate_dir = context
        .repository()
        .join("personal-hopspot")
        .join("embedded")
        .join("nrf52840");
    let target_directory = crate_dir.join(&build.target_directory);
    let mut cargo = embedded_cargo_command();
    cargo
        .arg("build")
        .arg("--release")
        .arg("--locked")
        .arg("--no-default-features")
        .arg("--features")
        .arg(&build.cargo_feature)
        .arg("--package")
        .arg(&build.package)
        .arg("--bin")
        .arg(&build.binary)
        .arg("--target")
        .arg(&build.rust_target)
        .arg("--target-dir")
        .arg(&target_directory)
        .env("PRNS_BUILD_VERSION", context.version())
        .current_dir(&crate_dir);
    run_status(&mut cargo, "Nordic serial DFU cargo build")?;

    let output_dir = context.board_output(&board.slug);
    fs::create_dir_all(&output_dir).map_err(|error| {
        AppError::developer_artifact(format!(
            "could not create {}: {error}",
            output_dir.display()
        ))
    })?;
    let work_dir = context.work_output(&board.slug);
    fs::create_dir_all(&work_dir).map_err(|error| {
        AppError::developer_artifact(format!("could not create work directory: {error}"))
    })?;
    let elf = target_directory
        .join(&build.rust_target)
        .join("release")
        .join(&build.binary);
    let application_path = work_dir.join(&build.application_filename);
    run_status(
        Command::new(llvm_objcopy()?.as_os_str())
            .arg("-O")
            .arg("binary")
            .arg(&elf)
            .arg(&application_path),
        "llvm-objcopy",
    )?;
    let application = fs::read(&application_path).map_err(|error| {
        AppError::developer_artifact(format!(
            "could not read {}: {error}",
            application_path.display()
        ))
    })?;
    let memory = build
        .memory_layout()
        .map_err(|error| AppError::developer_manifest(error.to_string()))?;
    let firmware_owned = memory.firmware_owned();
    let maximum_application_bytes = firmware_owned.byte_len();
    if application.len() as u64 > u64::from(maximum_application_bytes) {
        return Err(AppError::developer_artifact(format!(
            "{} application is {} bytes; firmware owns at most {maximum_application_bytes}",
            board.display_name,
            application.len()
        )));
    }
    let init_packet_spec = nrf_init_packet_spec(&build.compatibility)?;
    let init_packet = ApplicationInitPacket::build(&application, &init_packet_spec)
        .map_err(|error| AppError::developer_artifact(error.to_string()))?
        .bytes()
        .to_vec();
    atomic_write(&output_dir.join(&build.application_filename), &application)?;
    atomic_write(&output_dir.join(&build.init_packet_filename), &init_packet)?;

    let recovery_path = output_dir.join(&build.recovery.filename);
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
            .arg(&build.recovery.family_id),
        "bin2uf2.py",
    )?;
    let recovery_uf2 = fs::read(&recovery_path).map_err(|error| {
        AppError::developer_artifact(format!("could not read recovery UF2: {error}"))
    })?;
    let compatibility = build
        .manifest_compatibility()
        .map_err(|error| AppError::developer_manifest(error.to_string()))?;
    let dfu_manifest = NrfSerialDfuManifest {
        serial: build.serial.clone(),
        compatibility,
        application: release_artifact(
            board,
            context,
            &build.application_filename,
            FlashPartKind::DfuApplication,
            &application,
        ),
        init_packet: release_artifact(
            board,
            context,
            &build.init_packet_filename,
            FlashPartKind::DfuInitPacket,
            &init_packet,
        ),
        recovery: NrfSerialDfuRecoveryManifest {
            mount_label: build.recovery.mount_label.clone(),
            board_id_prefix: build.recovery.board_identity.value.clone(),
            family_id: build.recovery.family_id.clone(),
            artifact: release_artifact(
                board,
                context,
                &build.recovery.filename,
                FlashPartKind::Uf2,
                &recovery_uf2,
            ),
        },
    };
    let target = target_record(
        board,
        BuiltTargetArtifacts::NrfSerialDfu(Box::new(dfu_manifest)),
    );
    write_target_record(&output_dir, &target)?;
    write_source_capability_record(&output_dir, board)?;
    let (version, target) = validated_prepared_target(board, context.version(), target)?;
    let ReleaseTarget::NrfSerialDfu(validated_target) = &target else {
        return Err(AppError::developer_manifest(
            "built target did not validate as Nordic serial DFU",
        ));
    };
    DfuImage::from_artifacts(&application, &init_packet, &init_packet_spec)
        .map_err(|error| AppError::developer_artifact(error.to_string()))?;
    validate_nrf_serial_dfu_recovery_artifact(validated_target, &application, &recovery_uf2)
        .map_err(|error| AppError::developer_artifact(error.to_string()))?;
    let prepared = PreparedTarget::bind(version, target, vec![application, init_packet])
        .map_err(|error| AppError::developer_artifact(error.to_string()))?;
    reporter.phase(
        Phase::ArtifactReady,
        Some(&board.slug),
        "Nordic serial DFU and recovery artifacts are ready.",
    );
    let target_record = output_dir.join("target.json");
    Ok(BuildOutput {
        prepared: Some(prepared),
        output_dir,
        target_record,
    })
}

fn nrf_init_packet_spec(
    compatibility: &prns_flash_manifest::NrfSerialDfuBuildCompatibility,
) -> Result<ApplicationInitPacketSpec, AppError> {
    let fwid = SoftdeviceFirmwareId::new(parse_catalog_hex_u16("FWID", &compatibility.fwid)?)
        .map_err(|error| AppError::developer_manifest(error.to_string()))?;
    Ok(ApplicationInitPacketSpec {
        device_type: DfuDeviceType::new(parse_catalog_hex_u16(
            "device type",
            &compatibility.device_type,
        )?),
        device_revision: DfuDeviceRevision::new(compatibility.device_revision),
        application_version: match compatibility.application_version {
            NrfDfuApplicationVersion::NotEnforced => ApplicationVersion::NotEnforced,
        },
        softdevices: SoftdeviceRequirements::new(fwid, std::iter::empty())
            .map_err(|error| AppError::developer_manifest(error.to_string()))?,
    })
}

fn parse_catalog_hex_u16(label: &str, value: &str) -> Result<u16, AppError> {
    let digits = value.strip_prefix("0x").ok_or_else(|| {
        AppError::developer_manifest(format!("invalid Nordic DFU {label} {value:?}"))
    })?;
    u16::from_str_radix(digits, 16).map_err(|error| {
        AppError::developer_manifest(format!("invalid Nordic DFU {label} {value:?}: {error}"))
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

fn compatible_uf2_build_variants<'a>(
    build: &'a prns_flash_manifest::Uf2Build,
    softdevice: Option<&SoftdeviceIdentity>,
) -> Vec<&'a prns_flash_manifest::Uf2BuildVariant> {
    match softdevice {
        Some(softdevice) => build
            .variants
            .iter()
            .filter(|variant| {
                variant.softdevice_family == softdevice.family().as_str()
                    && variant.softdevice_version == softdevice.version().as_str()
            })
            .collect(),
        None => build.variants.iter().collect(),
    }
}

fn write_source_capability_record(
    output_dir: &Path,
    board: &BoardCatalogEntry,
) -> Result<(), AppError> {
    let json = serde_json::to_vec_pretty(&serde_json::json!({
        "schema": 1,
        "board_slug": board.slug,
        "nominally_capable": false,
        "status": "absent",
        "source": null,
        "reserve_bytes": null,
    }))
    .map_err(|error| {
        AppError::developer_manifest(format!("could not encode source capability: {error}"))
    })?;
    atomic_write(
        &output_dir.join("source-capability.json"),
        &with_newline(json),
    )
}

enum BuiltTargetArtifacts {
    Esp(Vec<FlashPart>),
    Uf2(Vec<Uf2VariantManifest>),
    NrfSerialDfu(Box<NrfSerialDfuManifest>),
}

fn target_record(board: &BoardCatalogEntry, artifacts: BuiltTargetArtifacts) -> TargetManifest {
    let esp = match &board.build {
        BoardBuild::Esp(build) => Some(build),
        BoardBuild::Uf2(_) => None,
        BoardBuild::NrfSerialDfu(_) => None,
    };
    let (parts, variants, nrf_serial_dfu) = match artifacts {
        BuiltTargetArtifacts::Esp(parts) => (parts, Vec::new(), None),
        BuiltTargetArtifacts::Uf2(variants) => (Vec::new(), variants, None),
        BuiltTargetArtifacts::NrfSerialDfu(manifest) => (Vec::new(), Vec::new(), Some(*manifest)),
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
        variants,
        nrf_serial_dfu,
        provisioning: board.provisioning.clone(),
        source: None,
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

fn validated_prepared_uf2_variant(
    board: &BoardCatalogEntry,
    version: &str,
    target: TargetManifest,
    softdevice: &SoftdeviceIdentity,
) -> Result<(ReleaseVersion, prns_flash_manifest::ReleaseTarget), AppError> {
    let version = ReleaseVersion::parse(version.to_string()).map_err(|error| {
        AppError::developer_repository(format!("invalid repository VERSION: {error}"))
    })?;
    let target = target
        .into_validated_uf2_variant(board, &version, softdevice)
        .map_err(|error| AppError::developer_manifest(format!("invalid built target: {error}")))?;
    Ok((version, target))
}

fn write_target_record(output_dir: &Path, target: &TargetManifest) -> Result<(), AppError> {
    let json = serde_json::to_vec_pretty(target).map_err(|error| {
        AppError::developer_manifest(format!("could not encode target record: {error}"))
    })?;
    atomic_write(&output_dir.join("target.json"), &with_newline(json))
}

fn report_sparse_size(
    board: &BoardCatalogEntry,
    parts: &[esp_builder::Part],
    reporter: Reporter,
) -> Result<(), AppError> {
    let total = parts
        .iter()
        .map(|part| part.bytes().len() as u64)
        .sum::<u64>();
    if let Some((baseline, maximum)) = sparse_size_gate(&board.slug) {
        if total > maximum {
            return Err(AppError::developer_artifact(format!(
                "sparse payload is {total} bytes versus the {baseline}-byte merged baseline, and misses the 60% reduction gate (maximum {maximum})"
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
        "heltec-v4-r8" => Some((7_643_152, 3_057_260)),
        "t-beam-supreme" => Some((7_639_296, 3_055_718)),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use prns_flash_manifest::Transport;

    #[test]
    fn all_catalog_boards_have_a_build_recipe() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = prns_flash_manifest::board_catalog()?;
        assert_eq!(catalog.boards.len(), 9);
        assert!(catalog.boards.iter().all(|board| {
            matches!(
                (&board.transport, &board.build),
                (Transport::EspSerial, BoardBuild::Esp(_))
                    | (Transport::Uf2MassStorage, BoardBuild::Uf2(_))
                    | (Transport::NrfSerialDfu, BoardBuild::NrfSerialDfu(_))
            )
        }));
        Ok(())
    }

    #[test]
    fn device_bound_uf2_build_selects_only_the_compatible_variant(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let catalog = prns_flash_manifest::board_catalog()?;
        let board = catalog.board("t-echo").ok_or("missing T-Echo")?;
        let BoardBuild::Uf2(build) = &board.build else {
            return Err("T-Echo is not a UF2 build".into());
        };
        let v6 = SoftdeviceIdentity::parse("s140", "6.1.1")?;
        let selected = compatible_uf2_build_variants(build, Some(&v6));

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].softdevice_version, "6.1.1");
        assert_eq!(compatible_uf2_build_variants(build, None).len(), 2);
        Ok(())
    }

    #[test]
    fn a_rebootloadered_t114_matches_no_build_variant() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = prns_flash_manifest::board_catalog()?;
        let board = catalog.board("t114").ok_or("missing T114")?;
        let BoardBuild::Uf2(build) = &board.build else {
            return Err("T114 is not a UF2 build".into());
        };
        let stock = SoftdeviceIdentity::parse("s140", "6.1.1")?;
        let rebootloadered = SoftdeviceIdentity::parse("s140", "7.3.0")?;

        assert_eq!(compatible_uf2_build_variants(build, Some(&stock)).len(), 1);
        assert!(compatible_uf2_build_variants(build, Some(&rebootloadered)).is_empty());
        Ok(())
    }

    #[test]
    fn s3_size_gates_are_board_specific_and_at_least_sixty_percent() {
        assert_eq!(sparse_size_gate("heltec-v4"), Some((7_643_152, 3_057_260)));
        assert_eq!(
            sparse_size_gate("heltec-v4-r8"),
            Some((7_643_152, 3_057_260))
        );
        assert_eq!(
            sparse_size_gate("t-beam-supreme"),
            Some((7_639_296, 3_055_718))
        );
        assert_eq!(sparse_size_gate("xiao-esp32-c6"), None);
    }
}
