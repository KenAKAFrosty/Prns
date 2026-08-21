//! Verify signed release identity and bind its exact artifacts into a flashable target.

mod prepared;
mod source;

use std::fs;
use std::path::{Path, PathBuf};

use prns_flash_manifest::{
    pinned_key_id, pinned_key_is_configured, sha256_hex, verify_minisign, BoardCatalog,
    ReleaseChannel, ReleasePartRef, ReleaseTarget, SoftdeviceIdentity, ValidatedChannelDescriptor,
    ValidatedFlashManifest, PINNED_MINISIGN_PUBLIC_KEY,
};
use url::Url;

use crate::cache;
use crate::cli::ChannelArg;
use crate::error::AppError;
use crate::events::{Phase, Reporter};

#[cfg(test)]
use prepared::PreparedArtifactError;
pub(crate) use prepared::{
    PreparedEspTarget, PreparedNrfSerialDfuTarget, PreparedTarget, PreparedUf2Target,
};
#[cfg(test)]
use source::cached_channel_paths;
use source::{
    acquire, enforce_https, immutable_manifest_url, resolve_channel, validate_version,
    MAX_MANIFEST_BYTES, MAX_SIGNATURE_BYTES,
};

pub(crate) struct VerifiedReleaseTarget {
    version: prns_flash_manifest::ReleaseVersion,
    target: ReleaseTarget,
    source: VerifiedArtifactSource,
}

enum VerifiedArtifactSource {
    Candidate {
        root: PathBuf,
    },
    Published {
        base: Url,
        cache: PathBuf,
        offline: bool,
    },
}

pub(crate) fn verify_candidate_target(
    catalog: &BoardCatalog,
    board_slug: &str,
    channel: ChannelArg,
    candidate: &Path,
    reporter: Reporter,
) -> Result<VerifiedReleaseTarget, AppError> {
    if !pinned_key_is_configured() {
        return Err(AppError::trust_signing(
            "release key custody is not configured; release/keys/minisign.pub still contains the fail-closed marker",
        ));
    }
    let candidate_key = fs::read_to_string(candidate.join("minisign.pub")).map_err(|error| {
        AppError::trust_signing(format!(
            "could not read candidate Minisign public key: {error}"
        ))
    })?;
    if candidate_key != PINNED_MINISIGN_PUBLIC_KEY {
        return Err(AppError::trust_signing(
            "candidate public key differs from the CLI's pinned release key",
        ));
    }
    let channel_name = channel.as_str();
    let descriptor_path = candidate
        .join("channels")
        .join(format!("{channel_name}.json"));
    let descriptor_bytes = fs::read(&descriptor_path).map_err(|error| {
        AppError::trust_signing(format!("could not read signed candidate channel: {error}"))
    })?;
    verify_local_signature(&descriptor_path, &descriptor_bytes)?;
    let expected_channel = match channel {
        ChannelArg::Stable => ReleaseChannel::Stable,
        ChannelArg::Preview => ReleaseChannel::Preview,
    };
    let descriptor = ValidatedChannelDescriptor::from_json(&descriptor_bytes, expected_channel)
        .map_err(|error| AppError::trust_manifest(error.to_string()))?;

    reporter.phase(
        Phase::ValidatingManifest,
        Some(board_slug),
        &format!(
            "Verifying local signed Hopspot candidate {}…",
            descriptor.version()
        ),
    );
    let manifest_path = candidate.join("flash-manifest.json");
    let manifest_bytes = fs::read(&manifest_path).map_err(|error| {
        AppError::trust_manifest(format!("could not read candidate manifest: {error}"))
    })?;
    verify_local_signature(&manifest_path, &manifest_bytes)?;
    verify_hash(
        &manifest_bytes,
        descriptor.manifest_sha256().as_str(),
        "flash manifest",
    )?;
    let manifest = ValidatedFlashManifest::from_json(&manifest_bytes, catalog)
        .map_err(|error| AppError::trust_manifest(error.to_string()))?;
    verify_manifest_key_id(&manifest)?;
    if manifest.release().version() != descriptor.version()
        || manifest.release().channel() != expected_channel
    {
        return Err(AppError::trust_identity(
            "candidate channel and manifest release identity disagree",
        ));
    }
    let target = manifest
        .into_targets()
        .into_iter()
        .find(|target| target.board_id().as_str() == board_slug)
        .ok_or_else(|| {
            AppError::trust_identity(format!("candidate does not contain board {board_slug:?}"))
        })?;
    Ok(VerifiedReleaseTarget {
        version: descriptor.version().clone(),
        target,
        source: VerifiedArtifactSource::Candidate {
            root: candidate.to_path_buf(),
        },
    })
}

