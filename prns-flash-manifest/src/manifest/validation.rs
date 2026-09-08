use std::collections::BTreeSet;
use std::fmt;

use crate::{
    BoardBuild, BoardCatalogEntry, EspSparseImageError, FlashPartKind, ImmutableArtifactPath,
    KeyId, ReleaseVersion, Sha256Digest, SoftdeviceIdentity,
};

use super::{FlashManifest, ManifestError, ManifestTargetSetPolicy, TargetManifest};

pub(super) fn validate_target(
    target: &TargetManifest,
    board: &BoardCatalogEntry,
    version: &str,
) -> Result<(), ManifestError> {
    validate_target_with_uf2_identity(target, board, version, None)
}

pub(super) fn validate_uf2_target_variant(
    target: &TargetManifest,
    board: &BoardCatalogEntry,
    version: &str,
    softdevice: &SoftdeviceIdentity,
) -> Result<(), ManifestError> {
    validate_target_with_uf2_identity(target, board, version, Some(softdevice))
}

fn validate_target_with_uf2_identity(
    target: &TargetManifest,
    board: &BoardCatalogEntry,
    version: &str,
    softdevice: Option<&SoftdeviceIdentity>,
) -> Result<(), ManifestError> {
    let pairs = [
        (
            target.display_name.as_str(),
            board.display_name.as_str(),
            "display_name",
        ),
        (target.silicon.as_str(), board.silicon.as_str(), "silicon"),
        (
            target.preparation_profile.as_str(),
            board.preparation_profile.as_str(),
            "preparation_profile",
        ),
    ];
    for (actual, expected, field) in pairs {
        if actual != expected {
            return Err(mismatch(target, field));
        }
    }
    if target.board_slug != board.slug
        || target.interfaces != board.interfaces
        || target.transport != board.transport
        || target.expected_chip != board.expected_chip
        || target.flash_size != board.flash_size
        || !provisioning_is_compatible(target.provisioning.as_ref(), board.provisioning.as_ref())
    {
        return Err(mismatch(target, "board transport/capability fields"));
    }
    match &board.build {
        BoardBuild::Esp(build)
            if target.flash_mode.as_deref() == Some(build.flash_mode.as_str())
                && target.flash_frequency.as_deref() == Some(build.flash_frequency.as_str())
                && target.before_reset.as_deref() == Some(build.before_reset.as_str())
                && target.after_reset.as_deref() == Some(build.after_reset.as_str())
                && target.variants.is_empty()
                && target.nrf_serial_dfu.is_none() => {}
        BoardBuild::Uf2(_)
            if target.flash_mode.is_none()
                && target.flash_frequency.is_none()
                && target.before_reset.is_none()
                && target.after_reset.is_none()
                && target.parts.is_empty()
                && target.nrf_serial_dfu.is_none() => {}
        BoardBuild::NrfSerialDfu(_)
            if target.flash_mode.is_none()
                && target.flash_frequency.is_none()
                && target.before_reset.is_none()
                && target.after_reset.is_none()
                && target.parts.is_empty()
                && target.variants.is_empty()
                && target.nrf_serial_dfu.is_some() => {}
        _ => return Err(mismatch(target, "flash/reset parameters")),
    }
    if target.source.is_some() {
        return Err(mismatch(target, "source archive capability"));
    }
    validate_payloads(target, board, version, softdevice)
}

fn provisioning_is_compatible(
    target: Option<&crate::ProvisioningDescriptor>,
    board: Option<&crate::ProvisioningDescriptor>,
) -> bool {
    match (target, board) {
        (None, None) => true,
        (Some(target), Some(board)) => {
            target.format == board.format
                && target.version == board.version
                && target.offset == board.offset
                && target.size == board.size
                && target.ssid_max_bytes == board.ssid_max_bytes
                && target.password_max_bytes == board.password_max_bytes
                && (target.tcp_client == board.tcp_client || target.tcp_client.is_none())
        }
        _ => false,
    }
}

pub(super) fn validate_release(manifest: &FlashManifest) -> Result<(), ManifestError> {
    let release = &manifest.release;
    ReleaseVersion::parse(release.version.clone()).map_err(release_domain_error)?;
    if release.commit.len() != 40 || !release.commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ManifestError::Release(
            "commit must be a full 40-character Git hash".to_string(),
        ));
    }
    KeyId::parse(manifest.signing.key_id.clone()).map_err(release_domain_error)?;
    Ok(())
}

pub(super) fn validate_version(version: &str) -> Result<(), ManifestError> {
    ReleaseVersion::parse(version.to_string())
        .map(|_| ())
        .map_err(release_domain_error)
}

