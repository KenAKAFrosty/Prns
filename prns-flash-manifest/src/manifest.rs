use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    BoardBuild, BoardCatalog, ProvisioningDescriptor, Transport, CONFIG_OFFSET,
    FLASH_MANIFEST_SCHEMA,
};

/// Signed release manifest consumed by every public flasher.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlashManifest {
    /// Manifest schema version.
    pub schema: u32,
    /// Immutable release identity.
    pub release: ReleaseInfo,
    /// Signing-key identity.
    pub signing: SigningInfo,
    /// Shipping targets.
    pub targets: Vec<TargetManifest>,
}

/// Signed pointer from a mutable channel name to one immutable release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelDescriptor {
    /// Channel descriptor schema version.
    pub schema: u32,
    /// Channel this document controls.
    pub channel: ReleaseChannel,
    /// Immutable release version.
    pub version: String,
    /// Absolute HTTPS URL for the immutable manifest.
    pub manifest_url: String,
    /// SHA-256 of the exact signed manifest bytes.
    pub manifest_sha256: String,
}

/// Immutable release identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseInfo {
    /// Suite version.
    pub version: String,
    /// Signed release channel.
    pub channel: ReleaseChannel,
    /// Full source commit.
    pub commit: String,
}

/// Allowed signed release channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseChannel {
    /// Public stable release.
    Stable,
    /// Signed preview release.
    Preview,
}

/// Identity of the offline signing key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SigningInfo {
    /// Maintainer-assigned key identifier.
    pub key_id: String,
}

/// Board-specific release plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetManifest {
    /// Stable board slug.
    pub board_slug: String,
    /// Display name captured for external clients.
    pub display_name: String,
    /// Silicon summary captured for external clients.
    pub silicon: String,
    /// Supported interfaces.
    pub interfaces: Vec<String>,
    /// Flashing transport.
    pub transport: Transport,
    /// Expected Espressif chip.
    pub expected_chip: Option<String>,
    /// Flash capacity.
    pub flash_size: Option<u32>,
    /// SPI mode for ESP targets.
    pub flash_mode: Option<String>,
    /// SPI frequency for ESP targets.
    pub flash_frequency: Option<String>,
    /// Pre-connect reset mode.
    pub before_reset: Option<String>,
    /// Post-flash reset mode.
    pub after_reset: Option<String>,
    /// Localized preparation-instruction key.
    pub preparation_profile: String,
    /// Ordered immutable release parts.
    pub parts: Vec<FlashPart>,
    /// Optional local provisioning slot.
    pub provisioning: Option<ProvisioningDescriptor>,
}

/// One immutable firmware payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlashPart {
    /// Semantic part kind.
    pub kind: FlashPartKind,
    /// Relative immutable release path.
    pub path: String,
    /// ESP flash offset; `None` for UF2.
    pub offset: Option<u32>,
    /// Exact byte length.
    pub size: u64,
    /// Lowercase SHA-256 digest.
    pub sha256: String,
}

/// Supported release payload kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlashPartKind {
    /// ESP bootloader image.
    Bootloader,
    /// ESP partition-table image.
    PartitionTable,
    /// ESP application image.
    Application,
    /// Nordic UF2 image.
    Uf2,
}

/// Manifest parsing or invariant failure.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// JSON could not be parsed.
    #[error("flash manifest is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Schema is unsupported.
    #[error("unsupported flash manifest schema {0}")]
    Schema(u32),
    /// Release identity is not publishable.
    #[error("invalid release identity: {0}")]
    Release(String),
    /// A target is duplicated or missing.
    #[error("manifest target set is invalid: {0}")]
    TargetSet(String),
    /// Target disagrees with the catalog.
    #[error("target {board:?} disagrees with the catalog: {field}")]
    CatalogMismatch {
        /// Board slug.
        board: String,
        /// Mismatched field.
        field: String,
    },
    /// A firmware part is invalid.
    #[error("target {board:?} part {path:?}: {message}")]
    InvalidPart {
        /// Board slug.
        board: String,
        /// Part path.
        path: String,
        /// Failure detail.
        message: String,
    },
}

impl FlashManifest {
    /// Parse and validate a signed manifest's JSON bytes against the catalog.
    pub fn from_json(bytes: &[u8], catalog: &BoardCatalog) -> Result<Self, ManifestError> {
        let manifest: Self = serde_json::from_slice(bytes)?;
        manifest.validate(catalog)?;
        Ok(manifest)
    }