fn verify_local_signature(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let signature_path = PathBuf::from(format!("{}.minisig", path.display()));
    let signature = fs::read_to_string(&signature_path).map_err(|error| {
        AppError::trust_signing(format!(
            "could not read candidate signature {}: {error}",
            signature_path.display()
        ))
    })?;
    verify_minisign(bytes, &signature, PINNED_MINISIGN_PUBLIC_KEY)
        .map_err(|error| AppError::trust_signing(error.to_string()))
}

pub(crate) fn verify_published_target(
    catalog: &BoardCatalog,
    board_slug: &str,
    channel: ChannelArg,
    version: Option<&str>,
    offline: bool,
    reporter: Reporter,
) -> Result<VerifiedReleaseTarget, AppError> {
    if !pinned_key_is_configured() {
        return Err(AppError::trust_signing(
            "release key custody is not configured; release/keys/minisign.pub still contains the fail-closed marker",
        ));
    }
    let cache = cache::root()?;
    let (version, manifest_url, expected_manifest_hash) = match version {
        Some(version) => {
            let version = validate_version(version)?;
            let manifest_url = immutable_manifest_url(&version)?;
            (version, manifest_url, None)
        }
        None => resolve_channel(channel, offline, &cache, reporter)?,
    };

    reporter.phase(
        Phase::ValidatingManifest,
        Some(board_slug),
        &format!("Verifying signed Hopspot release {version}…"),
    );
    let manifest_cache = cache
        .join("releases")
        .join(version.as_str())
        .join("flash-manifest.json");
    let signature_cache = manifest_cache.with_extension("json.minisig");
    let manifest_bytes = acquire(
        &manifest_url,
        &manifest_cache,
        offline,
        MAX_MANIFEST_BYTES,
        &cache,
    )?;
    let signature_url = format!("{manifest_url}.minisig");
    let signature_bytes = acquire(
        &signature_url,
        &signature_cache,
        offline,
        MAX_SIGNATURE_BYTES,
        &cache,
    )?;
    let signature = String::from_utf8(signature_bytes).map_err(|error| {
        AppError::trust_signing(format!("manifest signature is not UTF-8: {error}"))
    })?;
    verify_minisign(&manifest_bytes, &signature, PINNED_MINISIGN_PUBLIC_KEY)
        .map_err(|error| AppError::trust_signing(error.to_string()))?;
    if let Some(expected_hash) = expected_manifest_hash {
        verify_hash(&manifest_bytes, expected_hash.as_str(), "flash manifest")?;
    }
    let manifest = ValidatedFlashManifest::from_json(&manifest_bytes, catalog)
        .map_err(|error| AppError::trust_manifest(error.to_string()))?;
    verify_manifest_key_id(&manifest)?;
    if manifest.release().version() != &version {
        return Err(AppError::trust_identity(format!(
            "signed manifest version {:?} does not match selected release {:?}",
            manifest.release().version().as_str(),
            version.as_str()
        )));
    }
    let expected_channel = match channel {
        ChannelArg::Stable => ReleaseChannel::Stable,
        ChannelArg::Preview => ReleaseChannel::Preview,
    };
    if manifest.release().channel() != expected_channel {
        return Err(AppError::trust_identity(format!(
            "signed manifest channel {:?} does not match requested channel {:?}",
            manifest.release().channel(),
            expected_channel
        )));
    }
    if !offline {
        cache::store_immutable(&cache, &manifest_cache, &manifest_bytes)?;
        cache::store_immutable(&cache, &signature_cache, signature.as_bytes())?;
    }
    let target = manifest
        .into_targets()
        .into_iter()
        .find(|target| target.board_id().as_str() == board_slug)
        .ok_or_else(|| {
            AppError::trust_identity(format!("release does not contain board {board_slug:?}"))
        })?;

    let base = Url::parse(&manifest_url)
        .map_err(|error| AppError::trust_manifest(format!("invalid manifest URL: {error}")))?;
    Ok(VerifiedReleaseTarget {
        version,
        target,
        source: VerifiedArtifactSource::Published {
            base,
            cache,
            offline,
        },
    })
}

