use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{
    BoardBuild, BoardCatalog, BoardCatalogEntry, FlashPart, FlashPartKind, ImmutableArtifactPath,
    KeyId, ReleaseVersion, Sha256Digest, Transport, CONFIG_OFFSET, ESP_FLASH_SECTOR_SIZE,
};

use super::{FlashManifest, ManifestError, TargetManifest};

pub(super) fn validate_target(
    target: &TargetManifest,
    board: &BoardCatalogEntry,
    version: &str,
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
        || target.provisioning != board.provisioning
    {
        return Err(mismatch(target, "board transport/capability fields"));
    }
    match &board.build {
        BoardBuild::Esp(build)
            if target.flash_mode.as_deref() == Some(build.flash_mode.as_str())
                && target.flash_frequency.as_deref() == Some(build.flash_frequency.as_str())
                && target.before_reset.as_deref() == Some(build.before_reset.as_str())
                && target.after_reset.as_deref() == Some(build.after_reset.as_str()) => {}
        BoardBuild::Uf2(_)
            if target.flash_mode.is_none()
                && target.flash_frequency.is_none()
                && target.before_reset.is_none()
                && target.after_reset.is_none() => {}
        _ => return Err(mismatch(target, "flash/reset parameters")),
    }
    validate_parts(target, version)
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
    catalog: &BoardCatalog,
) -> Result<(), ManifestError> {
    let actual = manifest
        .targets
        .iter()
        .map(|target| target.board_slug.as_str())
        .collect::<BTreeSet<_>>();
    if actual.len() != manifest.targets.len() {
        return Err(ManifestError::TargetSet("duplicate board slug".to_string()));
    }
    let expected = catalog
        .boards
        .iter()
        .map(|board| board.slug.as_str())
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(ManifestError::TargetSet(format!(
            "expected {expected:?}, found {actual:?}"
        )));
    }
    Ok(())
}

fn validate_parts(target: &TargetManifest, version: &str) -> Result<(), ManifestError> {
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
            Transport::Uf2MassStorage => {
                if part.kind != FlashPartKind::Uf2
                    || part.offset.is_some()
                    || target.parts.len() != 1
                {
                    return Err(invalid_part(
                        target,
                        &part.path,
                        "UF2 target must contain one offset-free UF2 part",
                    ));
                }
            }
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
