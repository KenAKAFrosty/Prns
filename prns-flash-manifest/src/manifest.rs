use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    BoardCatalog, ProvisioningDescriptor, Transport, ValidatedFlashManifest, FLASH_MANIFEST_SCHEMA,
};

mod conversion;
mod validation;

use conversion::{convert_channel_descriptor, convert_manifest};
use validation::{
    validate_release, validate_sha256, validate_target, validate_target_set, validate_version,
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
    /// Native source archive served by this exact target. Its absence means the target does not
    /// register source-download routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceArchiveIdentity>,
}

/// Identity of one commit-bound archive embedded in a source-capable target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveIdentity {
    /// Native NomadNet archive route.
    pub route: String,
    /// Native NomadNet checksum route.
    pub checksum_route: String,
    /// Exact embedded archive bytes.
    pub size: u64,
    /// SHA-256 of those exact bytes.
    pub sha256: String,
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
        // Every accepted wire document must also admit the transport-specific
        // domain conversion. Compatibility callers may retain the DTO, but no
        // signed parse can bypass this trust-boundary proof.
        let _ = convert_manifest(manifest.clone(), catalog)?;
        Ok(manifest)
    }

    /// Consume a schema-v2 wire DTO into the validated release-domain model.
    pub fn into_validated(
        self,
        catalog: &BoardCatalog,
    ) -> Result<ValidatedFlashManifest, ManifestError> {
        self.validate(catalog)?;
        convert_manifest(self, catalog)
    }

    /// Clone this DTO into the validated release-domain model.
    pub fn to_validated(
        &self,
        catalog: &BoardCatalog,
    ) -> Result<ValidatedFlashManifest, ManifestError> {
        self.clone().into_validated(catalog)
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
            validate_target(target, board, &self.release.version)?;
        }
        Ok(())
    }
}

impl ChannelDescriptor {
    /// Parse and validate a signed channel descriptor.
    pub fn from_json(bytes: &[u8], expected: ReleaseChannel) -> Result<Self, ManifestError> {
        let descriptor: Self = serde_json::from_slice(bytes)?;
        descriptor.validate(expected)?;
        let _ = convert_channel_descriptor(descriptor.clone())?;
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
    use crate::{board_catalog, BoardBuild, ReleaseTarget, CONFIG_OFFSET};

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
                    source: None,
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
    fn schema_two_wire_roundtrips_through_transport_typed_domain(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let wire = valid_manifest()?;
        let encoded = serde_json::to_vec(&wire)?;

        // Compatibility callers retain the exact schema-v2 DTO.
        assert_eq!(FlashManifest::from_json(&encoded, &catalog)?, wire);

        let validated = ValidatedFlashManifest::from_json(&encoded, &catalog)?;
        assert_eq!(validated.schema(), FLASH_MANIFEST_SCHEMA);
        assert_eq!(validated.release().version().as_str(), "0.2.6");
        assert_eq!(validated.targets().len(), 4);
        assert_eq!(
            validated
                .targets()
                .iter()
                .map(ReleaseTarget::to_wire)
                .collect::<Vec<_>>(),
            wire.targets
        );
        assert_eq!(
            validated
                .targets()
                .iter()
                .filter(|target| matches!(target, ReleaseTarget::EspSerial(_)))
                .count(),
            3
        );
        let t_echo = validated
            .targets()
            .iter()
            .find(|target| target.board_id().as_str() == "t-echo")
            .ok_or("missing typed T-Echo")?;
        let ReleaseTarget::Uf2(t_echo) = t_echo else {
            return Err("T-Echo did not convert to the UF2 variant".into());
        };
        assert_eq!(
            t_echo.part().path().as_str(),
            "firmware/hopspot/t-echo/0.2.6/firmware.uf2"
        );
        Ok(())
    }

    #[test]
    fn uf2_cannot_cross_the_domain_boundary_with_esp_only_fields(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let mut wire = valid_manifest()?;
        let target = wire
            .targets
            .iter_mut()
            .find(|target| target.board_slug == "t-echo")
            .ok_or("missing T-Echo")?;
        target.expected_chip = Some("esp32s3".to_string());
        target.flash_size = Some(8_388_608);
        assert!(ValidatedFlashManifest::from_json(&serde_json::to_vec(&wire)?, &catalog).is_err());
        Ok(())
    }

    #[test]
    fn domain_conversion_rechecks_provisioning_even_with_an_unvalidated_catalog(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut catalog = board_catalog()?;
        let mut wire = valid_manifest()?;
        let catalog_board = catalog
            .boards
            .iter_mut()
            .find(|board| board.slug == "heltec-v4")
            .ok_or("missing catalog board")?;
        let slot = catalog_board
            .provisioning
            .as_mut()
            .ok_or("missing catalog provisioning")?;
        slot.size /= 2;
        let target = wire
            .targets
            .iter_mut()
            .find(|target| target.board_slug == "heltec-v4")
            .ok_or("missing manifest target")?;
        target.provisioning = catalog_board.provisioning.clone();

        let encoded = serde_json::to_vec(&wire)?;
        assert!(wire.validate(&catalog).is_ok());
        assert!(ValidatedFlashManifest::from_json(&encoded, &catalog).is_err());
        assert!(FlashManifest::from_json(&encoded, &catalog).is_err());
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
    fn esp_offsets_must_match_flash_erase_geometry() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let mut manifest = valid_manifest()?;
        manifest.targets[0].parts[1].offset = Some(0x8001);
        assert!(matches!(
            manifest.validate(&catalog),
            Err(ManifestError::InvalidPart { .. })
        ));
        Ok(())
    }

    #[test]
    fn byte_disjoint_parts_cannot_share_an_erase_sector() -> Result<(), Box<dyn std::error::Error>>
    {
        let catalog = board_catalog()?;
        let mut manifest = valid_manifest()?;
        let target = &mut manifest.targets[0];
        target.parts[0].size = 0x1001;
        target.parts[1].offset = Some(0x1001);
        assert!(matches!(
            manifest.validate(&catalog),
            Err(ManifestError::InvalidPart { .. })
        ));
        Ok(())
    }

    #[test]
    fn sector_rounding_cannot_reach_the_configuration_slot(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let mut manifest = valid_manifest()?;
        let target = manifest
            .targets
            .iter_mut()
            .find(|target| target.board_slug == "heltec-v4")
            .ok_or("missing test target")?;
        target.parts[1].offset = Some(0xC000);
        target.parts[1].size = 0x1001;
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