impl VerifiedReleaseTarget {
    pub(crate) fn prepare(
        self,
        softdevice: Option<&SoftdeviceIdentity>,
        reporter: Reporter,
    ) -> Result<PreparedTarget, AppError> {
        let Self {
            version,
            target,
            source,
        } = self;
        let board_slug = target.board_id().as_str().to_string();
        let target_parts = selected_target_parts(&target, softdevice)?;
        let mut artifacts = Vec::with_capacity(target_parts.len());
        for part in target_parts {
            let part_path = part.path().as_str();
            let (bytes, cache_destination) = match &source {
                VerifiedArtifactSource::Candidate { root } => {
                    reporter.phase(
                        Phase::VerifyingArtifacts,
                        Some(&board_slug),
                        &format!("Verifying local {} ({} bytes)…", part_path, part.size()),
                    );
                    let path = root.join(part_path);
                    let bytes = fs::read(&path).map_err(|error| {
                        AppError::trust_artifact(format!(
                            "could not read candidate artifact {}: {error}",
                            path.display()
                        ))
                    })?;
                    (bytes, None)
                }
                VerifiedArtifactSource::Published {
                    base,
                    cache,
                    offline,
                } => {
                    reporter.phase(
                        Phase::Downloading,
                        Some(&board_slug),
                        &format!("Acquiring {} ({} bytes)…", part_path, part.size()),
                    );
                    let part_url = base.join(part_path).map_err(|error| {
                        AppError::trust_artifact(format!("invalid artifact URL: {error}"))
                    })?;
                    enforce_https(&part_url)?;
                    let file_name = Path::new(part_path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| {
                            AppError::trust_artifact(format!("invalid artifact path {part_path:?}"))
                        })?;
                    let part_cache = cache
                        .join("releases")
                        .join(version.as_str())
                        .join(&board_slug)
                        .join(file_name);
                    let limit = part.size().checked_add(1).ok_or_else(|| {
                        AppError::trust_artifact("artifact size overflows download limit")
                    })?;
                    let bytes = acquire(part_url.as_str(), &part_cache, *offline, limit, cache)?;
                    let cache_destination = (!offline).then_some((cache.as_path(), part_cache));
                    (bytes, cache_destination)
                }
            };
            if bytes.len() as u64 != part.size() {
                return Err(AppError::trust_artifact(format!(
                    "artifact {:?} is {} bytes; signed manifest requires {}",
                    part_path,
                    bytes.len(),
                    part.size()
                )));
            }
            verify_hash(&bytes, part.sha256().as_str(), part_path)?;
            if let Some((cache, part_cache)) = cache_destination {
                cache::store_immutable(cache, &part_cache, &bytes)?;
            }
            artifacts.push(bytes);
        }
        bind_prepared_target(version, target, softdevice, artifacts)
    }
}