    /// Validate release, target, and flash-layout invariants.
    pub fn validate(&self, catalog: &BoardCatalog) -> Result<(), ManifestError> {
        if self.schema != FLASH_MANIFEST_SCHEMA {
            return Err(ManifestError::Schema(self.schema));
        }
        validate_release(self)?;
        validate_target_set(self, catalog)?;
        for target in &self.targets {
            let board = catalog.board(&target.board_slug).ok_or_else(|| {
                ManifestError::TargetSet(format!("unknown board {:?}", target.board_slug))
            })?;
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
            if target.interfaces != board.interfaces
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
                        && target.flash_frequency.as_deref()
                            == Some(build.flash_frequency.as_str())
                        && target.before_reset.as_deref() == Some(build.before_reset.as_str())
                        && target.after_reset.as_deref() == Some(build.after_reset.as_str()) => {}
                BoardBuild::Uf2(_)
                    if target.flash_mode.is_none()
                        && target.flash_frequency.is_none()
                        && target.before_reset.is_none()
                        && target.after_reset.is_none() => {}
                _ => return Err(mismatch(target, "flash/reset parameters")),
            }
            validate_parts(target, &self.release.version)?;
        }
        Ok(())
    }
}

impl ChannelDescriptor {
    /// Parse and validate a signed channel descriptor.
    pub fn from_json(bytes: &[u8], expected: ReleaseChannel) -> Result<Self, ManifestError> {
        let descriptor: Self = serde_json::from_slice(bytes)?;
        descriptor.validate(expected)?;
        Ok(descriptor)
    }

    /// Validate channel identity, immutability, URL safety, and manifest digest.
    pub fn validate(&self, expected: ReleaseChannel) -> Result<(), ManifestError> {
        if self.schema != 1 {
            return Err(ManifestError::Release(format!(
                "unsupported channel descriptor schema {}",
                self.schema
            )));
        }
        if self.channel != expected {
            return Err(ManifestError::Release(
                "channel descriptor identity does not match the requested channel".to_string(),
            ));
        }
        validate_version(&self.version)?;
        if !self
            .manifest_url
            .starts_with("https://reticulum.rs/releases/")
            || self.manifest_url.contains([' ', '\\', '#', '?'])
            || !self
                .manifest_url
                .ends_with(&format!("/releases/{}/flash-manifest.json", self.version))
        {
            return Err(ManifestError::Release(
                "channel manifest URL is not an immutable HTTPS release path".to_string(),
            ));
        }
        validate_sha256(&self.manifest_sha256).map_err(ManifestError::Release)
    }
}

fn validate_release(manifest: &FlashManifest) -> Result<(), ManifestError> {
    let release = &manifest.release;
    validate_version(&release.version)?;
    if release.commit.len() != 40 || !release.commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ManifestError::Release(
            "commit must be a full 40-character Git hash".to_string(),
        ));
    }
    if manifest.signing.key_id.len() != 16
        || !manifest
            .signing
            .key_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ManifestError::Release(
            "signing key ID must be 16 hexadecimal characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<(), ManifestError> {
    let valid = !version.is_empty()
        && !version.eq_ignore_ascii_case("next")
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'));
    if valid {
        Ok(())
    } else {
        Err(ManifestError::Release(
            "version must be an immutable path-safe identifier".to_string(),
        ))
    }
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err("SHA-256 must be 64 lowercase hexadecimal characters".to_string())
    }
}