pub(super) fn validate_sha256(value: &str) -> Result<(), String> {
    Sha256Digest::parse(value.to_string())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(super) fn validate_target_set(
    manifest: &FlashManifest,
    policy: &ManifestTargetSetPolicy,
) -> Result<(), ManifestError> {
    let actual = manifest
        .targets
        .iter()
        .map(|target| target.board_slug.as_str())
        .collect::<BTreeSet<_>>();
    if actual.len() != manifest.targets.len() {
        return Err(ManifestError::TargetSet("duplicate board slug".to_string()));
    }
    let expected = policy
        .expected
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(ManifestError::TargetSet(format!(
            "expected {expected:?}, found {actual:?}"
        )));
    }
    Ok(())
}

fn validate_payloads(
    target: &TargetManifest,
    board: &BoardCatalogEntry,
    version: &str,
    softdevice: Option<&SoftdeviceIdentity>,
) -> Result<(), ManifestError> {
    match &board.build {
        BoardBuild::Uf2(build) => {
            return validate_uf2_variants(target, build, version, softdevice);
        }
        BoardBuild::NrfSerialDfu(build) => {
            if softdevice.is_some() {
                return Err(mismatch(target, "UF2 compatibility selection"));
            }
            return validate_nrf_serial_dfu(target, build, version);
        }
        BoardBuild::Esp(_) => {}
    }
    if softdevice.is_some() {
        return Err(mismatch(target, "UF2 compatibility selection"));
    }
    let expected_prefix = format!("firmware/hopspot/{}/{version}/", target.board_slug);
    for part in &target.parts {
        if !part.path.starts_with(&expected_prefix) {
            return Err(invalid_part(
                target,
                &part.path,
                "path is not immutable and relative",
            ));
        }
    }
    crate::validate_esp_sparse_image(board, &target.parts).map_err(|error| {
        let message = error.to_string();
        match &error {
            EspSparseImageError::Build { .. }
            | EspSparseImageError::MissingFlashSize { .. }
            | EspSparseImageError::MemoryProfile(_) => mismatch(target, &message),
            EspSparseImageError::MissingParts | EspSparseImageError::PartOrder { .. } => {
                invalid_part(target, "", &message)
            }
            EspSparseImageError::Part { path, violation } => {
                invalid_part(target, path, &violation.to_string())
            }
        }
    })
}

fn validate_nrf_serial_dfu(
    target: &TargetManifest,
    build: &crate::NrfSerialDfuBuild,
    version: &str,
) -> Result<(), ManifestError> {
    let manifest = target
        .nrf_serial_dfu
        .as_ref()
        .ok_or_else(|| mismatch(target, "Nordic serial DFU artifact contract"))?;
    let expected_compatibility = build
        .manifest_compatibility()
        .map_err(|_| mismatch(target, "Nordic serial DFU memory profile"))?;
    if manifest.serial != build.serial
        || manifest.compatibility != expected_compatibility
        || manifest.recovery.mount_label != build.recovery.mount_label
        || manifest.recovery.board_id_prefix != build.recovery.board_identity.value
        || manifest.recovery.family_id != build.recovery.family_id
    {
        return Err(mismatch(target, "Nordic serial DFU compatibility"));
    }
    let expected_prefix = format!("firmware/hopspot/{}/{version}/", target.board_slug);
    let expected = [
        (
            &manifest.application,
            FlashPartKind::DfuApplication,
            build.application_filename.as_str(),
        ),
        (
            &manifest.init_packet,
            FlashPartKind::DfuInitPacket,
            build.init_packet_filename.as_str(),
        ),
        (
            &manifest.recovery.artifact,
            FlashPartKind::Uf2,
            build.recovery.filename.as_str(),
        ),
    ];
    let mut paths = BTreeSet::new();
    for (part, kind, filename) in expected {
        let expected_path = format!("{expected_prefix}{filename}");
        if part.kind != kind || part.path != expected_path || part.offset.is_some() {
            return Err(invalid_part(
                target,
                &part.path,
                "Nordic serial DFU artifact role or path disagrees with the catalog",
            ));
        }
        if part.size == 0 {
            return Err(invalid_part(target, &part.path, "size must be nonzero"));
        }
        if ImmutableArtifactPath::parse(part.path.clone()).is_err() {
            return Err(invalid_part(
                target,
                &part.path,
                "path is not immutable and relative",
            ));
        }
        if validate_sha256(&part.sha256).is_err() {
            return Err(invalid_part(
                target,
                &part.path,
                "SHA-256 must be lowercase hex",
            ));
        }
        if !paths.insert(part.path.as_str()) {
            return Err(invalid_part(
                target,
                &part.path,
                "artifact path is duplicated",
            ));
        }
    }
    let application_base =
        crate::canonical_hex::parse_u32(&manifest.compatibility.application_base)
            .ok_or_else(|| mismatch(target, "Nordic serial DFU application base"))?;
    let application_end =
        crate::canonical_hex::parse_u32(&manifest.compatibility.application_end_exclusive)
            .ok_or_else(|| mismatch(target, "Nordic serial DFU application end"))?;
    application_end
        .checked_sub(application_base)
        .ok_or_else(|| mismatch(target, "Nordic serial DFU application region"))?;
    let firmware_owned = build
        .memory_layout()
        .map_err(|_| mismatch(target, "Nordic serial DFU memory profile"))?
        .firmware_owned();
    if application_base != firmware_owned.start() {
        return Err(mismatch(target, "Nordic serial DFU firmware-owned region"));
    }
    let maximum_application_size = u64::from(firmware_owned.byte_len());
    if manifest.application.size > maximum_application_size {
        return Err(invalid_part(
            target,
            &manifest.application.path,
            "application exceeds the firmware-owned region",
        ));
    }
    Ok(())
}