fn selected_target_parts<'a>(
    target: &'a ReleaseTarget,
    softdevice: Option<&SoftdeviceIdentity>,
) -> Result<Vec<ReleasePartRef<'a>>, AppError> {
    match target {
        ReleaseTarget::EspSerial(_) => {
            if softdevice.is_some() {
                return Err(AppError::trust_identity(
                    "ESP target cannot use a SoftDevice compatibility selection",
                ));
            }
            Ok(target.parts())
        }
        ReleaseTarget::Uf2(target) => {
            let softdevice = softdevice.ok_or_else(|| {
                AppError::trust_identity(
                    "UF2 target requires a detected SoftDevice compatibility selection",
                )
            })?;
            let variant = target.variant_for(softdevice).ok_or_else(|| {
                AppError::trust_identity(format!(
                    "signed release has no UF2 variant for detected {softdevice}"
                ))
            })?;
            Ok(vec![ReleasePartRef::Uf2(variant.part())])
        }
        ReleaseTarget::NrfSerialDfu(target) => {
            if softdevice.is_some() {
                return Err(AppError::trust_identity(
                    "Nordic serial DFU target cannot use a UF2 compatibility selection",
                ));
            }
            Ok(vec![
                ReleasePartRef::NrfSerialDfu(target.application()),
                ReleasePartRef::NrfSerialDfu(target.init_packet()),
            ])
        }
    }
}

fn bind_prepared_target(
    version: prns_flash_manifest::ReleaseVersion,
    target: ReleaseTarget,
    softdevice: Option<&SoftdeviceIdentity>,
    artifacts: Vec<Vec<u8>>,
) -> Result<PreparedTarget, AppError> {
    match softdevice {
        Some(softdevice) => {
            let [bytes] = <[Vec<u8>; 1]>::try_from(artifacts).map_err(|artifacts| {
                AppError::trust_artifact(format!(
                    "selected UF2 variant produced {} artifacts instead of one",
                    artifacts.len()
                ))
            })?;
            PreparedTarget::bind_uf2(version, target, softdevice, bytes)
                .map_err(|error| AppError::trust_artifact(error.to_string()))
        }
        None => PreparedTarget::bind(version, target, artifacts)
            .map_err(|error| AppError::trust_artifact(error.to_string())),
    }
}

fn verify_manifest_key_id(manifest: &ValidatedFlashManifest) -> Result<(), AppError> {
    let expected = pinned_key_id()
        .ok_or_else(|| AppError::trust_signing("pinned release key has no canonical key ID"))?;
    if manifest.signing().key_id().as_str() == expected.to_ascii_uppercase() {
        Ok(())
    } else {
        Err(AppError::trust_signing(format!(
            "manifest key ID {:?} differs from pinned key {expected}",
            manifest.signing().key_id().as_str()
        )))
    }
}

