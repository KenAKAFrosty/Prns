use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{
    BoardBuild, BoardCatalogEntry, FlashPart, FlashPartKind, ImmutableArtifactPath, KeyId,
    ReleaseVersion, Sha256Digest, SoftdeviceIdentity, Transport, CONFIG_OFFSET,
    ESP_FLASH_SECTOR_SIZE,
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
    if target.parts.is_empty() {
        return Err(invalid_part(target, "", "at least one part is required"));
    }
    let mut ranges = BTreeMap::<u32, (u32, &str)>::new();
    let mut paths = BTreeSet::new();
    for part in &target.parts {
        if part.size == 0 {
            return Err(invalid_part(target, &part.path, "size must be nonzero"));
        }
        if !part.path.starts_with(&expected_prefix)
            || ImmutableArtifactPath::parse(part.path.clone()).is_err()
        {
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
        match target.transport {
            Transport::EspSerial => validate_esp_part(target, part, &mut ranges)?,
            Transport::Uf2MassStorage => unreachable!(),
            Transport::NrfSerialDfu => unreachable!(),
        }
    }
    if target.transport == Transport::EspSerial {
        let kinds = target
            .parts
            .iter()
            .map(|part| part.kind)
            .collect::<Vec<_>>();
        let required = vec![
            FlashPartKind::Bootloader,
            FlashPartKind::PartitionTable,
            FlashPartKind::Application,
        ];
        if kinds != required {
            return Err(invalid_part(
                target,
                "",
                "ESP parts must be ordered bootloader, partition-table, application",
            ));
        }
    }
    Ok(())
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
    if manifest.serial != build.serial
        || manifest.compatibility != build.compatibility
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
    let application_base = parse_hex_u32(&manifest.compatibility.application_base)
        .ok_or_else(|| mismatch(target, "Nordic serial DFU application base"))?;
    let application_end = parse_hex_u32(&manifest.compatibility.application_end_exclusive)
        .ok_or_else(|| mismatch(target, "Nordic serial DFU application end"))?;
    let maximum_application_size = u64::from(
        application_end
            .checked_sub(application_base)
            .ok_or_else(|| mismatch(target, "Nordic serial DFU application region"))?,
    );
    if manifest.application.size > maximum_application_size {
        return Err(invalid_part(
            target,
            &manifest.application.path,
            "application exceeds the serial DFU region",
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
            || variant.application_base != expected.application_base
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

fn validate_esp_part<'a>(
    target: &'a TargetManifest,
    part: &'a FlashPart,
    ranges: &mut BTreeMap<u32, (u32, &'a str)>,
) -> Result<(), ManifestError> {
    let offset = part
        .offset
        .ok_or_else(|| invalid_part(target, &part.path, "ESP part requires an offset"))?;
    let size = u32::try_from(part.size)
        .map_err(|_| invalid_part(target, &part.path, "part is too large"))?;
    if offset % ESP_FLASH_SECTOR_SIZE != 0 {
        return Err(invalid_part(
            target,
            &part.path,
            "offset must be aligned to the 4 KiB flash erase sector",
        ));
    }
    let erase_size = size
        .checked_add(ESP_FLASH_SECTOR_SIZE - 1)
        .map(|rounded| rounded / ESP_FLASH_SECTOR_SIZE * ESP_FLASH_SECTOR_SIZE)
        .ok_or_else(|| invalid_part(target, &part.path, "erase footprint overflows"))?;
    let erase_end = offset
        .checked_add(erase_size)
        .ok_or_else(|| invalid_part(target, &part.path, "erase footprint overflows"))?;
    let flash_size = target
        .flash_size
        .ok_or_else(|| invalid_part(target, &part.path, "ESP target has no flash size"))?;
    if erase_end > flash_size {
        return Err(invalid_part(
            target,
            &part.path,
            "sector-rounded erase footprint exceeds physical flash",
        ));
    }
    let config_end = CONFIG_OFFSET + crate::CONFIG_SIZE as u32;
    if offset < config_end && CONFIG_OFFSET < erase_end {
        return Err(invalid_part(
            target,
            &part.path,
            "sector-rounded erase footprint overlaps provisioning slot",
        ));
    }
    if let Some((_, (previous_end, previous_path))) = ranges.range(..=offset).next_back() {
        if *previous_end > offset {
            return Err(invalid_part(
                target,
                &part.path,
                &format!("overlaps {previous_path:?}"),
            ));
        }
    }
    if let Some((next_offset, (_, next_path))) = ranges.range(offset..).next() {
        if erase_end > *next_offset {
            return Err(invalid_part(
                target,
                &part.path,
                &format!("overlaps {next_path:?}"),
            ));
        }
    }
    ranges.insert(offset, (erase_end, part.path.as_str()));
    Ok(())
}

fn parse_hex_u32(value: &str) -> Option<u32> {
    let digits = value.strip_prefix("0x")?;
    (digits.len() == 8
        && digits
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    .then(|| u32::from_str_radix(digits, 16).ok())
    .flatten()
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