fn validate_target_set(
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
            || part.path.starts_with('/')
            || part.path.contains(['\\', '?', '#'])
            || part
                .path
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."))
            || part.path.contains("/latest/")
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
    let end = offset
        .checked_add(size)
        .ok_or_else(|| invalid_part(target, &part.path, "address range overflows"))?;
    let flash_size = target
        .flash_size
        .ok_or_else(|| invalid_part(target, &part.path, "ESP target has no flash size"))?;
    if end > flash_size {
        return Err(invalid_part(
            target,
            &part.path,
            "part exceeds physical flash",
        ));
    }
    let config_end = CONFIG_OFFSET + crate::CONFIG_SIZE as u32;
    if offset < config_end && CONFIG_OFFSET < end {
        return Err(invalid_part(
            target,
            &part.path,
            "part overlaps provisioning slot",
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
        if end > *next_offset {
            return Err(invalid_part(
                target,
                &part.path,
                &format!("overlaps {next_path:?}"),
            ));
        }
    }
    ranges.insert(offset, (end, part.path.as_str()));
    Ok(())
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

impl Ord for FlashPartKind {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (*self as u8).cmp(&(*other as u8))
    }
}

impl PartialOrd for FlashPartKind {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{board_catalog, BoardBuild};

    fn valid_manifest() -> Result<FlashManifest, crate::catalog::CatalogError> {
        let catalog = board_catalog()?;
        let targets = catalog
            .boards
            .iter()
            .map(|board| {
                let (flash_mode, flash_frequency, before_reset, after_reset, parts) =
                    match &board.build {
                        BoardBuild::Esp(build) => (
                            Some(build.flash_mode.clone()),
                            Some(build.flash_frequency.clone()),
                            Some(build.before_reset.clone()),
                            Some(build.after_reset.clone()),
                            vec![
                                part(board, FlashPartKind::Bootloader, "bootloader.bin", Some(0)),
                                part(
                                    board,
                                    FlashPartKind::PartitionTable,
                                    "partition-table.bin",
                                    Some(0x8000),
                                ),
                                part(
                                    board,
                                    FlashPartKind::Application,
                                    "application.bin",
                                    Some(0x10000),
                                ),
                            ],
                        ),
                        BoardBuild::Uf2(_) => (
                            None,
                            None,
                            None,
                            None,
                            vec![part(board, FlashPartKind::Uf2, "firmware.uf2", None)],
                        ),
                    };
                TargetManifest {
                    board_slug: board.slug.clone(),
                    display_name: board.display_name.clone(),
                    silicon: board.silicon.clone(),
                    interfaces: board.interfaces.clone(),
                    transport: board.transport,
                    expected_chip: board.expected_chip.clone(),
                    flash_size: board.flash_size,
                    flash_mode,
                    flash_frequency,
                    before_reset,
                    after_reset,
                    preparation_profile: board.preparation_profile.clone(),
                    parts,
                    provisioning: board.provisioning.clone(),
                }
            })
            .collect();
        Ok(FlashManifest {
            schema: FLASH_MANIFEST_SCHEMA,
            release: ReleaseInfo {
                version: "0.2.6".to_string(),
                channel: ReleaseChannel::Preview,
                commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
            signing: SigningInfo {
                key_id: "1FB2CA18B2C25E1F".to_string(),
            },
            targets,
        })
    }

    fn part(
        board: &crate::BoardCatalogEntry,
        kind: FlashPartKind,
        name: &str,
        offset: Option<u32>,
    ) -> FlashPart {
        FlashPart {
            kind,
            path: format!("firmware/hopspot/{}/0.2.6/{name}", board.slug),
            offset,
            size: 256,
            sha256: "a".repeat(64),
        }
    }

    #[test]
    fn valid_manifest_matches_catalog() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        valid_manifest()?.validate(&catalog)?;
        Ok(())
    }

    #[test]
    fn provisioning_overlap_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let mut manifest = valid_manifest()?;
        let target = manifest
            .targets
            .iter_mut()
            .find(|target| target.board_slug == "heltec-v4")
            .ok_or("missing test target")?;
        target.parts[1].offset = Some(CONFIG_OFFSET);
        assert!(matches!(
            manifest.validate(&catalog),
            Err(ManifestError::InvalidPart { .. })
        ));
        Ok(())
    }

    #[test]
    fn reserved_slot_is_protected_even_without_provisioning(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let mut manifest = valid_manifest()?;
        let target = manifest
            .targets
            .iter_mut()
            .find(|target| target.board_slug == "xiao-esp32-c6")
            .ok_or("missing test target")?;
        target.parts[1].offset = Some(CONFIG_OFFSET);
        assert!(matches!(
            manifest.validate(&catalog),
            Err(ManifestError::InvalidPart { .. })
        ));
        Ok(())
    }

    #[test]
    fn flash_parameters_must_match_the_catalog() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let mut manifest = valid_manifest()?;
        manifest.targets[0].after_reset = Some("unexpected-reset".to_string());
        assert!(matches!(
            manifest.validate(&catalog),
            Err(ManifestError::CatalogMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn sparse_parts_require_the_canonical_order() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let mut manifest = valid_manifest()?;
        manifest.targets[0].parts.swap(0, 1);
        assert!(matches!(
            manifest.validate(&catalog),
            Err(ManifestError::InvalidPart { .. })
        ));
        Ok(())
    }

    #[test]
    fn duplicate_artifact_paths_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let mut manifest = valid_manifest()?;
        let duplicate = manifest.targets[0].parts[0].path.clone();
        manifest.targets[0].parts[1].path = duplicate;
        assert!(matches!(
            manifest.validate(&catalog),
            Err(ManifestError::InvalidPart { .. })
        ));
        Ok(())
    }

    #[test]
    fn channel_descriptor_requires_an_immutable_https_manifest(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let valid = ChannelDescriptor {
            schema: 1,
            channel: ReleaseChannel::Stable,
            version: "0.2.6".to_string(),
            manifest_url: "https://reticulum.rs/releases/0.2.6/flash-manifest.json".to_string(),
            manifest_sha256: "a".repeat(64),
        };
        valid.validate(ReleaseChannel::Stable)?;

        let mut mutable = valid.clone();
        mutable.manifest_url = "https://reticulum.rs/flash-manifest.json".to_string();
        assert!(mutable.validate(ReleaseChannel::Stable).is_err());

        let mut foreign_host = valid.clone();
        foreign_host.manifest_url =
            "https://example.com/releases/0.2.6/flash-manifest.json".to_string();
        assert!(foreign_host.validate(ReleaseChannel::Stable).is_err());

        let encoded = serde_json::to_vec(&valid)?;
        assert_eq!(
            ChannelDescriptor::from_json(&encoded, ReleaseChannel::Stable)?,
            valid
        );
        Ok(())
    }
}