fn verify_hash(bytes: &[u8], expected: &str, label: &str) -> Result<(), AppError> {
    let actual = sha256_hex(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(AppError::trust_artifact(format!(
            "SHA-256 mismatch for {label}: expected {expected}, found {actual}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_flash_manifest::{
        board_catalog, BoardBuild, FlashPart, FlashPartKind, NrfSerialDfuManifest,
        NrfSerialDfuRecoveryManifest, ReleaseTarget, ReleaseVersion, SoftdeviceIdentity,
        TargetManifest, Uf2VariantManifest,
    };
    use prns_nrf_dfu::{
        ApplicationInitPacket, ApplicationInitPacketSpec, ApplicationVersion, DfuDeviceRevision,
        DfuDeviceType, SoftdeviceFirmwareId, SoftdeviceRequirements,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn esp_target(artifacts: &[&[u8]; 3]) -> (ReleaseVersion, ReleaseTarget) {
        let catalog = board_catalog().expect("catalog");
        let board = catalog.board("heltec-v4").expect("Heltec catalog entry");
        let BoardBuild::Esp(build) = &board.build else {
            panic!("Heltec must use the ESP build");
        };
        let version = ReleaseVersion::parse("0.2.6").expect("release version");
        let recipes = [
            (FlashPartKind::Bootloader, "bootloader.bin", 0),
            (FlashPartKind::PartitionTable, "partition-table.bin", 0x8000),
            (FlashPartKind::Application, "application.bin", 0x10000),
        ];
        let parts = recipes
            .into_iter()
            .zip(artifacts)
            .map(|((kind, name, offset), bytes)| FlashPart {
                kind,
                path: format!("firmware/hopspot/heltec-v4/0.2.6/{name}"),
                offset: Some(offset),
                size: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            })
            .collect();
        let target = TargetManifest {
            board_slug: board.slug.clone(),
            display_name: board.display_name.clone(),
            silicon: board.silicon.clone(),
            interfaces: board.interfaces.clone(),
            transport: board.transport,
            expected_chip: board.expected_chip.clone(),
            flash_size: board.flash_size,
            flash_mode: Some(build.flash_mode.clone()),
            flash_frequency: Some(build.flash_frequency.clone()),
            before_reset: Some(build.before_reset.clone()),
            after_reset: Some(build.after_reset.clone()),
            preparation_profile: board.preparation_profile.clone(),
            parts,
            variants: Vec::new(),
            nrf_serial_dfu: None,
            provisioning: board.provisioning.clone(),
            source: None,
        }
        .into_validated(board, &version)
        .expect("typed ESP target");
        (version, target)
    }

    fn uf2_target(seed: u8) -> (ReleaseVersion, ReleaseTarget, SoftdeviceIdentity, Vec<u8>) {
        let catalog = board_catalog().expect("catalog");
        let board = catalog.board("t-echo").expect("T-Echo catalog entry");
        let BoardBuild::Uf2(build) = &board.build else {
            panic!("T-Echo must use the UF2 build");
        };
        let version = ReleaseVersion::parse("0.2.6").expect("release version");
        let artifacts = build
            .variants
            .iter()
            .map(|variant| {
                let base = u32::from_str_radix(
                    variant
                        .application_base
                        .strip_prefix("0x")
                        .expect("hex base"),
                    16,
                )
                .expect("base");
                let family = u32::from_str_radix(
                    variant.family_id.strip_prefix("0x").expect("hex family"),
                    16,
                )
                .expect("family");
                test_uf2(base, family, seed)
            })
            .collect::<Vec<_>>();
        let target = TargetManifest {
            board_slug: board.slug.clone(),
            display_name: board.display_name.clone(),
            silicon: board.silicon.clone(),
            interfaces: board.interfaces.clone(),
            transport: board.transport,
            expected_chip: None,
            flash_size: None,
            flash_mode: None,
            flash_frequency: None,
            before_reset: None,
            after_reset: None,
            preparation_profile: board.preparation_profile.clone(),
            parts: Vec::new(),
            variants: build
                .variants
                .iter()
                .zip(&artifacts)
                .map(|(variant, bytes)| Uf2VariantManifest {
                    softdevice_family: variant.softdevice_family.clone(),
                    softdevice_version: variant.softdevice_version.clone(),
                    fwid: variant.fwid.clone(),
                    application_base: variant.application_base.clone(),
                    family_id: variant.family_id.clone(),
                    path: format!("firmware/hopspot/t-echo/0.2.6/{}", variant.filename),
                    size: bytes.len() as u64,
                    sha256: sha256_hex(bytes),
                })
                .collect(),
            nrf_serial_dfu: None,
            provisioning: None,
            source: None,
        }
        .into_validated(board, &version)
        .expect("typed UF2 target");
        let softdevice = SoftdeviceIdentity::parse("s140", "7.3.0").expect("identity");
        (version, target, softdevice, artifacts[1].clone())
    }

    fn nrf_target() -> (ReleaseVersion, ReleaseTarget, Vec<u8>, Vec<u8>) {
        let catalog = board_catalog().expect("catalog");
        let board = catalog.board("t1000-e").expect("T1000-E catalog entry");
        let BoardBuild::NrfSerialDfu(build) = &board.build else {
            panic!("T1000-E must use the Nordic serial DFU build");
        };
        let version = ReleaseVersion::parse("0.2.6").expect("release version");
        let application = vec![0x5a; 513];
        let fwid = SoftdeviceFirmwareId::new(0x0123).expect("FWID");
        let init_packet = ApplicationInitPacket::build(
            &application,
            &ApplicationInitPacketSpec {
                device_type: DfuDeviceType::new(0x0052),
                device_revision: DfuDeviceRevision::new(52840),
                application_version: ApplicationVersion::NotEnforced,
                softdevices: SoftdeviceRequirements::new(fwid, std::iter::empty())
                    .expect("SoftDevice requirements"),
            },
        )
        .expect("init packet")
        .bytes()
        .to_vec();
        let artifact = |kind, name: &str, bytes: &[u8]| FlashPart {
            kind,
            path: format!("firmware/hopspot/t1000-e/0.2.6/{name}"),
            offset: None,
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        };
        let recovery = b"recovery";
        let target = TargetManifest {
            board_slug: board.slug.clone(),
            display_name: board.display_name.clone(),
            silicon: board.silicon.clone(),
            interfaces: board.interfaces.clone(),
            transport: board.transport,
            expected_chip: None,
            flash_size: None,
            flash_mode: None,
            flash_frequency: None,
            before_reset: None,
            after_reset: None,
            preparation_profile: board.preparation_profile.clone(),
            parts: Vec::new(),
            variants: Vec::new(),
            nrf_serial_dfu: Some(NrfSerialDfuManifest {
                serial: build.serial.clone(),
                compatibility: build.compatibility.clone(),
                application: artifact(
                    FlashPartKind::DfuApplication,
                    &build.application_filename,
                    &application,
                ),
                init_packet: artifact(
                    FlashPartKind::DfuInitPacket,
                    &build.init_packet_filename,
                    &init_packet,
                ),
                recovery: NrfSerialDfuRecoveryManifest {
                    mount_label: build.recovery.mount_label.clone(),
                    board_id_prefix: build.recovery.board_identity.value.clone(),
                    family_id: build.recovery.family_id.clone(),
                    artifact: artifact(FlashPartKind::Uf2, &build.recovery.filename, recovery),
                },
            }),
            provisioning: None,
            source: None,
        }
        .into_validated(board, &version)
        .expect("typed Nordic serial DFU target");
        (version, target, application, init_packet)
    }

    fn test_uf2(base: u32, family: u32, seed: u8) -> Vec<u8> {
        let mut block = vec![0u8; 512];
        for (offset, value) in [
            (0, 0x0a32_4655),
            (4, 0x9e5d_5157),
            (8, 0x0000_2000),
            (12, base),
            (16, 256),
            (20, 0),
            (24, 1),
            (28, family),
            (508, 0x0ab1_6f30),
        ] {
            block[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        block[32..288].fill(seed);
        block
    }

    fn temporary_cache() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("hopspot-flash-cache-{nonce}"))
    }

    #[test]
    fn versions_cannot_escape_release_paths() {
        assert!(validate_version("0.2.6").is_ok());
        assert!(validate_version("../latest").is_err());
        assert!(validate_version("next").is_err());
    }

    #[test]
    fn hash_mismatch_is_a_trust_error() {
        assert!(matches!(
            verify_hash(b"payload", &"0".repeat(64), "test"),
            Err(AppError::Trust(_))
        ));
    }

    #[test]
    fn prepared_esp_parts_bind_signed_offsets_to_exact_bytes() {
        let artifacts: [&[u8]; 3] = [b"boot", b"partition", b"application"];
        let (version, target) = esp_target(&artifacts);
        let prepared = PreparedTarget::bind(
            version,
            target,
            artifacts.iter().map(|bytes| bytes.to_vec()).collect(),
        )
        .expect("bind signed artifacts");
        let PreparedTarget::EspSerial(prepared) = prepared else {
            panic!("expected ESP prepared target");
        };
        assert_eq!(
            prepared
                .parts()
                .iter()
                .map(|part| (part.offset(), part.bytes()))
                .collect::<Vec<_>>(),
            vec![
                (0, b"boot".as_slice()),
                (0x8000, b"partition".as_slice()),
                (0x10000, b"application".as_slice()),
            ]
        );
    }

    #[test]
    fn prepared_target_rejects_reordered_or_missing_byte_buffers() {
        let artifacts: [&[u8]; 3] = [b"boot", b"partition", b"application"];
        let (version, target) = esp_target(&artifacts);
        assert!(matches!(
            PreparedTarget::bind(
                version,
                target,
                vec![
                    b"partition".to_vec(),
                    b"boot".to_vec(),
                    b"application".to_vec()
                ],
            ),
            Err(PreparedArtifactError::Size { .. } | PreparedArtifactError::Hash { .. })
        ));

        let (version, target) = esp_target(&artifacts);
        assert!(matches!(
            PreparedTarget::bind(
                version,
                target,
                vec![b"boot".to_vec(), b"partition".to_vec()],
            ),
            Err(PreparedArtifactError::Count {
                expected: 3,
                actual: 2,
                ..
            })
        ));
    }

    #[test]
    fn prepared_uf2_target_binds_exactly_one_signed_payload() {
        let (version, target, softdevice, bytes) = uf2_target(0xa5);
        let prepared = PreparedTarget::bind_uf2(version, target, &softdevice, bytes.clone())
            .expect("bind signed UF2");
        let PreparedTarget::Uf2(prepared) = prepared else {
            panic!("expected UF2 prepared target");
        };
        assert_eq!(prepared.part().bytes(), bytes);

        let (version, target, softdevice, bytes) = uf2_target(0xa5);
        let mut forged = bytes;
        forged[32] ^= 1;
        assert!(matches!(
            PreparedTarget::bind_uf2(version, target, &softdevice, forged),
            Err(PreparedArtifactError::Hash { .. })
        ));

        let (version, target, _, bytes) = uf2_target(0xa5);
        assert!(matches!(
            PreparedTarget::bind(version, target, vec![bytes]),
            Err(PreparedArtifactError::CompatibilityRequired(_))
        ));
    }

    #[test]
    fn prepared_nordic_target_binds_only_the_delivery_artifacts() {
        let (version, target, application, init_packet) = nrf_target();
        let parts = selected_target_parts(&target, None).expect("selected artifacts");
        assert_eq!(
            parts.iter().map(|part| part.kind()).collect::<Vec<_>>(),
            [FlashPartKind::DfuApplication, FlashPartKind::DfuInitPacket]
        );
        let prepared =
            PreparedTarget::bind(version, target, vec![application.clone(), init_packet])
                .expect("prepared Nordic serial DFU target");
        let PreparedTarget::NrfSerialDfu(prepared) = prepared else {
            panic!("expected Nordic serial DFU target");
        };
        assert_eq!(prepared.image().expect("DFU image").firmware(), application);
    }

    #[test]
    fn verified_cache_publication_is_atomic_and_immutable() -> Result<(), AppError> {
        let root = temporary_cache();
        let path = root.join("releases/0.2.6/application.bin");
        cache::store_immutable(&root, &path, b"verified")?;
        assert_eq!(fs::read(&path).expect("read cache"), b"verified");
        cache::store_immutable(&root, &path, b"verified")?;
        assert!(matches!(
            cache::store_immutable(&root, &path, b"different"),
            Err(AppError::Trust(_))
        ));
        fs::remove_dir_all(root).expect("remove cache fixture");
        Ok(())
    }

    #[test]
    fn offline_cache_never_falls_back_to_network() {
        let root = temporary_cache();
        let missing = root.join("missing.bin");
        assert!(matches!(
            acquire(
                "https://example.invalid/missing.bin",
                &missing,
                true,
                64,
                &root,
            ),
            Err(AppError::Trust(_))
        ));
    }

    #[test]
    fn offline_cache_enforces_the_signed_size_limit() {
        let root = temporary_cache();
        fs::create_dir_all(&root).expect("create cache fixture");
        let oversized = root.join("oversized.bin");
        fs::write(&oversized, [0u8; 65]).expect("write oversized cache entry");
        assert!(matches!(
            acquire(
                "https://example.invalid/oversized.bin",
                &oversized,
                true,
                64,
                &root,
            ),
            Err(AppError::Trust(_))
        ));
        fs::remove_dir_all(root).expect("remove cache fixture");
    }

    #[cfg(unix)]
    #[test]
    fn offline_cache_rejects_symbolic_links() {
        use std::os::unix::fs::symlink;

        let root = temporary_cache();
        fs::create_dir_all(&root).expect("create cache fixture");
        let target = root.join("target.bin");
        let linked = root.join("linked.bin");
        fs::write(&target, b"cached").expect("write cache target");
        symlink(&target, &linked).expect("create cache symlink");
        assert!(matches!(
            acquire(
                "https://example.invalid/linked.bin",
                &linked,
                true,
                64,
                &root,
            ),
            Err(AppError::Trust(_))
        ));
        fs::remove_dir_all(root).expect("remove cache fixture");
    }

    #[cfg(unix)]
    #[test]
    fn offline_cache_rejects_symlinked_parent_directories() {
        use std::os::unix::fs::symlink;

        let root = temporary_cache();
        let outside = root.with_extension("outside");
        fs::create_dir_all(&root).expect("create cache fixture");
        fs::create_dir_all(outside.join("0.2.6/heltec-v4")).expect("create outside fixture");
        fs::write(
            outside.join("0.2.6/heltec-v4/application.bin"),
            b"external bytes",
        )
        .expect("write outside bytes");
        symlink(&outside, root.join("releases")).expect("create cache parent symlink");
        let redirected = root.join("releases/0.2.6/heltec-v4/application.bin");

        assert!(matches!(
            acquire(
                "https://example.invalid/application.bin",
                &redirected,
                true,
                64,
                &root,
            ),
            Err(AppError::Trust(_))
        ));
        fs::remove_dir_all(root).expect("remove cache fixture");
        fs::remove_dir_all(outside).expect("remove outside fixture");
    }

    #[test]
    fn offline_channel_uses_only_the_explicit_authenticated_head() {
        let root = temporary_cache();
        let directory = root.join("channels/preview");
        fs::create_dir_all(&directory).expect("create channel cache");
        let older = sha256_hex(b"older descriptor");
        let selected = sha256_hex(b"selected descriptor");
        fs::write(directory.join(format!("{older}.json")), b"older descriptor")
            .expect("write old descriptor");
        fs::write(
            directory.join(format!("{selected}.json")),
            b"selected descriptor",
        )
        .expect("write selected descriptor");
        fs::write(directory.join("HEAD"), format!("{selected}\n")).expect("write channel head");

        let (identifier, descriptor, signature) =
            cached_channel_paths(&root, &directory).expect("resolve channel head");
        assert_eq!(identifier, selected);
        assert_eq!(descriptor, directory.join(format!("{selected}.json")));
        assert_eq!(
            signature,
            directory.join(format!("{selected}.json.minisig"))
        );
        fs::remove_dir_all(root).expect("remove cache fixture");
    }

    #[test]
    fn malformed_offline_channel_head_fails_without_descriptor_fallback() {
        let root = temporary_cache();
        let directory = root.join("channels/stable");
        fs::create_dir_all(&directory).expect("create channel cache");
        let descriptor = sha256_hex(b"otherwise valid cached descriptor");
        fs::write(
            directory.join(format!("{descriptor}.json")),
            b"otherwise valid cached descriptor",
        )
        .expect("write cached descriptor");
        fs::write(directory.join("HEAD"), b"corrupt-head\n").expect("write corrupt head");

        assert!(matches!(
            cached_channel_paths(&root, &directory),
            Err(AppError::Trust(_))
        ));
        fs::remove_dir_all(root).expect("remove cache fixture");
    }
}
