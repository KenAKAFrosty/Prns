use std::collections::BTreeSet;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlashManifest {
    #[serde(rename = "schema")]
    pub schema_version: u32,
    pub release: ReleaseInfo,
    pub signing: OfflineKeySigningInfo,
    pub targets: Vec<TargetManifest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelDescriptor {
    #[serde(rename = "schema")]
    pub schema_version: u32,
    pub channel: ReleaseChannel,
    pub version: String,
    pub manifest_url: String,
    pub manifest_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseInfo {
    pub version: String,
    pub channel: ReleaseChannel,
    pub commit: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseChannel {
    Stable,
    Preview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfflineKeySigningInfo {
    pub key_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetManifest {
    pub board_slug: String,
    pub display_name: String,
    pub silicon: String,
    pub interfaces: Vec<String>,
    pub transport: Transport,
    pub expected_chip: Option<String>,
    pub flash_size: Option<u32>,
    pub flash_mode: Option<String>,
    pub flash_frequency: Option<String>,
    pub before_reset: Option<String>,
    pub after_reset: Option<String>,
    pub preparation_profile: String,
    pub parts: Vec<FlashPart>,
    pub variants: Vec<Uf2VariantManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nrf_serial_dfu: Option<NrfSerialDfuManifest>,
    pub provisioning: Option<ProvisioningDescriptor>,
    /// Native source archive served by this exact target. Its absence means the target does not
    /// register source-download routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceArchiveIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NrfSerialDfuManifest {
    pub serial: crate::NrfSerialDfuSerialTransport,
    pub compatibility: crate::NrfSerialDfuCompatibility,
    pub application: FlashPart,
    pub init_packet: FlashPart,
    pub recovery: NrfSerialDfuRecoveryManifest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NrfSerialDfuRecoveryManifest {
    pub mount_label: String,
    pub board_id_prefix: String,
    pub family_id: String,
    pub artifact: FlashPart,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uf2VariantManifest {
    pub softdevice_family: String,
    pub softdevice_version: String,
    pub fwid: String,
    pub application_base: String,
    pub family_id: String,
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestTargetSetPolicy {
    expected: BTreeSet<String>,
}

impl ManifestTargetSetPolicy {
    pub fn all_shipping_targets(catalog: &BoardCatalog) -> Self {
        Self {
            expected: catalog
                .shipping_boards()
                .map(|board| board.slug.clone())
                .collect(),
        }
    }

    pub fn local_development(
        catalog: &BoardCatalog,
        board_slugs: &[&str],
    ) -> Result<Self, ManifestError> {
        if board_slugs.is_empty() {
            return Err(ManifestError::TargetSet(
                "local development target set must not be empty".to_string(),
            ));
        }
        let expected = board_slugs
            .iter()
            .map(|slug| (*slug).to_string())
            .collect::<BTreeSet<_>>();
        if expected.len() != board_slugs.len() {
            return Err(ManifestError::TargetSet(
                "local development target set contains a duplicate board slug".to_string(),
            ));
        }
        if let Some(slug) = expected
            .iter()
            .find(|slug| catalog.board(slug.as_str()).is_none())
        {
            return Err(ManifestError::TargetSet(format!(
                "local development target set contains unknown board {slug:?}"
            )));
        }
        Ok(Self { expected })
    }

    pub fn expected_board_slugs(&self) -> impl Iterator<Item = &str> {
        self.expected.iter().map(String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveIdentity {
    pub route: String,
    pub checksum_route: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlashPart {
    pub kind: FlashPartKind,
    pub path: String,
    pub offset: Option<u32>,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlashPartKind {
    Bootloader,
    PartitionTable,
    Application,
    Uf2,
    DfuApplication,
    DfuInitPacket,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("flash manifest is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("unsupported flash manifest schema {0}")]
    Schema(u32),

    #[error("invalid release identity: {0}")]
    Release(String),

    #[error("manifest target set is invalid: {0}")]
    TargetSet(String),

    #[error("target {board:?} disagrees with the catalog: {field}")]
    CatalogMismatch { board: String, field: String },

    #[error("target {board:?} part {path:?}: {message}")]
    InvalidPart {
        board: String,
        path: String,
        message: String,
    },
}

impl FlashManifest {
    pub fn from_json(bytes: &[u8], catalog: &BoardCatalog) -> Result<Self, ManifestError> {
        let policy = ManifestTargetSetPolicy::all_shipping_targets(catalog);
        Self::from_json_with_target_set(bytes, catalog, &policy)
    }

    pub fn from_json_with_target_set(
        bytes: &[u8],
        catalog: &BoardCatalog,
        policy: &ManifestTargetSetPolicy,
    ) -> Result<Self, ManifestError> {
        let manifest: Self = serde_json::from_slice(bytes)?;
        manifest.validate_with_target_set(catalog, policy)?;
        // Every accepted wire document must also admit the transport-specific
        // domain conversion. Compatibility callers may retain the DTO, but no
        // signed parse can bypass this trust-boundary proof.
        let _ = convert_manifest(manifest.clone(), catalog)?;
        Ok(manifest)
    }

    pub fn into_validated(
        self,
        catalog: &BoardCatalog,
    ) -> Result<ValidatedFlashManifest, ManifestError> {
        let policy = ManifestTargetSetPolicy::all_shipping_targets(catalog);
        self.into_validated_with_target_set(catalog, &policy)
    }

    pub fn into_validated_with_target_set(
        self,
        catalog: &BoardCatalog,
        policy: &ManifestTargetSetPolicy,
    ) -> Result<ValidatedFlashManifest, ManifestError> {
        self.validate_with_target_set(catalog, policy)?;
        convert_manifest(self, catalog)
    }

    pub fn to_validated(
        &self,
        catalog: &BoardCatalog,
    ) -> Result<ValidatedFlashManifest, ManifestError> {
        self.clone().into_validated(catalog)
    }

    pub fn to_validated_with_target_set(
        &self,
        catalog: &BoardCatalog,
        policy: &ManifestTargetSetPolicy,
    ) -> Result<ValidatedFlashManifest, ManifestError> {
        self.clone().into_validated_with_target_set(catalog, policy)
    }

    pub fn validate(&self, catalog: &BoardCatalog) -> Result<(), ManifestError> {
        let policy = ManifestTargetSetPolicy::all_shipping_targets(catalog);
        self.validate_with_target_set(catalog, &policy)
    }

    pub fn validate_with_target_set(
        &self,
        catalog: &BoardCatalog,
        policy: &ManifestTargetSetPolicy,
    ) -> Result<(), ManifestError> {
        if self.schema_version != FLASH_MANIFEST_SCHEMA {
            return Err(ManifestError::Schema(self.schema_version));
        }
        validate_release(self)?;
        validate_target_set(self, policy)?;
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
    pub fn from_json(bytes: &[u8], expected: ReleaseChannel) -> Result<Self, ManifestError> {
        let descriptor: Self = serde_json::from_slice(bytes)?;
        descriptor.validate(expected)?;
        let _ = convert_channel_descriptor(descriptor.clone())?;
        Ok(descriptor)
    }

    pub fn validate(&self, expected: ReleaseChannel) -> Result<(), ManifestError> {
        if self.schema_version != 1 {
            return Err(ManifestError::Release(format!(
                "unsupported channel descriptor schema {}",
                self.schema_version
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
    use crate::{
        board_catalog, BoardBuild, PreparationProfile, ReleaseTarget, ReleaseVersion,
        SoftdeviceIdentity, CONFIG_OFFSET,
    };

    fn valid_manifest() -> Result<FlashManifest, crate::catalog::CatalogError> {
        let catalog = board_catalog()?;
        let targets = catalog.shipping_boards().map(target).collect();
        Ok(FlashManifest {
            schema_version: FLASH_MANIFEST_SCHEMA,
            release: ReleaseInfo {
                version: "0.2.6".to_string(),
                channel: ReleaseChannel::Preview,
                commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
            signing: OfflineKeySigningInfo {
                key_id: "1FB2CA18B2C25E1F".to_string(),
            },
            targets,
        })
    }

    fn target(board: &crate::BoardCatalogEntry) -> TargetManifest {
        let (
            flash_mode,
            flash_frequency,
            before_reset,
            after_reset,
            parts,
            variants,
            nrf_serial_dfu,
        ) = match &board.build {
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
                Vec::new(),
                None,
            ),
            BoardBuild::Uf2(build) => (
                None,
                None,
                None,
                None,
                Vec::new(),
                build
                    .variants
                    .iter()
                    .map(|variant| Uf2VariantManifest {
                        softdevice_family: variant.softdevice_family.clone(),
                        softdevice_version: variant.softdevice_version.clone(),
                        fwid: variant.fwid.clone(),
                        application_base: variant.application_base.clone(),
                        family_id: variant.family_id.clone(),
                        path: format!("firmware/hopspot/{}/0.2.6/{}", board.slug, variant.filename),
                        size: 256,
                        sha256: "a".repeat(64),
                    })
                    .collect(),
                None,
            ),
            BoardBuild::NrfSerialDfu(build) => (
                None,
                None,
                None,
                None,
                Vec::new(),
                Vec::new(),
                Some(NrfSerialDfuManifest {
                    serial: build.serial.clone(),
                    compatibility: build.compatibility.clone(),
                    application: part(
                        board,
                        FlashPartKind::DfuApplication,
                        &build.application_filename,
                        None,
                    ),
                    init_packet: part(
                        board,
                        FlashPartKind::DfuInitPacket,
                        &build.init_packet_filename,
                        None,
                    ),
                    recovery: NrfSerialDfuRecoveryManifest {
                        mount_label: build.recovery.mount_label.clone(),
                        board_id_prefix: build.recovery.board_identity.value.clone(),
                        family_id: build.recovery.family_id.clone(),
                        artifact: part(board, FlashPartKind::Uf2, &build.recovery.filename, None),
                    },
                }),
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
            variants,
            nrf_serial_dfu,
            provisioning: board.provisioning.clone(),
            source: None,
        }
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
    fn embedded_targets_reject_source_archives() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let mut manifest = valid_manifest()?;
        manifest.targets[0].source = Some(SourceArchiveIdentity {
            route: "/file/source.zip".to_string(),
            checksum_route: "/file/source.zip.sha256".to_string(),
            size: 1,
            sha256: "a".repeat(64),
        });
        assert!(manifest.validate(&catalog).is_err());
        Ok(())
    }

    #[test]
    fn production_and_local_target_set_policies_remain_distinct(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let mut manifest = valid_manifest()?;
        manifest
            .targets
            .retain(|target| matches!(target.board_slug.as_str(), "heltec-v4" | "t-echo"));
        let selected = ["heltec-v4", "t-echo"];
        let policy = ManifestTargetSetPolicy::local_development(&catalog, &selected)?;

        assert!(manifest.validate(&catalog).is_err());
        manifest.validate_with_target_set(&catalog, &policy)?;
        let encoded = serde_json::to_vec(&manifest)?;
        ValidatedFlashManifest::from_json_with_target_set(&encoded, &catalog, &policy)?;
        assert!(ValidatedFlashManifest::from_json(&encoded, &catalog).is_err());
        Ok(())
    }

    #[test]
    fn local_t096_manifest_constructs_the_exact_shipping_target(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let board = catalog.board("t096").ok_or("missing T096 catalog entry")?;
        let mut manifest = valid_manifest()?;
        manifest.targets = vec![target(board)];
        let policy = ManifestTargetSetPolicy::local_development(&catalog, &["t096"])?;

        manifest.validate_with_target_set(&catalog, &policy)?;
        let encoded = serde_json::to_vec(&manifest)?;
        let validated =
            ValidatedFlashManifest::from_json_with_target_set(&encoded, &catalog, &policy)?;
        let [target] = validated.targets() else {
            return Err("expected one validated T096 target".into());
        };
        assert_eq!(target.board_id().as_str(), "t096");
        assert_eq!(target.preparation_profile(), PreparationProfile::T096Uf2);
        let ReleaseTarget::Uf2(target) = target else {
            return Err("T096 did not convert to UF2".into());
        };
        let [variant] = target.variants() else {
            return Err("expected one validated T096 UF2 variant".into());
        };
        assert_eq!(
            variant.compatibility().softdevice(),
            &SoftdeviceIdentity::parse("s140", "6.1.1")?
        );
        assert_eq!(variant.compatibility().fwid(), 0x00b6);
        assert_eq!(variant.compatibility().application_base(), 0x0002_6000);
        assert_eq!(
            variant.compatibility().application_end_exclusive(),
            0x000e_8000
        );
        assert_eq!(variant.compatibility().family_id(), 0xada5_2840);
        assert_eq!(
            variant.part().path().as_str(),
            "firmware/hopspot/t096/0.2.6/t096-s140-6.1.1.uf2"
        );
        Ok(())
    }

    #[test]
    fn local_target_set_policy_rejects_invalid_requests_and_manifest_sets(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        assert!(ManifestTargetSetPolicy::local_development(&catalog, &[]).is_err());
        assert!(
            ManifestTargetSetPolicy::local_development(&catalog, &["heltec-v4", "heltec-v4"])
                .is_err()
        );
        assert!(ManifestTargetSetPolicy::local_development(&catalog, &["unknown-board"]).is_err());

        let selected = ["heltec-v4", "t-echo"];
        let policy = ManifestTargetSetPolicy::local_development(&catalog, &selected)?;
        let mut exact = valid_manifest()?;
        exact
            .targets
            .retain(|target| matches!(target.board_slug.as_str(), "heltec-v4" | "t-echo"));

        let mut missing = exact.clone();
        missing.targets.pop();
        assert!(missing.validate_with_target_set(&catalog, &policy).is_err());

        let mut duplicate = exact.clone();
        duplicate.targets.push(duplicate.targets[0].clone());
        assert!(duplicate
            .validate_with_target_set(&catalog, &policy)
            .is_err());

        let mut unknown = exact.clone();
        unknown.targets[0].board_slug = "unknown-board".to_string();
        assert!(unknown.validate_with_target_set(&catalog, &policy).is_err());

        let extra = valid_manifest()?;
        assert!(extra.validate_with_target_set(&catalog, &policy).is_err());
        Ok(())
    }

    #[test]
    fn schema_three_wire_roundtrips_through_transport_typed_domain(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let wire = valid_manifest()?;
        let encoded = serde_json::to_vec(&wire)?;

        assert_eq!(FlashManifest::from_json(&encoded, &catalog)?, wire);

        let validated = ValidatedFlashManifest::from_json(&encoded, &catalog)?;
        assert_eq!(validated.schema_version(), FLASH_MANIFEST_SCHEMA);
        assert_eq!(validated.release().version().as_str(), "0.2.6");
        assert_eq!(validated.targets().len(), 8);
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
            4
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
            t_echo
                .variants()
                .iter()
                .map(|variant| variant.part().path().as_str())
                .collect::<Vec<_>>(),
            [
                "firmware/hopspot/t-echo/0.2.6/t-echo-s140-6.1.1.uf2",
                "firmware/hopspot/t-echo/0.2.6/t-echo-s140-7.3.0.uf2",
            ]
        );
        Ok(())
    }

    #[test]
    fn local_nordic_dfu_target_has_three_typed_artifacts() -> Result<(), Box<dyn std::error::Error>>
    {
        let catalog = board_catalog()?;
        let board = catalog.board("t1000-e").ok_or("missing T1000-E")?;
        let policy = ManifestTargetSetPolicy::local_development(&catalog, &["t1000-e"])?;
        let mut manifest = valid_manifest()?;
        manifest.targets = vec![target(board)];
        manifest.validate_with_target_set(&catalog, &policy)?;
        let validated = ValidatedFlashManifest::from_json_with_target_set(
            &serde_json::to_vec(&manifest)?,
            &catalog,
            &policy,
        )?;
        let [ReleaseTarget::NrfSerialDfu(target)] = validated.targets() else {
            return Err("T1000-E did not convert to Nordic serial DFU".into());
        };
        assert_eq!(
            [
                target.application().kind(),
                target.init_packet().kind(),
                target.recovery().artifact().kind(),
            ],
            [
                FlashPartKind::DfuApplication,
                FlashPartKind::DfuInitPacket,
                FlashPartKind::Uf2,
            ]
        );
        assert_eq!(
            target.compatibility().application_end_exclusive(),
            0x000e_a000
        );
        Ok(())
    }

    #[test]
    fn local_uf2_validation_accepts_only_the_selected_catalog_variant(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let catalog = board_catalog()?;
        let board = catalog
            .board("t-echo")
            .ok_or("missing T-Echo catalog entry")?;
        let mut target = valid_manifest()?
            .targets
            .into_iter()
            .find(|target| target.board_slug == "t-echo")
            .ok_or("missing T-Echo target")?;
        target
            .variants
            .retain(|variant| variant.softdevice_version == "6.1.1");
        let version = ReleaseVersion::parse("0.2.6")?;
        let v6 = SoftdeviceIdentity::parse("s140", "6.1.1")?;
        let v7 = SoftdeviceIdentity::parse("s140", "7.3.0")?;

        assert!(target.clone().into_validated(board, &version).is_err());
        assert!(target
            .clone()
            .into_validated_uf2_variant(board, &version, &v7)
            .is_err());
        let ReleaseTarget::Uf2(validated) =
            target.into_validated_uf2_variant(board, &version, &v6)?
        else {
            return Err("selected T-Echo target did not remain UF2".into());
        };
        assert_eq!(validated.variants().len(), 1);
        assert_eq!(validated.variants()[0].compatibility().softdevice(), &v6);
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
            schema_version: 1,
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