fn validate_uf2_variants(
    target: &TargetManifest,
    build: &crate::Uf2Build,
    version: &str,
    softdevice: Option<&SoftdeviceIdentity>,
) -> Result<(), ManifestError> {
    if target.variants.is_empty() {
        return Err(invalid_part(
            target,
            "",
            "UF2 target requires a non-empty compatibility variant set",
        ));
    }
    let expected = match softdevice {
        Some(softdevice) => build
            .variants
            .iter()
            .filter(|variant| {
                variant.softdevice_family == softdevice.family().as_str()
                    && variant.softdevice_version == softdevice.version().as_str()
            })
            .collect::<Vec<_>>(),
        None => build.variants.iter().collect::<Vec<_>>(),
    };
    if expected.is_empty() || target.variants.len() != expected.len() {
        return Err(invalid_part(
            target,
            "",
            "UF2 compatibility variant set disagrees with the catalog",
        ));
    }
    let mut identities = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for (variant, expected) in target.variants.iter().zip(expected) {
        let expected_application = expected
            .memory_layout()
            .map_err(|_| mismatch(target, "UF2 memory profile"))?
            .transport_envelope();
        let identity = (
            variant.softdevice_family.as_str(),
            variant.softdevice_version.as_str(),
        );
        if !identities.insert(identity) {
            return Err(invalid_part(
                target,
                &variant.path,
                "UF2 compatibility key is duplicated",
            ));
        }
        let expected_path = format!(
            "firmware/hopspot/{}/{version}/{}",
            target.board_slug, expected.filename
        );
        if variant.softdevice_family != expected.softdevice_family
            || variant.softdevice_version != expected.softdevice_version
            || variant.fwid != expected.fwid
            || crate::canonical_hex::parse_u32(&variant.application_base)
                != Some(expected_application.start())
            || variant.family_id != expected.family_id
            || variant.path != expected_path
        {
            return Err(invalid_part(
                target,
                &variant.path,
                "UF2 compatibility metadata disagrees with the catalog",
            ));
        }
        if variant.size == 0 {
            return Err(invalid_part(target, &variant.path, "size must be nonzero"));
        }
        if ImmutableArtifactPath::parse(variant.path.clone()).is_err() {
            return Err(invalid_part(
                target,
                &variant.path,
                "path is not immutable and relative",
            ));
        }
        if validate_sha256(&variant.sha256).is_err() {
            return Err(invalid_part(
                target,
                &variant.path,
                "SHA-256 must be lowercase hex",
            ));
        }
        if !paths.insert(variant.path.as_str()) {
            return Err(invalid_part(
                target,
                &variant.path,
                "artifact path is duplicated",
            ));
        }
    }
    Ok(())
}

fn release_domain_error(error: impl fmt::Display) -> ManifestError {
    ManifestError::Release(error.to_string())
}

fn mismatch(target: &TargetManifest, field: &str) -> ManifestError {
    ManifestError::CatalogMismatch {
        board: target.board_slug.clone(),
        field: field.to_string(),
    }
}

fn invalid_part(target: &TargetManifest, path: &str, message: &str) -> ManifestError {
    ManifestError::InvalidPart {
        board: target.board_slug.clone(),
        path: path.to_string(),
        message: message.to_string(),
    }
}
